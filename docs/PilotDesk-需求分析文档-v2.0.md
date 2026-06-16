# PilotDesk 需求分析文档 v2.0

> 版本: v2.0 | 作者: 简意工作室 (jorryn) | 日期: 2026-06
> 基于 v1.0 更新，反映截至 2026-06-01 的项目进展与需求变更

---

## 1. 项目概述

### 1.1 项目背景

随着 AI Agent 技术的快速发展，开发者需要一个统一的桌面客户端来管理和使用多种 AI Agent（如 Claude Code、Hermes Agent 等）。现有的 Web 端解决方案存在以下痛点：

- 多个 Agent 平台分散，频繁切换效率低下
- 缺乏本地化的会话管理和灵感沉淀能力
- API 密钥管理不安全，依赖浏览器存储
- 无法与本地开发工作流深度集成
- 缺少 API 直连模式，用户无法绕过 Agent 直接与模型对话

### 1.2 项目目标

构建一个 **Windows 原生桌面应用**，作为多 AI Agent 的统一客户端，提供会话管理、灵感系统、API 直连对话、配置管理等核心功能。

### 1.3 目标用户

| 用户类型 | 使用场景 | 核心需求 |
|---------|---------|---------|
| 独立开发者 | 日常编码辅助 | 快速切换 Agent、管理会话历史 |
| AI 研究者 | 多模型对比测试 | 多 API 配置、API 直连对话、快速切换 |
| 内容创作者 | 灵感管理 + AI 辅助写作 | 灵感收集、从对话中收藏、快速调用 AI |
| 团队技术负责人 | 统一团队 AI 工具 | 标准化 Agent 配置、SQLite 数据持久化 |

---

## 2. 功能需求

### 2.1 核心功能（P0）

#### 2.1.1 多 Agent 会话管理
- 支持创建、删除、**重命名**、归档会话
- 会话列表展示，支持会话间快速切换
- 会话模式切换（支持 **Claude Code / Hermes Agent / API 直连** 三种类型）
- 消息气泡展示（**用户消息靠右、全宽容器内 max-width:80%**，Agent/系统消息靠左）
- 消息内容支持 **Markdown 渲染**（react-markdown + remark-gfm + rehype-highlight）
- 消息列表支持 **虚拟滚动**（react-virtuoso，大量消息时保持性能）
- SSE 流式输出支持（**Anthropic 格式**与 **OpenAI 格式**双协议适配）
- 消息操作按钮 **hover 显示**（编辑、收藏灵感、复制等）

#### 2.1.2 API 直连对话（v2.0 新增）
- 在新建会话时选择 **API 直连模式**
- 选择 API 提供商和模型，直接与模型 API 通信
- 支持 **多轮对话**（自动携带历史消息）
- 用户填写 **完整 API URL**，系统直接调用，不做路径拼接
- 会话标题自动格式化为 `API: {提供商名} - {模型名}`
- 创建前校验 API Key 是否已配置，未配置时提示用户

#### 2.1.3 灵感系统
- **独立灵感面板**：右侧面板中展示所有灵感卡片
- 灵感创建：支持标题、内容、图标选择（24+ 图标）
- 灵感搜索：**本地实时过滤**（不污染全局 store）
- 灵感应用：点击灵感自动填入会话输入框
- 灵感市集：独立页面，浏览和管理灵感集合
- **从消息中收藏灵感**：hover 显示收藏按钮，一键创建灵感并 toast 提示
- 灵感 CRUD 全部通过 **Tauri invoke → Rust → SQLite** 持久化
- FTS5 全文搜索支持

#### 2.1.4 API 配置管理
- 支持多 API 提供商配置（**Anthropic 格式**与 **OpenAI 格式**自动识别）
- 每个配置包含：名称、**API URL**（完整地址）、**API Key**（加密存储于 SQLite）、模型列表
- 配置列表展示，支持新增、编辑、删除
- **拖拽排序**（@dnd-kit/sortable）调整配置优先级
- **测试连接**功能（发送 max_tokens=1 最小请求验证可用性，返回延迟和状态）
- 所有数据通过 **Tauri invoke → Rust rusqlite → SQLite** 持久化，**不再使用 localStorage**

#### 2.1.5 设置系统（5 Tab）
- **通用设置**：深色/浅色/跟随系统 主题切换、**工作区目录**（原生目录浏览选择，存储于 SQLite app_settings 表）
- **环境配置**：检测并显示 Node.js / Git / Python / Claude Code / Hermes 版本，支持一键安装
- **Agent 参数配置**：同时支持 Claude Code 和 Hermes Agent 的参数编辑器（ConfigEditor 组件）
  - Claude Code：URL、Model、Max Tokens、Temperature 等
  - Hermes Agent：对应参数配置
- **API 配置**：列表式管理（上述 2.1.4）
- **关于页面**：品牌 Logo、版本号、技术栈展示、MIT 版权协议

### 2.2 重要功能（P1）

#### 2.2.1 右侧面板
- 灵感面板：展示、搜索、新建灵感
- **技能浏览器**：从 Sidecar WebSocket 加载 Agent 技能列表并展示
- **配置编辑器**：按 Agent 类型加载参数配置
- **记忆浏览器**：查看 Agent 记忆数据
- **Bot 设置**：微信/钉钉 Bot 通道配置
- 面板可折叠/展开

#### 2.2.2 消息编辑
- 支持编辑已发送的用户消息（点击编辑按钮，**填充回输入框**）

#### 2.2.3 WebSocket 通信
- 与 Node.js Sidecar 通过 **WebSocket** 通信（端口 19830）
- 连接状态实时显示（状态栏 WebSocket 指示器）
- 支持 ping/pong 心跳、session:create/close、chat、stop、skills:list 等协议

#### 2.2.4 4 种对话模式
- **原生**（Native）：默认模式，无额外提示
- **快速**（Fast）：简洁回答，直接结论
- **深度思考**（Think）：逐步推理，详细解释思路
- **专家**（Expert）：专业视角，全面深入分析

### 2.3 辅助功能（P2）

- 会话模式下拉**向上展开**，避免遮挡
- **自定义标题栏**：拖动、双击最大化/还原、窗口控制按钮（最小化/最大化/关闭）
- 最大化后图标自动变为**还原图标**
- 整个标题栏可拖动
- **主题系统**：CSS 变量驱动，支持深色/浅色/跟随系统三种模式
- **Toast 通知系统**：操作反馈（收藏灵感、错误提示等）
- 技能系统（内置技能 + 自定义技能）
- MIT License 文件

---

## 3. 非功能需求

### 3.1 性能要求
- 应用启动时间 < 3 秒
- UI 渲染帧率 >= 60fps
- SSE 流式输出延迟 < 500ms
- **消息列表虚拟滚动**：万级消息不卡顿

### 3.2 安全要求
- **API Key 存储于 SQLite 数据库文件**（v2.0 变更：从 localStorage 迁移至 SQLite）
- Key 以 masked 形式在前端展示，原始值仅通过 `get_api_key` invoke 获取
- 不在日志中输出敏感信息
- CSP 安全策略限制资源加载范围
- MIT 开源协议

### 3.3 存储架构（v2.0 重要变更）

| 数据类型 | v1.0 存储方案 | v2.0 存储方案 |
|---------|-------------|-------------|
| API 提供商配置 | localStorage (`pd-api-providers`) | SQLite `api_providers` 表 |
| API Key | localStorage (`pd-api-{id}-key`) | SQLite `api_providers.api_key` 列 |
| 工作区目录 | localStorage (`pilotdesk-workspace`) | SQLite `app_settings` 表 (KV) |
| 会话数据 | SQLite `sessions` + `messages` 表 | 不变 |
| 灵感数据 | SQLite `inspirations` + `inspiration_tags` 表 | 不变 |
| Agent 配置 | Claude/Hermes JSON 配置文件 | 不变 |

### 3.4 兼容性
- 目标平台：Windows 10/11
- 系统要求：64 位，WebView2 运行时
- 开发环境要求：Node.js (F:\soft\nodejs)、Rust stable (C:\Users\Administrator\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin)

---

## 4. 技术选型

| 层级 | 技术栈 | 版本 | 说明 |
|------|--------|------|------|
| 前端 UI | React + TypeScript + TailwindCSS | 19 / 6.0 / v4 | 现代化响应式界面 |
| 状态管理 | Zustand | 5.x | 轻量级状态管理 |
| 桌面框架 | Tauri | 2.x | 轻量级原生桌面应用 |
| 后端 | Rust (rusqlite) | stable | 本地数据持久化 + IPC 处理 |
| 通信层 | Node.js Sidecar (WebSocket) | ws | Agent 通信桥接 |
| 拖拽排序 | @dnd-kit/sortable | 6.x | API 配置拖拽 |
| Markdown | react-markdown + remark-gfm + rehype-highlight | latest | 消息内容渲染 |
| 虚拟列表 | react-virtuoso | 4.x | 大量消息高性能渲染 |
| 图标 | lucide-react | latest | 统一图标体系 |
| 原生对话框 | @tauri-apps/plugin-dialog | 2.x | 目录选择器 |

---

## 5. 数据模型

### 5.1 Session（会话）

```typescript
interface Session {
  id: string;
  agentType: 'claude' | 'hermes' | 'api';  // v2.0: 新增 'api' 类型
  title: string;
  cwd: string;
  createdAt: number;
  updatedAt: number;
  lastMessagePreview: string;
  messageCount: number;
  status: 'active' | 'archived';
  apiProvider?: string;   // API 直连会话的提供商 ID
  apiModel?: string;     // API 直连会话的模型标识
}
```

### 5.2 Message（消息）

```typescript
interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
}
```

### 5.3 ApiProvider（API 提供商，v2.0 新增）

```typescript
interface ApiProvider {
  id: string;
  name: string;
  apiEndpoint: string;    // 完整 API URL
  apiKeyMasked: string;   // 前端展示用（如 "sk-a****b2cd"）
  apiKeySet: boolean;     // 是否已配置 Key
  models: string[];       // 模型列表
  sortOrder: number;      // 排序权重
  createdAt: number;
  updatedAt: number;
}
```

### 5.4 Inspiration（灵感）

```typescript
interface Inspiration {
  id: string;
  icon: string;
  title: string;
  content: string;
  sourceAgent: 'claude' | 'hermes' | 'manual';
  isFavorite: boolean;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}
```

### 5.5 BotChannel（Bot 通道）

```typescript
interface BotChannel {
  id: string;
  agentType: 'claude' | 'hermes';
  platform: string;
  method: string;
  status: string;
  triggerPrefix: string;
  responseFormat: string;
  config: Record<string, unknown>;
  createdAt: number;
  updatedAt: number;
}
```

---

## 6. 项目里程碑

| 阶段 | 内容 | 状态 |
|------|------|------|
| M1 | 基础架构搭建 + UI 框架 + 自定义标题栏 | 已完成 |
| M2 | 会话管理 + 消息系统 + Markdown 渲染 | 已完成 |
| M3 | 灵感系统 + 设置系统 + Agent 配置 | 已完成 |
| M4 | API 配置 + 连接测试 + 拖拽排序 | 已完成 |
| M5 | **API 直连会话**（端到端） | 已完成 |
| M6 | **存储迁移**（localStorage → SQLite） | 已完成 |
| M7 | Sidecar 通信 + Agent 集成测试 | 开发中 |
| M8 | UI 打磨 + 响应式优化 + 打包发布 | 待开始 |

---

## 7. v1.0 → v2.0 变更记录

### 7.1 新增功能

| 功能 | 描述 |
|------|------|
| API 直连会话 | 支持选择 API 提供商和模型，直接与模型 API 端到端对话，支持多轮历史 |
| API URL 完整填写 | 用户填写完整 URL，系统直接调用，不再智能拼接路径 |
| 消息 Markdown 渲染 | 引入 react-markdown + remark-gfm + rehype-highlight |
| 消息虚拟滚动 | 引入 react-virtuoso，大量消息高性能渲染 |
| 会话重命名 | 会话列表支持内联重命名和编辑 |
| 灵感收藏 | 从消息气泡 hover 按钮一键收藏为灵感 |
| 4 种对话模式 | 原生/快速/深度思考/专家，带颜色标识 |
| 环境配置 | 检测系统环境，一键安装 Claude Code / Hermes |
| Bot 通道 | 微信/钉钉 Bot 通道配置管理 |
| 设置页增加环境 Tab | 5 Tab 布局（通用/环境/Agent参数/API配置/关于） |

### 7.2 架构变更

| 变更项 | v1.0 | v2.0 |
|--------|------|------|
| API 提供商数据 | localStorage | SQLite `api_providers` 表 |
| API Key 存储 | localStorage | SQLite（Rust 端管理） |
| 通用设置数据 | localStorage | SQLite `app_settings` KV 表 |
| 前端数据访问方式 | `localStorage.getItem/setItem` | `useApiProviderStore` (Zustand) + `invoke` |
| Rust 命令数量 | ~10 个 | **38 个**（新增 api_provider 6 + app_settings 2 + bot 2 + theme 2 + 其他） |
| 会话类型 | `claude` / `hermes` | `claude` / `hermes` / `api` |

### 7.3 修复项

| 修复 | 描述 |
|------|------|
| 双层边框 | 移除会话列表项外层 `border-l-2`，仅用 `backgroundColor` 区分激活态 |
| 消息气泡布局 | 用户消息重构为 `flex justify-end` 全宽行 + `max-width:80%` 内层容器 |
| 操作按钮显示 | 改为 `opacity-0 group-hover:opacity-100`，hover 时显示 |
| API 404 错误 | 路径重复拼接导致 `/v4/v1/chat/completions`，改为用户填完整 URL |
| 命名风格统一 | 前后端字段统一为 camelCase，消除 snake_case 不一致 |

---

*Copyright (c) 2026 PilotDesk by 简意工作室 (jorryn)*
*本项目代码基于 MIT 协议开源*
