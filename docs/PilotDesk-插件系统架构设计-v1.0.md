# PilotDesk 插件系统架构设计 v1.0

> 更新时间：2026-06-17
> 状态：已实现

---

## 1. 设计目标

- **可扩展**：第三方开发者可以独立开发插件，无需修改核心代码
- **安全**：插件运行在沙箱环境中，不能直接访问系统资源
- **轻量**：插件加载和卸载对主应用性能影响最小化
- **统一**：所有插件遵循相同的 API 规范

## 2. 架构总览

```
┌──────────────────────────────────────────────────────────┐
│                    PilotDesk Core                         │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │  PluginHost   │  │  PluginAPI   │  │  PluginAPI   │   │
│  │  (Rust)       │  │  (运行时)    │  │  (事件总线)  │   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘   │
│         │                 │                  │           │
│  ┌──────┴─────────────────┴──────────────────┴───────┐   │
│  │              Zustand PluginStore                    │   │
│  │  plugins[] + registeredPanels/Commands/Hooks       │   │
│  └──────────────────────┬────────────────────────────┘   │
│                         │                                │
│  ┌──────────────────────┴────────────────────────────┐   │
│  │              PluginRegistry (组件注册表)           │   │
│  │  面板组件注册 + 加载状态 + JS 执行 + 生命周期     │   │
│  └───────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

## 3. 插件生命周期

```
发现 (Discovery) → 加载 (Load) → 执行 JS → 运行 (Run) → 卸载 (Unload)
```

### 3.1 发现
- 扫描 `~/.pilotdesk/plugins/` 目录
- 每个插件是一个独立目录，包含 `manifest.json` + 代码文件
- 支持通过管理面板的「+ 安装」按钮上传 .zip 压缩包安装

### 3.2 manifest.json 规范

```json
{
  "id": "hello-world",
  "name": "Hello World",
  "version": "1.0.0",
  "description": "示例插件",
  "author": "PilotDesk Team",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel", "ui:toast", "session:read"],
  "entry": {
    "main": "index.js",
    "styles": "styles.css"
  },
  "contributes": {
    "panels": [
      { "id": "hello-panel", "title": "Hello World", "icon": "https://example.com/icon.png" }
    ],
    "commands": [
      { "id": "hello.say", "title": "Say Hello" }
    ],
    "hooks": [
      { "event": "message:before-send", "handler": "onBeforeSend" }
    ]
  }
}
```

### 3.3 icon 字段说明

`contributes.panels[].icon` 字段支持三种格式：

| 格式 | 示例 | 说明 |
|------|------|------|
| 网络图片 | `"https://example.com/icon.png"` | 直接渲染为 `<img>` 标签 |
| 插件本地路径 | `"image/favicon.png"` | 相对于插件目录，通过 Tauri `convertFileSrc()` 转换 |
| 空/未定义 | 省略或 `""` | 显示默认图标 `📦` |

**注意**：图标以 14px 尺寸渲染，加载失败时自动回退到默认图标。

## 4. Plugin API

### 4.1 前端 API (TypeScript)

```typescript
interface PluginAPI {
  // UI 能力
  ui: {
    addPanel(config: PanelContribution & { component: React.ComponentType }): void;
    removePanel(id: string): void;
    showToast(message: string, type: 'info' | 'success' | 'error'): void;
  };

  // 数据访问
  data: {
    invoke<T>(cmd: string, params?: Record<string, unknown>): Promise<T>;
  };

  // 事件
  events: {
    on(event: string, handler: (...args: any[]) => void): () => void;
    emit(event: string, ...args: any[]): void;
  };

  // 存储
  storage: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
}
```

### 4.2 插件入口（纯 JS 格式）

插件入口文件使用纯 JavaScript 编写，通过 `React.createElement` 构建 UI，无需 JSX 转译。

```javascript
// index.js — 插件入口文件（纯 JS 格式）
function MyPanel() {
  return React.createElement('div', null,
    React.createElement('h3', null, 'Hello from Plugin!')
  );
}

export default {
  onLoad: function(api) {
    api.ui.addPanel({
      id: 'my-panel',
      title: 'My Plugin',
      component: MyPanel,
    });
  },
  onUnload: function() {
    // 清理逻辑
  },
};
```

## 5. 安全模型

| 权限 | 说明 | 风险等级 | 默认 |
|------|------|---------|------|
| `ui:panel` | 添加/移除面板 | 低 | 需声明 |
| `ui:toast` | 显示通知 | 低 | 默认授权 |
| `ui:modal` | 打开模态框 | 低 | 需声明 |
| `session:read` | 读取会话和消息 | 中 | 需声明 |
| `session:write` | 创建/修改/删除会话 | 中 | 需声明 |
| `data:invoke` | 调用 Tauri 命令 | **高** | 需声明 |
| `storage:*` | 插件独立存储 | 低 | 默认授权 |
| `fs:read` | 读取文件系统 | **高** | 需声明 |
| `fs:write` | 写入文件系统 | **高** | 需声明 |

### 沙箱规则

1. **清单验证**：manifest.json 大小限制 64KB，字段格式严格校验
2. **路径保护**：所有文件路径禁止包含 `..`，防止目录遍历攻击
3. **权限白名单**：未知权限自动拒绝，高风险权限标记警告
4. **入口验证**：入口文件必须存在，路径必须在插件目录内
5. **沙箱禁用时**：所有权限检查跳过，插件可正常加载

## 6. 数据流

### 6.1 面板注册数据流

```
manifest.json (contributes.panels 静态声明)
  → Rust PluginHost 解析
  → Zustand PluginStore.refreshRegistrations()
    → registeredPanels Map → RightPanel 下拉菜单
    → registeredCommands Map → (预留)
    → registeredHooks Map → (预留)

index.js (运行时注册)
  → PluginRegistry.loadPlugin()
    → Rust: plugin_read_entry 读取文件
    → new Function('React', source) 执行
    → 调用 onLoad(api)
      → api.ui.addPanel() 注册真实 React 组件
      → api.ui.showToast() 显示通知
      → api.events.on() 监听事件
      → api.storage.set/get() 存储数据
```

### 6.2 图标渲染数据流

```
manifest.json contributes.panels[].icon
  → pluginStore.buildRegistrations()
    → registeredPanels Map (contribution 原样保留)
  → PluginPanelRenderer / RightPanel
    → PluginIcon 组件
      → parsePluginIcon(icon, pluginPath)
        → 网络地址: 直接返回 URL
        → 本地路径: 拼接插件目录 + convertFileSrc()
      → 渲染 <img> 标签 (14px)
      → 加载失败 → 回退默认图标 📦
```

## 7. 文件清单

| 文件 | 说明 |
|------|------|
| `src-tauri/src/plugin/mod.rs` | Rust PluginHost（扫描、验证、沙箱、Tauri 命令） |
| `src/plugin/PluginAPI.ts` | 插件运行时 API 实现 |
| `src/plugin/PluginRegistry.ts` | 面板组件注册表 + JS 执行 + 生命周期 |
| `src/stores/pluginStore.ts` | Zustand store（插件列表 + 注册数据） |
| `src/components/plugin/PluginManager.tsx` | 插件管理 UI |
| `src/components/plugin/PluginPanelRenderer.tsx` | 插件面板渲染器 |
| `src/components/plugin/PluginIcon.tsx` | 插件图标渲染组件 |
| `src/components/plugin/DefaultPluginPanel.tsx` | 默认面板组件 |
| `src/utils/pluginIcon.ts` | 插件图标解析工具 |
| `src/types/plugin.ts` | 类型定义 |
| `examples/plugins/hello-world/` | 示例插件 |
| `examples/plugins/malicious-sample/` | 恶意插件示例（沙箱测试） |
