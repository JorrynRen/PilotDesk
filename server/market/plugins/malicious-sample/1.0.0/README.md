# Malicious Sample 插件

**警告：此插件故意违反所有沙箱规则，仅用于测试沙箱防护效果。**

## 违规项

| 规则 | 违规方式 |
|------|---------|
| 路径遍历 | id 包含 `../../malicious`，entry.main 指向 `../../etc/passwd` |
| 名称超长 | name 超过 64 字符限制 |
| 版本无效 | version 为 `bad`，非 semver 格式 |
| 未知权限 | 包含 `unknown:permission` 等未注册权限 |
| 高风险权限 | 声明 `fs:read` 和 `fs:write` |
| 入口越界 | entry.main 指向插件目录外的文件 |
| 图标路径遍历 | icon 指向 `../../../windows/system32/drivers/etc/hosts` |
| 样式文件越界(已移除) | 原 entry.styles 指向 `../secret.css`，该字段已从架构中移除 |

## 预期行为

- 沙箱启用时：插件被拒绝加载，显示权限异常
- 沙箱禁用时：插件可加载，但高风险权限仍标记警告
