wit_bindgen::generate!({path:"wit",world:"watermelon",generate_all});
use astrobox::psys_host_v4::{
    account, device, dialog, interconnect, register, thirdpartyapp, timer, ui,
};
use exports::astrobox::psys_plugin_v4::{
    event::{self, EventType},
    lifecycle,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use uuid::Uuid;
mod identity;
mod images;
mod net;
mod skins;
mod view;

struct Probe {
    id: String,
    addr: String,
    tries: u8,
}
struct Batch {
    skin: Option<usize>,
    completed: usize,
}
/// 会覆盖手环上已换肤水果的整套操作，先在页面内确认一次。
#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    ApplySkin,
    ResetAll,
}
#[derive(Default)]
struct State {
    root: Option<String>,
    confirm: Option<Confirm>,
    devices: Vec<device::DeviceInfo>,
    selected: Option<usize>,
    connected: bool,
    probe: Option<Probe>,
    connecting: bool,
    level: usize,
    skin: usize,
    picking: bool,
    verifying: bool,
    account_id: Option<String>,
    entitled: bool,
    account_message: String,
    pending: Option<images::Converted>,
    custom: [bool; 11],
    current: [Option<String>; 11],
    read_failed: [bool; 11],
    readable: bool,
    /// 当前手环要接收的图片格式；小米手环 9/9 Pro 为 PNG。
    format: images::Format,
    read: Option<images::PreviewRead>,
    transfer: Option<images::Transfer>,
    transfer_preview: Option<String>,
    batch: Option<Batch>,
    message: String,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
struct Watermelon;
fn locked(s: &State) -> bool {
    s.connecting || s.probe.is_some() || s.picking || s.verifying || s.transfer.is_some()
}
fn target(s: &State) -> Option<String> {
    if !s.connected {
        return None;
    }
    s.selected
        .and_then(|i| s.devices.get(i))
        .map(|d| d.addr.clone())
}
fn show(message: impl Into<String>) {
    STATE.with(|state| state.borrow_mut().message = message.into());
    render();
}
fn current_image(s: &State, level: usize) -> Option<&str> {
    if s.custom[level] {
        s.current[level].as_deref()
    } else {
        Some(skins::preview(0, level))
    }
}
fn custom_count(s: &State) -> usize {
    s.custom.iter().filter(|custom| **custom).count()
}
/// 已换肤但还没有预览时，区分「正在 / 即将回读」与「读不到」。
fn preview_placeholder(s: &State, level: usize) -> &'static str {
    let reading = s.read.as_ref().is_some_and(|read| read.level == level);
    if reading || (s.readable && !s.read_failed[level]) {
        "读取中…"
    } else {
        "预览不可用"
    }
}
fn stage(source: Option<&str>, placeholder: &str, caption: &str) -> ui::Element {
    let body = match source {
        Some(source) => view::image(source, 104),
        None => view::label(placeholder).height(104),
    };
    view::column()
        .align_center()
        .width(120)
        .child(body)
        .child(view::label(caption))
}
fn device_name(d: &device::DeviceInfo) -> &str {
    if d.name.is_empty() { &d.addr } else { &d.name }
}
/// 手环选择与连接：没连上时连接按钮是页面上唯一的主按钮。
fn device_section(s: &State, busy: bool) -> ui::Element {
    let mut selector = ui::Element::new(ui::ElementType::Select, Some("选择手环"))
        .on(ui::Event::Change, "device-select")
        .prop(
            "value",
            &s.selected.map(|i| i.to_string()).unwrap_or_default(),
        )
        .flex_grow(1.0);
    for (i, d) in s.devices.iter().enumerate() {
        selector = selector.child(
            ui::Element::new(ui::ElementType::Option, Some(device_name(d)))
                .prop("value", &i.to_string()),
        );
    }
    if busy || s.devices.is_empty() {
        selector = selector.disabled();
    }
    let connecting = s.connecting || s.probe.is_some();
    let label = if connecting {
        "连接中…"
    } else if s.connected {
        "重新连接"
    } else {
        "连接"
    };
    let mut section = view::column()
        .gap(8)
        .child(view::row().child(selector).child(view::button(
            label,
            "connect",
            !s.connected && !connecting,
            busy,
        )));
    if !s.connected && !connecting {
        section = section.child(view::label(if s.devices.is_empty() {
            "请先在 AstroBox 中连接手环，然后点击「连接」。"
        } else {
            "点击「连接」后，手环会自动打开换肤页面；如果没有打开，请在手环的合成大西瓜中进入「设置 > 水果换肤」。需要合成大西瓜 1.2.0 或更高版本。"
        }));
    }
    section
}
/// 进度、结果与错误统一放在页面顶部，点完上方按钮也能立刻看到反馈。
fn status_section(s: &State) -> Option<ui::Element> {
    if s.transfer.is_none() && s.message.is_empty() {
        return None;
    }
    let mut panel = view::panel().gap(8);
    if let Some(t) = &s.transfer {
        let progress = transfer_progress(s);
        panel = panel
            .child(view::text(&format!("{} · {}%", task_name(s), progress)))
            .child(view::progress(progress));
        if t.failed {
            panel = panel.child(
                view::row()
                    .child(view::button("重试", "image-retry", true, false))
                    .child(view::button("停止", "transfer-discard", false, false)),
            );
        }
    }
    if !s.message.is_empty() {
        panel = panel.child(view::label(&s.message));
    }
    Some(panel)
}
fn skin_section(s: &State, busy: bool) -> ui::Element {
    let mut grid = ui::Element::new(ui::ElementType::Grid, None)
        .grid_template_columns("repeat(2,minmax(0,1fr))")
        .gap(8)
        .width_full();
    for (i, skin) in skins::SKINS.iter().enumerate() {
        let tile = view::skin_tile(i == s.skin)
            .on(ui::Event::Click, &format!("skin-select:{i}"))
            .prop("aria-pressed", if i == s.skin { "true" } else { "false" })
            .prop("aria-label", skin.id)
            .child(view::image(skins::preview(i, 10), 56))
            .child(view::text(skin.name));
        grid = grid.child(if busy { tile.disabled() } else { tile });
    }
    let name = skins::SKINS[s.skin].name;
    // 选中皮肤的全部 11 种水果，从小到大，切换皮肤即可直观对比样式。
    let mut gallery = ui::Element::new(ui::ElementType::Grid, None)
        .grid_template_columns("repeat(auto-fill,minmax(52px,1fr))")
        .gap(8)
        .width_full();
    for level in 0..11 {
        gallery = gallery.child(
            view::column()
                .gap(4)
                .align_center()
                .child(view::image(skins::preview(s.skin, level), 44))
                .child(view::label(images::NAMES[level])),
        );
    }
    let gallery = view::panel()
        .child(view::label(&format!("「{name}」预览")))
        .child(gallery);
    let replaced = custom_count(s);
    let actions = match s.confirm {
        Some(confirm) => {
            let question = match confirm {
                Confirm::ApplySkin => format!(
                    "手环上已有 {replaced} 种水果换过图片，应用「{name}」后将被替换。确定继续吗？"
                ),
                Confirm::ResetAll => {
                    format!("确定把 {replaced} 种水果全部恢复为默认图片吗？")
                }
            };
            view::column().gap(8).child(view::text(&question)).child(
                view::row()
                    .child(view::button(
                        "确定",
                        "confirm-yes",
                        true,
                        busy || !s.connected,
                    ))
                    .child(view::button("取消", "confirm-no", false, false)),
            )
        }
        None => view::row()
            .child(view::button(
                &format!("应用「{name}」"),
                "skin-apply",
                s.connected,
                busy || !s.connected,
            ))
            .child(view::button(
                "全部恢复默认",
                "reset-all",
                false,
                busy || !s.connected || replaced == 0,
            )),
    };
    view::column()
        .child(view::heading("整套皮肤"))
        .child(view::label("选择一套皮肤，一键更换全部水果。"))
        .child(grid)
        .child(gallery)
        .child(actions)
}
fn fruit_section(s: &State, busy: bool) -> ui::Element {
    let level = s.level;
    let name = images::NAMES[level];
    let mut grid = ui::Element::new(ui::ElementType::Grid, None)
        .grid_template_columns("repeat(auto-fit,minmax(80px,1fr))")
        .gap(8)
        .width_full();
    for i in 0..11 {
        let source = current_image(s, i);
        let mut tile = view::fruit_tile(i == level)
            .on(ui::Event::Click, &format!("fruit-select:{i}"))
            .prop("aria-pressed", if i == level { "true" } else { "false" })
            .child(view::image(source.unwrap_or(skins::preview(0, i)), 36))
            .child(view::label(images::NAMES[i]));
        if s.custom[i] {
            tile = tile.child(view::label("已换肤"));
        }
        grid = grid.child(if busy { tile.disabled() } else { tile });
    }
    // 选中水果的详情：左边是手环上的现状，选了自定义图片后右边出现待应用的预览。
    let mut preview = view::row().justify_center().child(stage(
        current_image(s, level),
        preview_placeholder(s, level),
        if s.custom[level] {
            "当前"
        } else {
            "当前（默认）"
        },
    ));
    if let Some(pending) = &s.pending {
        preview = preview.child(view::text("→")).child(stage(
            Some(pending.preview.as_str()),
            "",
            "新图片",
        ));
    }
    let mut detail = view::panel().child(view::heading(name)).child(preview);
    if s.custom[level] && s.current[level].is_none() && s.read_failed[level] {
        detail = detail.child(view::row().child(view::button(
            "重新加载预览",
            "image-view",
            false,
            busy || !s.connected,
        )));
    }
    detail = detail.child(if s.pending.is_some() {
        view::row()
            .child(view::button(
                "应用到手环",
                "image-apply",
                true,
                busy || !s.connected,
            ))
            .child(view::button("取消", "image-discard", false, busy))
    } else {
        let pick = if s.verifying {
            "正在验证购买…".to_string()
        } else if s.picking {
            "正在选择图片…".to_string()
        } else if s.entitled {
            format!("更换「{name}」图片")
        } else {
            format!("更换「{name}」图片（需购买）")
        };
        let mut row = view::row().child(view::button(
            &pick,
            "image-file",
            false,
            busy || !s.connected,
        ));
        if s.custom[level] {
            row = row.child(view::button(
                "恢复默认",
                "image-reset",
                false,
                busy || !s.connected,
            ));
        }
        row
    });
    if !s.account_message.is_empty() {
        detail = detail.child(view::label(&s.account_message));
    } else if !s.entitled {
        detail = detail.child(view::label(
            "自定义图片需在爱发电购买，并在 AstroBox 账号设置中绑定同一个爱发电账号。",
        ));
    }
    if !s.entitled {
        detail = detail.child(
            view::row()
                .child(view::button(
                    if s.verifying {
                        "验证中…"
                    } else {
                        "我已购买"
                    },
                    "account-check",
                    false,
                    busy,
                ))
                .child(view::button("去购买", "purchase", false, false)),
        );
    }
    view::column()
        .child(view::heading("单个水果"))
        .child(view::label("选择一种水果，换成你喜欢的图片，或恢复默认。"))
        .child(grid)
        .child(detail)
}
fn render() {
    STATE.with(|state| {
        let s = state.borrow();
        let Some(root) = s.root.as_deref() else {
            return;
        };
        let busy = locked(&s);
        let mut page = view::column()
            .max_width(640)
            .padding(16)
            .child(view::title("合成大西瓜换肤"))
            .child(view::label("为手环上的合成大西瓜更换水果图片"))
            .child(device_section(&s, busy));
        if let Some(status) = status_section(&s) {
            page = page.child(status);
        }
        page = page
            .child(skin_section(&s, busy))
            .child(fruit_section(&s, busy));
        ui::render(root, view::column().align_center().child(page));
    });
}
fn task_name(s: &State) -> String {
    match &s.batch {
        Some(batch) => {
            let step = (batch.completed + 1).min(11);
            match batch.skin {
                Some(i) => format!("正在应用「{}」 · 第 {step}/11 种", skins::SKINS[i].name),
                None => format!("正在全部恢复默认 · 第 {step}/11 种"),
            }
        }
        None => {
            let t = s.transfer.as_ref();
            let name = images::NAMES[t.map(|t| t.level).unwrap_or(s.level)];
            if t.is_some_and(|t| t.stage == images::Stage::Reset) {
                format!("正在恢复「{name}」默认图片")
            } else {
                format!("正在更换「{name}」图片")
            }
        }
    }
}
fn transfer_progress(s: &State) -> u32 {
    let progress = s
        .transfer
        .as_ref()
        .map(|t| t.progress().min(99))
        .unwrap_or(0);
    if let Some(batch) = &s.batch {
        ((batch.completed as u32 * 100 + progress) / 11).min(99)
    } else {
        progress
    }
}
fn clear_connection(s: &mut State) {
    s.connected = false;
    s.probe = None;
    s.read = None;
    s.current = Default::default();
    s.custom = [false; 11];
    s.read_failed = [false; 11];
    s.readable = false;
    s.pending = None;
    s.confirm = None;
}
async fn send(addr: &str, packet: String) -> Result<(), String> {
    register::register_interconnect_recv(addr.to_string(), images::PRODUCT.to_string()).await?;
    interconnect::send_qaic_message(addr.to_string(), images::PRODUCT.to_string(), packet).await
}
/// 刷新已连接手环列表，尽量保留原来选中的那只。
async fn load_devices() {
    let devices = device::get_connected_device_list().await;
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let previous = s
            .selected
            .and_then(|i| s.devices.get(i))
            .map(|d| d.addr.clone());
        s.selected = previous
            .and_then(|addr| devices.iter().position(|d| d.addr == addr))
            .or(if devices.is_empty() { None } else { Some(0) });
        s.devices = devices;
    });
}
async fn connect(refresh_devices: bool) {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        clear_connection(&mut s);
        s.connecting = true;
        s.message = "正在连接手环…".into();
    });
    render();
    if refresh_devices {
        load_devices().await;
    }
    let addr = STATE.with(|state| {
        let s = state.borrow();
        s.selected
            .and_then(|i| s.devices.get(i))
            .map(|d| d.addr.clone())
    });
    let Some(addr) = addr else {
        STATE.with(|state| state.borrow_mut().connecting = false);
        show("没有找到已连接的手环，请先在 AstroBox 中连接手环");
        return;
    };
    match thirdpartyapp::get_thirdparty_app_list(addr.clone()).await {
        Ok(apps) => {
            let Some(app) = apps
                .into_iter()
                .find(|app| app.package_name == images::PRODUCT)
            else {
                STATE.with(|state| state.borrow_mut().connecting = false);
                show("手环上还没有安装合成大西瓜");
                return;
            };
            let mut launched = false;
            for page in ["pages/activation", "/pages/activation"] {
                if thirdpartyapp::launch_qa(addr.clone(), app.clone(), page.to_string())
                    .await
                    .is_ok()
                {
                    launched = true;
                    break;
                }
            }
            if !launched {
                STATE.with(|state| state.borrow_mut().connecting = false);
                show("无法自动打开手环上的换肤页面，请在手环上手动打开后重试");
                return;
            }
        }
        Err(reason) => {
            STATE.with(|state| state.borrow_mut().connecting = false);
            show(format!(
                "无法读取手环上的应用，请确认手环已连接后重试（{reason}）"
            ));
            return;
        }
    }
    let id = Uuid::new_v4().to_string();
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.connecting = false;
        s.probe = Some(Probe {
            id: id.clone(),
            addr: addr.clone(),
            tries: 0,
        });
    });
    if let Err(error) = send(&addr, json!({"v":1,"id":id,"type":"hello"}).to_string()).await {
        STATE.with(|state| state.borrow_mut().probe = None);
        show(error);
        return;
    }
    timer::set_timeout(2000, &format!("probe:{id}"));
    render();
}
fn next_read(s: &mut State) -> bool {
    if !s.connected || !s.readable || s.read.is_some() || s.transfer.is_some() || s.probe.is_some()
    {
        return false;
    }
    let needed = |i: usize| s.custom[i] && s.current[i].is_none() && !s.read_failed[i];
    let level = if needed(s.level) {
        Some(s.level)
    } else {
        (0..11).find(|i| needed(*i))
    };
    if let Some(level) = level {
        s.read = Some(images::PreviewRead::new(level));
        true
    } else {
        false
    }
}
async fn send_read() {
    let packet = STATE.with(|state| {
        let s = state.borrow();
        let read = s.read.as_ref()?;
        Some((
            target(&s)?,
            read.packet(),
            format!("read:{}:{}", read.id, read.bytes.len()),
        ))
    });
    if let Some((addr, packet, key)) = packet {
        if let Err(error) = send(&addr, packet).await {
            fail_read();
            show(error);
            continue_read().await;
        } else {
            timer::set_timeout(3000, &key);
        }
    }
}
fn fail_read() {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(read) = s.read.take() {
            s.read_failed[read.level] = true;
        }
    });
}
async fn continue_read() {
    loop {
        let packet = STATE.with(|state| {
            let mut s = state.borrow_mut();
            if !next_read(&mut s) {
                return None;
            }
            let read = s.read.as_ref()?;
            Some((
                target(&s)?,
                read.packet(),
                format!("read:{}:{}", read.id, read.bytes.len()),
            ))
        });
        let Some((addr, packet, key)) = packet else {
            render();
            return;
        };
        if send(&addr, packet).await.is_ok() {
            timer::set_timeout(3000, &key);
            render();
            return;
        }
        fail_read();
    }
}
/// 点击「验证」「选择图片」时只锁界面并画出「验证中…」，验证本身
/// 交给定时事件：宿主在导出返回后才绘制，就地读取账号、请求服务会让画面停在
/// 点击前的样子，用户以为没点上。
const VERIFY_PREFIX: &str = "verify:";
fn queue_verify(action: &str) {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.verifying = true;
        s.account_message = "正在验证购买记录…首次验证需要允许读取 AstroBox 账号".into();
    });
    render();
    timer::set_timeout(1, &format!("{VERIFY_PREFIX}{action}"));
}
async fn verify() -> bool {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.verifying = true;
        s.entitled = false;
        s.account_id = None;
    });
    render();
    let result = identity::check().await;
    let enabled = STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.verifying = false;
        match result {
            Ok((id, paid)) => {
                s.account_id = Some(id);
                s.entitled = paid;
                s.account_message = if paid {
                    "已解锁自定义图片"
                } else {
                    "没有找到购买记录。请确认 AstroBox 绑定的是购买时使用的爱发电账号。"
                }
                .into();
                paid
            }
            Err(error) => {
                s.account_message = error;
                false
            }
        }
    });
    render();
    enabled
}
/// 购买只在选图时把关：本次会话首次选图验证一次，验证通过后选图、应用都不再验证。
/// 宿主没有账号切换事件，会话内切换 AstroBox 账号不会自动重新验证；点「验证购买」
/// 或重新打开插件可刷新。这是插件本地功能解锁，本就不是服务端级别的防护。
fn entitlement_cached(s: &State) -> bool {
    s.entitled && s.account_id.is_some()
}
const PICK_TICK: &str = "pick";
fn queue_pick() {
    if !STATE.with(|state| entitlement_cached(&state.borrow())) {
        queue_verify("pick");
        return;
    }
    // 与验证一样交给定时事件：先把「选择图片中…」画出来再弹文件框。
    STATE.with(|state| state.borrow_mut().picking = true);
    render();
    timer::set_timeout(1, PICK_TICK);
}
async fn pick_image(verify_first: bool) {
    if verify_first && !verify().await {
        return;
    }
    STATE.with(|state| state.borrow_mut().picking = true);
    render();
    let result = dialog::pick_file(
        dialog::PickConfig {
            read: true,
            copy_to: None,
        },
        dialog::FilterConfig {
            multiple: false,
            extensions: ["png", "jpg", "jpeg", "webp", "bmp", "gif"]
                .map(str::to_owned)
                .to_vec(),
            default_directory: String::new(),
            default_file_name: String::new(),
        },
    )
    .await;
    let (level, format) = STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.picking = false;
        (s.level, s.format)
    });
    match result {
        Ok(file) if file.data.is_empty() => render(),
        Ok(file) => match images::convert(&file.data, level, format) {
            Ok(image) => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.pending = Some(image);
                    s.message = "看看效果，满意就点「应用到手环」".into();
                });
                render();
            }
            Err(error) => show(error),
        },
        Err(error) => show(error),
    }
}
fn next_transfer(s: &mut State) {
    let Some(batch) = s.batch.as_ref() else {
        return;
    };
    let Some(addr) = target(s) else {
        return;
    };
    let level = batch.completed;
    let image = batch.skin.map(|i| {
        images::Converted::indexed(
            skins::SKINS[i].fruits[level].to_vec(),
            String::new(),
            s.format,
        )
        .expect("bundled skin image")
    });
    s.transfer_preview = batch.skin.map(|i| skins::preview(i, level).to_string());
    s.transfer = Some(images::Transfer::new(addr, level, image));
}
async fn apply_skin(reset: bool) {
    let ready = STATE.with(|state| {
        let mut s = state.borrow_mut();
        // 没有选中的手环就不开始；地址由 next_transfer 自己再取。
        target(&s)?;
        s.message.clear();
        s.pending = None;
        s.read = None;
        s.batch = Some(Batch {
            skin: if reset { None } else { Some(s.skin) },
            completed: 0,
        });
        next_transfer(&mut s);
        Some(())
    });
    if ready.is_some() {
        send_transfer().await;
    }
}
/// 待应用图片只能经过选图那一步产生，而选图已经确认过购买，这里不再验证。
async fn apply_custom() {
    let ready = STATE.with(|state| {
        let mut s = state.borrow_mut();
        let addr = target(&s)?;
        let image = s.pending.take()?;
        if image.format != s.format {
            // 选图后换了手环：格式不符的图片不能发，请重新选择。
            s.message = "手环已更换，请重新选择图片".into();
            return None;
        }
        s.message.clear();
        s.transfer_preview = Some(image.preview.clone());
        let level = s.level;
        s.read = None;
        s.batch = None;
        s.transfer = Some(images::Transfer::new(addr, level, Some(image)));
        Some(())
    });
    if ready.is_some() {
        send_transfer().await;
    } else {
        render();
    }
}
async fn reset_current() {
    let ready = STATE.with(|state| {
        let mut s = state.borrow_mut();
        let addr = target(&s)?;
        let level = s.level;
        s.message.clear();
        s.pending = None;
        s.read = None;
        s.batch = None;
        s.transfer_preview = None;
        s.transfer = Some(images::Transfer::new(addr, level, None));
        Some(())
    });
    if ready.is_some() {
        send_transfer().await;
    }
}
async fn fail_transfer(message: impl Into<String>) {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(t) = s.transfer.as_mut() {
            t.failed = true;
            t.waiting = false;
        }
        s.message = message.into();
    });
    render();
}
async fn send_transfer() {
    let packet = STATE.with(|state| {
        let s = state.borrow();
        let t = s.transfer.as_ref()?;
        if !t.waiting || t.failed {
            return None;
        }
        Some((
            t.addr.clone(),
            t.packet(),
            format!("transfer:{}:{}", t.id, t.sequence),
        ))
    });
    if let Some((addr, packet, key)) = packet {
        render();
        match send(&addr, packet).await {
            Ok(()) => {
                timer::set_timeout(8000, &key);
            }
            Err(error) => fail_transfer(error).await,
        }
    }
}
async fn receive(value: Value) {
    if value["v"] != 1 {
        return;
    }
    if value["type"] == "device" {
        let matched = STATE.with(|state| {
            state
                .borrow()
                .probe
                .as_ref()
                .is_some_and(|p| value["id"] == p.id)
        });
        if !matched {
            return;
        }
        let compatible = value["productId"] == images::PRODUCT
            && value["images"]["v"] == 1
            && value["images"]["freeTransfer"] == true
            && value["images"]["sizes"] == json!(images::SIZES)
            && value["images"]["maxBytes"] == images::MAX_BYTES
            && value["images"]["chunkBytes"] == images::CHUNK_BYTES;
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.probe = None;
            s.connected = compatible;
            if compatible {
                s.message = "手环已连接".into();
                s.readable = value["images"]["preview"] == true;
                let format = images::Format::from_value(&value["images"]["format"]);
                if s.format != format {
                    s.pending = None;
                }
                s.format = format;
                s.custom = [false; 11];
                if let Some(levels) = value["images"]["customLevels"].as_array() {
                    for level in levels
                        .iter()
                        .filter_map(Value::as_u64)
                        .filter(|level| *level < 11)
                    {
                        s.custom[level as usize] = true;
                    }
                }
            } else {
                s.message = "手环上的合成大西瓜版本过旧，请更新到 1.2.0 或更高版本".into();
            }
        });
        continue_read().await;
        return;
    }
    if value["type"] == "image-preview" {
        let result = STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.read.as_mut().map(|read| read.accept(&value))
        });
        match result {
            Some(Ok(Some(images::PreviewResult::Next))) => send_read().await,
            Some(Ok(Some(images::PreviewResult::Complete))) => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    let Some(read) = s.read.take() else {
                        return;
                    };
                    if read.bytes.is_empty() {
                        s.custom[read.level] = false;
                        s.current[read.level] = None;
                    } else {
                        match read.preview() {
                            Ok(preview) => s.current[read.level] = Some(preview),
                            Err(error) => {
                                s.read_failed[read.level] = true;
                                s.message = error;
                            }
                        }
                    }
                });
                continue_read().await;
            }
            Some(Ok(Some(images::PreviewResult::Restart))) => {
                let retry = STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    let read = s.read.as_mut()?;
                    if read.restarts >= 2 {
                        return Some(false);
                    }
                    read.restart();
                    Some(true)
                });
                if retry == Some(true) {
                    send_read().await;
                } else {
                    fail_read();
                    continue_read().await;
                }
            }
            Some(Err(error)) => {
                fail_read();
                show(error);
                continue_read().await;
            }
            _ => {}
        }
        return;
    }
    if value["type"] == "image-result" {
        let result = STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.transfer.as_mut().map(|t| t.response(&value))
        });
        match result {
            Some(Ok(Some(false))) => send_transfer().await,
            Some(Ok(Some(true))) => {
                let more = STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    let Some(t) = s.transfer.take() else {
                        return false;
                    };
                    let level = t.level;
                    let reset = t.stage == images::Stage::Reset;
                    s.custom[level] = !reset;
                    s.current[level] = s.transfer_preview.take();
                    s.read_failed[level] = false;
                    if let Some(batch) = s.batch.as_mut() {
                        batch.completed += 1;
                        if batch.completed < 11 {
                            next_transfer(&mut s);
                            return true;
                        }
                    }
                    let name = images::NAMES[level];
                    s.message = match s.batch.as_ref().map(|batch| batch.skin) {
                        Some(Some(skin)) => {
                            format!("「{}」皮肤已应用到手环", skins::SKINS[skin].name)
                        }
                        Some(None) => "已全部恢复默认图片".into(),
                        None if reset => format!("「{name}」已恢复默认图片"),
                        None => format!("「{name}」已换成新图片"),
                    };
                    false
                });
                if more {
                    send_transfer().await;
                } else {
                    STATE.with(|state| state.borrow_mut().batch = None);
                    render();
                    continue_read().await;
                }
            }
            Some(Err(error)) => fail_transfer(error).await,
            _ => {}
        }
    }
}
fn event_text(payload: &str) -> String {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::String(payload.into()));
    value
        .as_str()
        .or_else(|| value["value"].as_str())
        .unwrap_or(payload)
        .into()
}
/// 宿主的 timer 事件把 `set_timeout` 的标签包在 JSON 的 `payload` 字段里
/// （与 Kovela 插件同一处理）；不是 JSON 时按原文处理。之前误用 `event_text`
/// 只认 `value` 字段，整段 JSON 原样落进 `tick`，所有定时器都对不上。
fn timer_body(payload: &str) -> String {
    serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("payload")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| payload.to_string())
}
const DEVICES_TICK: &str = "devices";
async fn tick(body: &str) {
    if body == PICK_TICK {
        pick_image(false).await;
    } else if body == DEVICES_TICK {
        if STATE.with(|state| locked(&state.borrow())) {
            return;
        }
        load_devices().await;
        render();
    } else if let Some(action) = body.strip_prefix(VERIFY_PREFIX) {
        match action {
            "pick" => pick_image(true).await,
            _ => {
                verify().await;
            }
        }
    } else if let Some(id) = body.strip_prefix("probe:") {
        let retry = STATE.with(|state| {
            let mut s = state.borrow_mut();
            let p = s.probe.as_mut()?;
            if p.id != id {
                return None;
            }
            if p.tries >= 9 {
                return Some(None);
            }
            p.tries += 1;
            Some(Some(p.addr.clone()))
        });
        match retry {
            Some(Some(addr)) => {
                let _ = send(&addr, json!({"v":1,"id":id,"type":"hello"}).to_string()).await;
                timer::set_timeout(2000, body);
            }
            Some(None) => {
                STATE.with(|state| state.borrow_mut().probe = None);
                show("连接超时，请确认手环上的换肤页面已打开，然后重试");
            }
            None => {}
        }
    } else if let Some(key) = body.strip_prefix("transfer:") {
        let retry = STATE.with(|state| {
            let mut s = state.borrow_mut();
            let t = s.transfer.as_mut()?;
            if key != format!("{}:{}", t.id, t.sequence) || !t.waiting || t.failed {
                return None;
            }
            if t.retries >= 3 {
                return Some(false);
            }
            t.retries += 1;
            Some(true)
        });
        match retry {
            Some(true) => send_transfer().await,
            Some(false) => fail_transfer("手环没有响应，请确认换肤页面仍然打开后重试").await,
            None => {}
        }
    } else if let Some(key) = body.strip_prefix("read:") {
        let retry = STATE.with(|state| {
            let mut s = state.borrow_mut();
            let read = s.read.as_mut()?;
            if key != format!("{}:{}", read.id, read.bytes.len()) {
                return None;
            }
            if read.retries < 2 {
                read.retries += 1;
                Some(true)
            } else if read.restarts < 2 {
                read.restart();
                Some(true)
            } else {
                Some(false)
            }
        });
        if retry == Some(true) {
            send_read().await;
        } else if retry == Some(false) {
            fail_read();
            continue_read().await;
        }
    }
}
impl lifecycle::Guest for Watermelon {
    async fn on_load() {}
}
impl event::Guest for Watermelon {
    async fn on_ui_render(id: String) {
        let load = STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.root = Some(id);
            s.devices.is_empty() && !locked(&s)
        });
        render();
        // 打开页面就列出已连接的手环，用户只需点「连接」；不自动拉起手环应用。
        if load {
            timer::set_timeout(1, DEVICES_TICK);
        }
    }
    async fn on_card_render(_id: String) {}
    async fn on_event(kind: EventType, payload: String) -> String {
        match kind {
            EventType::InterconnectMessage => {
                if let Ok(outer) = serde_json::from_str::<Value>(&payload) {
                    let value = outer["payloadText"]
                        .as_str()
                        .and_then(|text| serde_json::from_str(text).ok())
                        .unwrap_or(outer);
                    receive(value).await;
                }
            }
            EventType::Timer => {
                tick(&timer_body(&payload)).await;
            }
            _ => {}
        }
        String::new()
    }
    async fn on_ui_event(id: String, event: ui::Event, payload: String) -> String {
        if id == "purchase" && event == ui::Event::Click {
            dialog::open_url(identity::PURCHASE_URL);
            return String::new();
        }
        if id == "device-select"
            && event == ui::Event::Change
            && !STATE.with(|state| locked(&state.borrow()))
        {
            let changed = event_text(&payload).parse::<usize>().ok().is_some_and(|i| {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    if i >= s.devices.len() {
                        return false;
                    }
                    s.selected = Some(i);
                    true
                })
            });
            if changed {
                connect(false).await;
            }
            return String::new();
        }
        if event != ui::Event::Click {
            return String::new();
        }
        if id == "image-retry" {
            let retry = STATE.with(|state| {
                let mut s = state.borrow_mut();
                let t = s.transfer.as_mut()?;
                if !t.failed {
                    return None;
                }
                t.failed = false;
                t.waiting = true;
                t.retries = 0;
                t.sequence += 1;
                s.message.clear();
                Some(())
            });
            if retry.is_some() {
                send_transfer().await;
            }
            return String::new();
        }
        if id == "transfer-discard" {
            STATE.with(|state| {
                let mut s = state.borrow_mut();
                if !s.transfer.as_ref().is_some_and(|t| t.failed) {
                    return;
                }
                s.transfer = None;
                s.batch = None;
                s.transfer_preview = None;
                s.message = "已停止。已经换好的水果会保留新图片，其余保持不变".into();
            });
            render();
            return String::new();
        }
        if STATE.with(|state| locked(&state.borrow())) {
            return String::new();
        }
        // 页面内确认只对紧接着的一次点击有效，点别处就当作取消。
        let confirm = STATE.with(|state| state.borrow_mut().confirm.take());
        if let Some(i) = id
            .strip_prefix("skin-select:")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|i| *i < skins::SKINS.len())
        {
            // 选皮肤只切换整套预览，不丢弃已经选好的自定义图片。
            STATE.with(|state| state.borrow_mut().skin = i);
            render();
            return String::new();
        }
        if let Some(i) = id
            .strip_prefix("fruit-select:")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|i| *i < 11)
        {
            STATE.with(|state| {
                let mut s = state.borrow_mut();
                if s.level == i {
                    return;
                }
                // 自定义图片按水果尺寸转换，换了水果只能作废。
                if s.pending.take().is_some() {
                    s.message = format!(
                        "已切换到「{}」，之前为「{}」选的图片已取消",
                        images::NAMES[i],
                        images::NAMES[s.level]
                    );
                }
                s.level = i;
            });
            render();
            continue_read().await;
            return String::new();
        }
        match id.as_str() {
            "connect" => connect(true).await,
            "skin-apply" => {
                if STATE.with(|state| custom_count(&state.borrow())) > 0 {
                    STATE.with(|state| state.borrow_mut().confirm = Some(Confirm::ApplySkin));
                    render();
                } else {
                    apply_skin(false).await;
                }
            }
            "reset-all" => {
                STATE.with(|state| state.borrow_mut().confirm = Some(Confirm::ResetAll));
                render();
            }
            "confirm-yes" => match confirm {
                Some(Confirm::ApplySkin) => apply_skin(false).await,
                Some(Confirm::ResetAll) => apply_skin(true).await,
                None => render(),
            },
            "confirm-no" => render(),
            "account-check" => queue_verify("check"),
            "image-file" => queue_pick(),
            "image-apply" => apply_custom().await,
            "image-reset" => reset_current().await,
            "image-view" => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    let level = s.level;
                    s.read_failed[level] = false;
                    s.current[level] = None;
                });
                continue_read().await;
            }
            "image-discard" => {
                STATE.with(|state| state.borrow_mut().pending = None);
                render();
            }
            _ => {}
        }
        String::new()
    }
}
export!(Watermelon);

#[cfg(test)]
mod tests {
    #[test]
    fn timer_body_unwraps_the_host_payload_field() {
        // 宿主送来的 timer 事件：标签在 payload 字段里。
        assert_eq!(
            super::timer_body(r#"{"id":3,"payload":"verify:check"}"#),
            "verify:check"
        );
        // 不是 JSON（或没有 payload 字段）就按原文。
        assert_eq!(super::timer_body("probe:abc"), "probe:abc");
        assert_eq!(super::timer_body(r#"{"value":"x"}"#), r#"{"value":"x"}"#);
    }
}
