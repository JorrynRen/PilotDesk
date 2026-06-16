# PilotDesk 插件系统架构设计 v1.0

> 更新时间：2026-06-16
> 状态：设计阶段

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
│  │  PluginHost   │  │  PluginAPI   │  │  EventBus    │   │
│  │  (Rust)       │  │  (TypeScript)│  │  (跨层通信)  │   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘   │
│         │                 │                  │           │
│  ┌──────┴─────────────────┴──────────────────┴───────┐   │
│  │                 Plugin Registry                    │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐           │   │
│  │  │Plugin A │  │Plugin B │  │Plugin C │  ...      │   │
│  │  └─────────┘  └─────────┘  └─────────┘           │   │
│  └───────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

## 3. 插件生命周期

```
发现 (Discovery) → 加载 (Load) → 初始化 (Init) → 运行 (Run) → 卸载 (Unload)
```

### 3.1 发现
- 扫描 `~/.pilotdesk/plugins/` 目录
- 每个插件是一个独立目录，包含 `manifest.json` + 代码文件
- 支持 npm 包安装 (`npm install pilotdesk-plugin-xxx`)

### 3.2 manifest.json 规范

```json
{
  "id": "plugin-hello-world",
  "name": "Hello World",
  "version": "1.0.0",
  "description": "示例插件",
  "author": "PilotDesk",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel", "session:read"],
  "entry": {
    "main": "index.tsx",
    "styles": "styles.css"
  },
  "contributes": {
    "panels": [
      { "id": "hello-panel", "title": "Hello", "icon": "Smile" }
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

## 4. Plugin API

### 4.1 前端 API (TypeScript)

```typescript
// 插件可用的核心 API
interface PluginAPI {
  // UI 能力
  ui: {
    addPanel(config: PanelConfig): void;
    removePanel(id: string): void;
    showToast(message: string, type: 'info' | 'success' | 'error'): void;
    openModal(content: React.ReactNode): void;
  };

  // 数据访问
  data: {
    getSessions(): Promise<Session[]>;
    getMessages(sessionId: string): Promise<Message[]>;
    getInspirations(): Promise<Inspiration[]>;
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

### 4.2 插件入口

```typescript
// index.tsx — 插件入口文件
export default {
  onLoad(api: PluginAPI) {
    // 初始化逻辑
    api.ui.addPanel({
      id: 'my-panel',
      title: 'My Plugin',
      component: MyPanelComponent,
    });
  },

  onUnload() {
    // 清理逻辑
  },
};
```

## 5. 安全模型

| 权限 | 说明 | 默认 |
|------|------|------|
| `ui:panel` | 添加/移除面板 | 需声明 |
| `ui:toast` | 显示通知 | 默认 |
| `session:read` | 读取会话 | 需声明 |
| `session:write` | 修改会话 | 需声明 |
| `data:invoke` | 调用 Tauri 命令 | 需声明 |
| `storage:*` | 插件独立存储 | 默认 |

## 6. 实现计划

| Phase | 内容 | 预估工时 |
|-------|------|---------|
| P1 | PluginHost Rust 端（扫描 manifest、加载、生命周期管理） | 8h |
| P2 | PluginAPI TypeScript 端（UI API、数据 API、事件 API） | 6h |
| P3 | Plugin Registry UI（管理面板、启用/禁用） | 4h |
| P4 | 示例插件 + 文档 | 3h |
| P5 | 安全沙箱 + 权限校验 | 4h |
