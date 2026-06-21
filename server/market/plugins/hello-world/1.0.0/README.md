# Hello World 示例插件

PilotDesk 插件系统的示例插件，展示插件开发的基本模式。

## 目录结构

```
hello-world/
├── manifest.json    # 插件清单（必填）
├── index.js        # 插件入口（必填，纯 JS 格式）
```

## manifest.json 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | 是 | 插件唯一标识，不能包含路径分隔符 |
| `name` | string | 是 | 插件显示名称，最长 64 字符 |
| `version` | string | 是 | semver 版本号（如 1.0.0） |
| `description` | string | 是 | 插件描述 |
| `author` | string | 是 | 作者名称 |
| `minAppVersion` | string | 是 | 最低兼容的 PilotDesk 版本 |
| `permissions` | string[] | 是 | 所需权限列表 |
| `entry.main` | string | 是 | 入口文件路径（相对于插件目录） |
| `contributes` | object | 否 | 贡献点声明（面板/命令/钩子） |

### contributes 字段

```json
{
  "contributes": {
    "panels": [
      { "id": "panel-id", "title": "面板标题", "icon": "https://example.com/icon.png" }
    ],
    "commands": [
      { "id": "cmd.id", "title": "命令标题" }
    ],
    "hooks": [
      { "event": "事件名", "handler": "处理函数名" }
    ]
  }
}
```

`contributes` 用于**静态声明**插件提供的扩展能力：
- **panels**：在右侧栏下拉菜单中显示面板入口
- **commands**：注册可执行命令（预留）
- **hooks**：注册事件钩子（预留）

### icon 字段说明

`contributes.panels[].icon` 字段支持三种格式：

| 格式 | 示例 | 说明 |
|------|------|------|
| 网络图片 | `"https://example.com/icon.png"` | 直接渲染为 `<img>` 标签 |
| 插件本地路径 | `"image/favicon.png"` | 相对于插件目录，通过 Tauri `convertFileSrc()` 转换 |
| 空/未定义 | 省略或 `""` | 显示默认图标 `📦` |

图标以 14px 尺寸渲染，加载失败时自动回退到默认图标。

## 权限系统

### 合法权限列表

| 权限 | 说明 | 风险等级 |
|------|------|---------|
| `ui:panel` | 添加/移除面板 | 低 |
| `ui:toast` | 显示通知（默认授权） | 低 |
| `ui:modal` | 打开模态框 | 低 |
| `session:read` | 读取会话和消息 | 中 |
| `session:write` | 创建/修改/删除会话 | 中 |
| `data:invoke` | 调用 Tauri 命令 | **高** |
| `storage:*` | 插件独立存储（默认授权） | 低 |
| `fs:read` | 读取文件系统 | **高** |
| `fs:write` | 写入文件系统 | **高** |

### 沙箱规则

1. **清单验证**：manifest.json 大小限制 64KB，字段格式严格校验
2. **路径保护**：所有文件路径禁止包含 `..`，防止目录遍历攻击
3. **权限校验**：未知权限自动拒绝，高风险权限标记警告
4. **入口验证**：入口文件必须存在，路径必须在插件目录内
5. **沙箱禁用时**：所有权限检查跳过，插件可正常加载

## 开发指南

### 1. 创建插件目录

```bash
mkdir -p ~/.pilotdesk/plugins/my-plugin
```

### 2. 编写 manifest.json

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "description": "我的插件",
  "author": "Me",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel", "ui:toast"],
  "entry": {
    "main": "index.js"
  },
  "contributes": {
    "panels": [
      { "id": "my-panel", "title": "My Panel", "icon": "https://example.com/icon.png" }
    ]
  }
}
```

### 3. 编写入口文件

插件入口文件使用纯 JavaScript 编写，通过 `React.createElement` 构建 UI：

```javascript
function MyPanel() {
  return React.createElement('div', { className: 'my-panel' },
    React.createElement('h3', null, 'Hello from Plugin!')
  );
}

export default {
  onLoad: function(api) {
    // 注册面板（覆盖 contributes 中的默认面板）
    api.ui.addPanel({
      id: 'my-panel',
      title: 'My Panel',
      component: MyPanel,
    });
    api.ui.showToast('插件已加载', 'success');
  },
  onUnload: function() {
    // 清理逻辑
  },
};
```

> **注意**：入口文件必须使用纯 JS 格式（`React.createElement` 替代 JSX），不支持 JSX 语法。

### 4. 安装插件

将插件目录复制到 `~/.pilotdesk/plugins/` 目录下，然后在 PilotDesk 的插件管理面板中点击"刷新"即可发现。

## Plugin API 参考

### ui

```typescript
api.ui.addPanel(config)     // 注册面板（覆盖默认面板）
api.ui.removePanel(id)      // 移除面板
api.ui.showToast(msg, type) // 显示通知
```

### data

```typescript
api.data.invoke(cmd, params) // 调用 Tauri 命令
```

### events

```typescript
api.events.on(event, handler)  // 监听事件（返回取消函数）
api.events.emit(event, ...args) // 触发事件
```

### storage

```typescript
api.storage.get(key)      // 读取
api.storage.set(key, val) // 写入
api.storage.delete(key)   // 删除
```

## 生命周期

```
插件发现 → PluginRegistry.loadPlugin()
  → 1. 注册默认面板组件（来自 manifest.json contributes）
  → 2. 读取并执行 index.js
  → 3. 调用 onLoad(api)
    → api.ui.addPanel() 注册真实组件（覆盖默认面板）
  → 4. 插件进入运行状态

插件卸载 → PluginRegistry.unloadPlugin()
  → 1. 调用 onUnload()
  → 2. 清理 API 资源（事件监听、存储）
  → 3. 注销面板组件
```
