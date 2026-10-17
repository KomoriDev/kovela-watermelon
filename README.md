# Kovela Watermelon

合成大西瓜独立换肤工具。AstroBox 插件仅使用 API Level 4 / WASI Preview 3；手环游戏、免费皮肤和图片传输无需购买。自定义图片导入是插件内付费功能。

## 使用

安装 `plugin/dist/watermelon/Kovela-Watermelon.abp`，并将手环上的合成大西瓜更新到 1.2.0。在插件里选择手环并点击“连接”，插件会自动在手环上打开换肤页（若未打开，手动进入手环“设置 > 水果换肤”后重试）。在“整套皮肤”里选择经典水果或 Q版水果并点击“应用”，免费替换全部 11 种水果；覆盖已换肤水果前会先确认。在“单个水果”里点选水果，可为它选择自定义图片或恢复默认。进度与结果显示在页面顶部。

导入自定义图片时，先登录 AstroBox，在账号设置中绑定购买商品的爱发电账号。插件从 V4 `account.get-current` 获取 `linked` 的爱发电 ID，向 Kovela 查询权益；本次会话首次选图前验证一次，通过后选图与应用到手环都不再重复验证；宿主没有账号切换事件，会话内切换 AstroBox 账号后可点“验证购买”或重新打开插件刷新权益。未登录、未绑定、服务器无法确认绑定或权益时会显示具体原因，免费皮肤仍可使用。

商品保持 `com.komoridev.watermelon`，爱发电 item 保持 `3187fe28c08a11f1805c52540025c377`：<https://afdian.com/item/3187fe28c08a11f1805c52540025c377>。旧购买记录继续有效，自定义权益按账号检查，原来的手环设备绑定不影响权益。

## 源码与构建

| 路径 | 作用 |
| --- | --- |
| `plugin/src` | V4 UI、账号权益、本地图片转换、逐包确认传输 |
| `plugin/wit` | 官方 V4 WIT 定义与只导入所需接口的 world |
| `plugin/assets/skins` | 两套完整皮肤，每套 11 张 I8 资源、PNG 预览和原始资源 |
| `wearable/src` | 免费通信与持久图片存储；不依赖 VelaPass 或许可证 |
| `scripts/prepare-vela.mjs` | 为 `../watermelon/src/skin/index.js` 生成独立通信包 |
| `scripts/prepare-skins.mjs` | 从经典 PNG 和 Q版水果 PNG 重建皮肤 |
| `scripts/crop-qfruit.mjs` | 从 `qfruit/source/sheet.png` 素材图切出 11 张 Q版水果原图 |
| `manifest_v2.json`、`downloads`、`media` | 既有游戏分发目录 |
| `index.txt` | AstroBox 插件源入口 `plugin/dist/watermelon` |

需要 Node.js 22+、Python 3 和 Rust stable：

```bash
npm ci
rustup target add wasm32-wasip2
npm run prepare:skins
npm run prepare:vela
npm test
npm run build:plugin
```

`build:plugin` 生成 V4 `.wasm`、manifest、图标和 `.abp`；皮肤编译进 WASM，无须联网下载。`.github/workflows/plugin.yml` 在 CI 构建并上传产物。游戏在相邻 `watermelon` 仓库构建，`npm run build` / `npm run release` 会先生成免费的换肤通信代码。

游戏执行 `npm run release` 后，在本仓库运行 `npm run stage:game`，将 1.2.0 RPK 复制到既有 16 个设备条目并更新 `manifest_v2.json`。该步骤只整理本地产物，不执行推送或部署。

## 传输

沿用 JSON v1 的 `hello`、`image-begin/chunk/commit/reset/read` 协议，手环握手明确返回 `images.freeTransfer=true`。旧固件没有此标记时插件提示更新，避免把旧的授权限制误当作新功能。每块 1536 字节、每张最多 40 KiB，CRC32 校验成功后提交图片及索引；失败保留原图片，重试继续当前水果。整套换肤按水果依次提交，中途结束会保留已经完成的水果。

插件支持 PNG、JPEG、WebP、BMP、GIF 首帧，自动校正方向、居中裁剪、圆形透明遮罩与 I8 量化。输入上限 8 MiB、边长上限 8192、解码内存上限 64 MiB。

手环继续使用 `internal://files/watermelon-images.json` 和 `watermelon-image-<id>-<level>.bin`，已有图片无需迁移。索引加载、预览回读、迟到回调与中断文件回收保留原来的恢复逻辑。

插件不向手环推送任何通知（真机上 AstroBox 推送的通知会让手环整机重启），传输进度、失败原因与完成状态只在插件页显示。

## 付费后端与发布

VelaPass 的 `purchases.buyer_id` 已保存爱发电 ID。`POST /api/watermelon/entitlement` 只接收 `{afdianUserId}`，返回 `{productId, afdianUserId, entitled}`；会核对原商品和爱发电当前支付状态，不签发手环许可证。发布前应用 `apps/worker/migrations/0004_buyer_entitlements.sql` 并部署更新的 Worker，然后分别发布本插件与游戏 1.2.0。默认服务为 <https://kovela.komoridevs.icu>；本地构建可用 `KOVELA_ORIGIN` 覆盖。

身份资料接口不返回 token、Cookie 或服务器可验证的登录证明。此实现按宿主可信绑定资料解锁插件本地功能，权益接口不是 HTTP 调用者身份认证，也不返回可访问服务端付费资源的凭据。若以后把生成或资源下载移到服务器，应增加独立 OAuth 验证。

AstroBox 文档：[V4](https://abox.run/docs/plugin-development/v4)、[身份资料](https://abox.run/docs/plugin-development/v4/host-api/identity)。WIT 来源为 [AstroBox-Plugin-WIT](https://github.com/AstralSightStudios/AstroBox-Plugin-WIT)，同步版本 `f87ffddbc59e2819204664773e4a2a68789638c7`。
