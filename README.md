# PilotDesk
PilotDesk 是一个 **Agent 统一桌面客户端**，将多个 AI Agent（Claude Code、Hermes Agent、CodeX CLI等）集成到单一桌面应用中，提供统一的会话管理、消息交互体验及LLM API直连会话（支持上下文）。同时，基于此AI能力提供工作流任务管理和其他用户自定义拓展能力。

## 发布流程

正式发布渠道为 GitHub 仓库 [JorrynRen/PilotDesk](https://github.com/JorrynRen/PilotDesk)，通过 GitHub Releases 分发，并支持应用内自动更新。

### 一、发版要改哪几处（版本号）

版本号目前需要 **人工同步以下 3 处**，缺一不可（前端显示与更新判断都依赖它们一致）：

| 文件 | 字段 | 说明 |
| --- | --- | --- |
| `package.json` | `version` | 前端唯一版本来源 |
| `src-tauri/tauri.conf.json` | `version` | 打包版本号，同时决定 `tauri-action` 生成的 tag（`v__VERSION__`） |
| `src-tauri/Cargo.toml` | `[package] version` | Rust 侧编译期版本（`env!("CARGO_PKG_VERSION")`） |

> 前端 UI 中显示的版本号（如状态栏 `PilotDesk vX.Y.Z`）**无需手改**：`vite.config.ts` 会在构建时读取 `package.json` 的 `version`，
> 注入为 `import.meta.env.VITE_APP_VERSION`，各组件统一从该处读取。

发版步骤：

1. 同步上述 3 处版本号（例如统一改为 `0.2.0`）。
2. 提交并推送到 `main`，确认 CI 绿灯。
3. 打 tag 并推送：`git tag v0.2.0 && git push origin v0.2.0`。
4. `.github/workflows/release.yml` 自动触发：先跑 `ci.yml` 作为门禁，再在三平台矩阵上构建并创建 Release
   （Windows NSIS、macOS app/dmg、Linux AppImage/deb），同时上传 updater 清单 `latest.json`。
5. 待 Release 发布完成后，应用内「设置 → 更新检查」即可检测到新版本并一键下载安装。

也可在 Actions 页面手动触发 `Release` 工作流（`workflow_dispatch`）。

### 二、必须在 GitHub Secrets 中配置的项

路径：仓库 Settings → Secrets and variables → Actions。

**updater 签名（三平台共用，缺失则构建不出自动更新包）**

- `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
- 生成：`npm run tauri signer generate -- -w "$HOME/.tauri/pilotdesk.key"`
  - 私钥全文 → `TAURI_SIGNING_PRIVATE_KEY`（密码 → `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`）
  - 公钥 → 填入 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`（当前为占位符，必须替换）
  - ⚠️ 私钥请离线备份：一旦丢失，已发布版本将无法再接收更新。

**macOS 代码签名 + 公证**

- `APPLE_CERTIFICATE`：Developer ID Application 证书导出的 `.p12` 的 base64
- `APPLE_CERTIFICATE_PASSWORD`：导出 `.p12` 时设置的密码
- `APPLE_SIGNING_IDENTITY`：形如 `Developer ID Application: Your Name (TEAMID)`
- `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`：Apple ID、App 专用密码、10 位 Team ID

**Windows 代码签名（可选）**

- 证书在构建机证书库：`TAURI_WINDOWS_SIGNTOOL_THUMBPRINT`
- Azure Trusted Signing：`AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` /
  `AZURE_CODE_SIGNING_ACCOUNT_NAME` / `AZURE_CERTIFICATE_PROFILE_NAME` / `AZURE_CODE_SIGNING_ENDPOINT`
- 注意：Windows 代码签名（Authenticode）与上面的 updater 签名是两回事，前者用于消除 SmartScreen 提示，后者用于更新包校验。

各 Secret 的详细获取步骤见 `.github/workflows/release.yml` 顶部注释。

### 三、图标

- Windows：`src-tauri/icons/icon.ico`
- macOS：`src-tauri/icons/icon.icns`
- 需要重新生成全部平台图标时（建议使用 1024×1024 带透明通道的方形源图）：
  `npx tauri icon path/to/app-icon.png`

