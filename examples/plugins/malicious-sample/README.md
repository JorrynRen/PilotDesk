# Malicious Sample 插件

**警告：此插件故意违反所有沙箱规则，仅用于验证防护效果。**

## 违规项

| 规则 | 违规方式 |
|------|---------|
| 路径遍历 | id 为 `../../malicious`，entry.main 指向 `../../etc/passwd` |
| 名称超长 | name 超过 64 字符限制 |
| 版本无效 | version 为 `bad`，非 semver 格式 |
| 未知权限 | 包含 `unknown:permission` 等未注册权限 |
| 高风险权限 | 声明 `fs:read` 和 `fs:write` |
| 入口越界 | entry.main 指向插件目录外的文件 |
| 图标路径遍历 | icon 指向 `../../../windows/system32/drivers/etc/hosts` |
| 样式文件越界(已移除) | 原 entry.styles 指向 `../secret.css`，该字段已从架构中移除 |

## 预期行为

违规项分**两个阶段**拦截。注意：**这个样本本身是装不上的**，这是设计意图，不是故障。

### 阶段一：安装期（`plugin_install_zip`，与沙箱开关无关）

`id` 含 `..` 会在安装时被直接拒绝：

```
插件 ID 包含非法字符: ../../malicious（id 不允许包含“..”、正斜杠或反斜杠）
```

原因：`id` 会参与目标目录拼接（`plugins/<id>/`），放行等于把落盘位置交给插件。
同一阶段的 zip 解包也做了条目级 `..` 过滤与解压体积上限。

→ **本样本到不了阶段二。** 想观察阶段二的拦截，先把 `manifest.json` 的 `id` 改成合法值
（例如 `malicious-sample`），再重新打包安装。

### 阶段二：加载期（`plugin_discover` → `load_and_validate_plugin`）

id 合法、能装上之后，下面这些才会被检查：

- `name` 超过 64 字符
- `version` 非法（`bad` 不是 semver）
- `unknown:permission` 未注册
- `entry.main` 越过插件目录（`../../etc/passwd`）
- `icon` 路径遍历

沙箱开关在这一阶段的作用：

- **沙箱启用**（默认）：清单字段校验 + 权限检查都会执行 → 插件被拒绝加载，并在插件列表里标记错误
- **沙箱禁用**：上面这两步**会被跳过** → 插件可以加载

### 与沙箱开关无关的部分：命令层权限

命令层的权限校验（`PluginHost::require_permission`，见 `plugin_fs.rs` / `shell.rs` / `mod.rs`）
**与沙箱开关解耦**：`fs:read` / `fs:write` / `shell:exec` / `data:invoke` / `session:execute`
即使关掉沙箱，也**必须**在 manifest 里显式声明才允许调用，不再"关沙箱即全放行"。

本样本声明了 `fs:read` / `fs:write`，所以它若真能装上，调用这两个能力时会走上面这套真实判定 ——
但 `get_api_key`、`write_text_file`、`plugin_install_zip`、`terminal_*` 等
**永远不在插件可达范围内**（`data:invoke` 走白名单，见 `PLUGIN_DATA_INVOKE_ALLOWLIST`）。
