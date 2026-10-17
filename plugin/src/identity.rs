use crate::{account, images, net};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};
use waki::bindings::wasi::http::types::Method;

pub(crate) const PURCHASE_URL: &str = "https://afdian.com/item/3187fe28c08a11f1805c52540025c377";
/// 读取宿主账号要弹授权，宿主每次还会实时查询绑定（官方口径单项最多 10 秒）。
/// 宿主卡住时不能把界面永远锁在「验证中…」，到点就收场。
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
/// 整次购买验证请求的截止时间：连上后服务端不回包也不能一直等。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

fn origin() -> &'static str {
    option_env!("KOVELA_ORIGIN")
        .filter(|v| !v.is_empty())
        .unwrap_or("https://kovela.komoridevs.icu")
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entitlement {
    product_id: String,
    afdian_user_id: String,
    entitled: bool,
}

/// 通用截止时间：到点仍未完成就返回 `None`。
///
/// 不能靠另排一个 timer 事件兜底：宿主按插件串行投递事件，宿主调用卡住时排在
/// 后面的 timer 同样进不来。这里在同一个任务里让出执行权轮询单调时钟，每轮
/// yield 都把控制权交回宿主；丢弃未完成的 future 时 wit-bindgen 会取消对应的
/// 宿主子任务。
pub(crate) async fn with_deadline<F: Future>(work: F, timeout: Duration) -> Option<F::Output> {
    let deadline = async move {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            wit_bindgen::yield_async().await;
        }
    };
    futures_util::pin_mut!(work, deadline);
    match futures_util::future::select(work, deadline).await {
        futures_util::future::Either::Left((value, _)) => Some(value),
        futures_util::future::Either::Right(_) => None,
    }
}

async fn afdian_user_id() -> Result<String, String> {
    let profile = account::get_current()
        .await?
        .ok_or("请先登录 AstroBox，并在账号设置中绑定爱发电账号")?;
    let binding = profile
        .bindings
        .into_iter()
        .find(|binding| binding.provider == "afdian")
        .ok_or("请先在 AstroBox 账号设置中绑定爱发电账号")?;
    let id = match binding.status {
        account::BindingStatus::Linked => binding
            .id
            .ok_or("无法读取爱发电账号，请在 AstroBox 中重新绑定后重试")?,
        account::BindingStatus::Unlinked => {
            return Err("请在 AstroBox 账号设置中绑定购买时使用的爱发电账号".into());
        }
        account::BindingStatus::Unavailable => {
            return Err("暂时无法读取爱发电账号，请检查网络后重试".into());
        }
    };
    if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("无法读取爱发电账号，请在 AstroBox 中重新绑定后重试".into());
    }
    Ok(id.to_ascii_lowercase())
}

pub(crate) async fn check() -> Result<(String, bool), String> {
    let id = with_deadline(afdian_user_id(), ACCOUNT_TIMEOUT)
        .await
        .unwrap_or_else(
            || Err("读取 AstroBox 账号超时，请确认已允许授权并检查网络后重试".into()),
        )?;
    let body =
        serde_json::to_vec(&json!({"afdianUserId":id})).map_err(|_| "验证失败，请稍后重试")?;
    let reply = net::send(
        Method::Post,
        origin(),
        "/api/watermelon/entitlement",
        Some(&body),
        CONNECT_TIMEOUT,
        REQUEST_TIMEOUT,
    )
    .await
    .map_err(|error| match error {
        net::NetError::Connect => "无法连接验证服务器，请检查网络后重试",
        net::NetError::Timeout => "验证超时，请检查网络后重试",
        net::NetError::Body => "验证失败，请稍后重试",
    })?;
    let (status, bytes) = (reply.status, reply.body);
    if bytes.len() > 4096 {
        return Err("验证失败，请稍后重试".into());
    }
    if !(200..300).contains(&status) {
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        return Err(value
            .pointer("/error/message")
            .and_then(|value| value.as_str())
            .filter(|v| !v.is_empty() && v.len() < 512)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("验证服务暂时不可用，请稍后重试（错误码 {status}）")));
    }
    let value: Entitlement = serde_json::from_slice(&bytes).map_err(|_| "验证失败，请稍后重试")?;
    if value.product_id != images::PRODUCT || value.afdian_user_id != id {
        return Err("验证失败，请稍后重试".into());
    }
    Ok((id, value.entitled))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// 测试用的最小执行器：反复轮询直到完成（deadline 靠 yield 自唤醒推进）。
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(value) = Future::poll(future.as_mut(), &mut context) {
                return value;
            }
        }
    }

    #[test]
    fn deadline_unlocks_a_hung_host_call_and_passes_results_through() {
        // 宿主账号接口永远不返回：截止时间一到就收场，界面不会锁死在「验证中…」。
        let started = std::time::Instant::now();
        let hung = block_on(super::with_deadline(
            std::future::pending::<u8>(),
            Duration::from_millis(30),
        ));
        assert_eq!(hung, None);
        assert!(started.elapsed() >= Duration::from_millis(30));

        // 正常返回的结果原样透传，不等截止时间。
        let started = std::time::Instant::now();
        let value = block_on(super::with_deadline(async { 7 }, Duration::from_secs(20)));
        assert_eq!(value, Some(7));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
