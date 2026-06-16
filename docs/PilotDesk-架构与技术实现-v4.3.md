# PilotDesk 架构与技术实现 v4.3

> 更新时间：2026-06-16
> 版本：v4.3
> 变更：插件系统运行时实现、自定义主题色 SettingsPage 集成

---

## 1. 插件系统运行时

### 1.1 Rust PluginHost

**文件：** `src-tauri/src/plugin/mod.rs`

```rust
pub struct PluginHost {
    plugins_dir: PathBuf,       // ~/.pilotdesk/plugins/
    plugins: HashMap<String, PluginInstance>,
}
```

**生命周期：**
```
discover() → 扫描 plugins/ 目录 → 读取 manifest.json → 注册到 HashMap
  → list_plugins() → 返回所有插件
  → enable_plugin(id) → 启用
  → disable_plugin(id) → 禁用
```

**Tauri 命令：**
| 命令 | 参数 | 返回 |
|------|------|------|
| `plugin_discover` | 无 | `Vec<PluginInstance>` |
| `plugin_list` | 无 | `Vec<PluginInstance>` |
| `plugin_enable` | `id: String` | `()` |
| `plugin_disable` | `id: String` | `()` |

### 1.2 前端 PluginStore

**文件：** `src/stores/pluginStore.ts`

Zustand store，封装 `discover`、`list`、`enable`、`disable` 四个 action，统一调用 Rust 后端命令。

### 1.3 前端 PluginManager UI

**文件：** `src/components/plugin/PluginManager.tsx`

- 自动扫描插件列表
- 每个插件显示：名称、版本、描述、作者、权限标签
- 启用/禁用切换按钮
- 空状态提示（引导用户放置插件到 `~/.pilotdesk/plugins/`）
- 刷新按钮

### 1.4 RightPanel 集成

在右侧面板新增"插件"标签页，与灵感、技能、记忆并列。

---

## 2. 自定义主题色

### 2.1 themeStore

**文件：** `src/stores/themeStore.ts`

```typescript
interface ThemeColors {
  accent: string;       // 主色
  accentHover: string;  // 悬停色（自动变暗 20）
  accentLight: string;  // 浅色（15% 透明度）
}
```

- 9 种预设颜色 + 自定义颜色选择器
- 持久化到 SQLite `app_settings` 表（`theme_colors` key）
- 通过 CSS 变量 `--accent`、`--accent-hover`、`--accent-light` 即时生效
- 重置功能恢复默认蓝色主题

### 2.2 ThemeCustomizer 组件

**文件：** `src/components/settings/ThemeCustomizer.tsx`

- 预设色块网格
- 自定义颜色 input[type="color"]
- 重置按钮

### 2.3 SettingsPage 集成

在"通用设置 → 主题设置"区域下方嵌入 ThemeCustomizer。

---

## 3. 版本演进

| 版本 | 核心变更 | 日期 |
|------|---------|------|
| v1.0 | 初始架构：Node.js Sidecar + WebSocket | - |
| v2.0 | AgentType 枚举 | - |
| v3.0 | Zustand + ID-based 去重 | - |
| v3.5 | DPAPI 加密 + 结构化日志 | - |
| v4.0 | 消除 Sidecar + r2d2 + useReducer | 2026-06-16 |
| v4.1 | 虚拟滚动 + 消息编辑 + 设计令牌系统 | 2026-06-16 |
| v4.2 | 消息重发 + 批量操作 + 消息搜索 + 插件架构 + i18n + 自定义主题色 | 2026-06-16 |
| v4.3 | 插件系统运行时 + 自定义主题色集成 | 2026-06-16 |

---

## 4. 待实施

| 项目 | 状态 | 说明 |
|------|------|------|
| i18n 组件集成 | 暂缓 | 基础设施已完成，组件硬编码文本待替换 |
| Function Calling + 联网搜索 | 暂缓 | 技术方案 v1.0 已完成 |
| 插件安全沙箱 | 待实施 | 权限校验、进程隔离 |
| 插件示例 | 待实施 | 示例插件 + 开发文档 |

---

*本文档对应代码版本：PilotDesk v0.1.0*
