# PilotDesk 实现计划 v1.0

> 版本: v1.0 | 作者: 简意工作室 (jorryn) | 日期: 2026-05

---

## 1. 项目信息

- **项目名称**: PilotDesk — AI Agent 统一桌面客户端
- **技术栈**: Tauri 2.0 + React 19 + TypeScript + TailwindCSS + Zustand + Rust
- **开源协议**: MIT
- **代码仓库**: pilotdesk/

---

## 2. 架构设计

```
┌─────────────────────────────────────────┐
│              React 前端                  │
│  ┌─────────┐ ┌──────────┐ ┌───────────┐ │
│  │ Pages   │ │Components│ │  Stores   │ │
│  │(App/    │ │(TitleBar/│ │(Zustand:  │ │
│  │ Settings│ │ Session/ │ │ session/  │ │
│  │ Market) │ │ Message/ │ │ pending/  │ │
│  │         │ │ Input)   │ │ insp)     │ │
│  └────┬────┘ └────┬─────┘ └─────┬─────┘ │
│       │           │             │        │
│       └───────────┼─────────────┘        │
│                   │ Tauri IPC             │
├───────────────────┼───────────────────────┤
│              Rust 后端                    │
│  ┌──────────┐ ┌───────────┐ ┌────────┐  │
│  │Commands  │ │ SQLite     │ │Sidecar │  │
│  │(IPC处理) │ │(rusqlite)  │ │Manager │  │
│  └──────────┘ └───────────┘ └───┬────┘  │
└──────────────────────────────────┼───────┘
                                   │
                    ┌──────────────┘
                    │
              ┌─────┴──────┐
              │ Node.js    │
              │ Sidecar    │
              │(WebSocket │
              │ + Agent)   │
              └────────────┘
```

---

## 3. 目录结构

```
pilotdesk/
├── src/                          # React 前端源码
│   ├── components/
│   │   ├── layout/               # 布局组件
│   │   │   ├── TitleBar.tsx      # 标题栏（拖动、双击最大化、窗口控制）
│   │   │   ├── SessionList.tsx   # 会话列表
│   │   │   ├── MainPanel.tsx     # 主面板（消息区域）
│   │   │   ├── RightPanel.tsx    # 右侧面板
│   │   │   ├── StatusBar.tsx     # 状态栏
│   │   │   └── InspirationPanel.tsx # 灵感面板（独立组件）
│   │   ├── input/                # 输入相关
│   │   │   ├── InspirationPicker.tsx # 灵感搜索（本地过滤）
│   │   │   ├── SkillPicker.tsx   # 技能选择（内置+自定义）
│   │   │   └── ...
│   │   └── inspiration/         # 灵感相关
│   │       └── MarketPage.tsx    # 灵感市集
│   ├── hooks/
│   │   └── useWebSocket.ts       # WebSocket hook + SSE 流式
│   ├── pages/
│   │   └── SettingsPage.tsx      # 设置页（4 Tab + 拖拽 + 测试连接）
│   ├── stores/
│   │   ├── sessionStore.ts       # 会话状态
│   │   ├── pendingInputStore.ts  # 输入桥接（替代sessionStorage）
│   │   └── inspirationStore.ts   # 灵感状态
│   ├── styles/
│   │   └── ui.css                # CSS 变量 + 全局样式
│   ├── App.tsx                   # 主应用（页面路由：main/market/settings）
│   └── main.tsx                  # 入口
├── src-tauri/                    # Tauri/Rust 后端
│   ├── src/
│   │   ├── lib.rs                # 主入口（SidecarManager、dialog插件）
│   │   └── ...
│   ├── capabilities/
│   │   └── default.json          # 权限配置
│   ├── Cargo.toml
│   └── tauri.conf.json
├── image/                        # 品牌图标资源
│   ├── logo.svg
│   ├── icon-*.png (16/32/64/128/256/512)
│   └── tray-icon.png
├── package.json
├── tsconfig.json
├── tailwind.config.js
├── LICENSE                       # MIT License
└── ...
```

---

## 4. 已实现功能清单

### 4.1 Phase 1 — 基础架构 ✅

- [x] Tauri 2.0 + React 19 项目初始化
- [x] TypeScript + TailwindCSS 配置
- [x] Zustand 状态管理集成
- [x] CSS 变量系统（深色主题）
- [x] 自定义标题栏（拖动、窗口控制）

### 4.2 Phase 2 — 会话系统 ✅

- [x] 会话列表（创建、删除、切换、重命名）
- [x] 会话模式切换（Claude Code / Hermes Agent）
- [x] 消息气泡（用户靠右、Agent靠左）
- [x] 消息编辑（填充输入框）
- [x] 用户消息收藏灵感
- [x] SSE 流式输出（Anthropic + OpenAI 格式）

### 4.3 Phase 3 — 灵感系统 ✅

- [x] 灵感面板（独立组件）
- [x] 灵感创建（标题 + 内容 + 24图标选择）
- [x] 灵感搜索（本地过滤，不污染全局store）
- [x] 灵感应用（点击填入输入框）
- [x] 灵感市集页面
- [x] pendingInputStore（Zustand替代sessionStorage）

### 4.4 Phase 4 — API + 设置 ✅

- [x] API 配置（列表式新增/编辑/删除）
- [x] API 拖拽排序（@dnd-kit）
- [x] API 测试连接（max_tokens=1 最小请求）
- [x] 设置页（通用/Agent参数/API配置/关于 四Tab）
- [x] Agent 参数配置（Claude Code + Hermes Agent）
- [x] 通用设置-工作区目录（原生目录浏览选择）
- [x] 关于页（MIT协议 + 版权信息）

### 4.5 Phase 5 — UI 打磨 ✅

- [x] 标题栏按钮紧邻窗口操作按钮
- [x] 最大化后图标变为还原
- [x] 会话模式下拉向上展开
- [x] 整个标题栏可拖动
- [x] 标题栏双击最大化/还原
- [x] 设置按钮 toggle
- [x] Tab 区域滚动条修正
- [x] 技能系统（12内置 + localStorage自定义）
- [x] MIT License 文件

---

## 5. 待完成功能

### 5.1 Sidecar 通信层

- [ ] Node.js Sidecar 完整 WebSocket 服务
- [ ] Rust SidecarManager 进程监控 + 自动重启
- [ ] 端到端 Agent 通信测试

### 5.2 UI 细节

- [ ] 状态栏重复提示修复
- [ ] 空状态引导优化
- [ ] 响应式布局微调

---

## 6. 开发规范

### 6.1 代码规范

- TypeScript 严格模式
- 组件使用函数式组件 + Hooks
- 状态管理统一使用 Zustand
- CSS 使用 TailwindCSS 工具类 + CSS 变量

### 6.2 Git 规范

```
feat: 新功能
fix: 修复bug
refactor: 重构
style: 样式调整
docs: 文档更新
```

### 6.3 提交链

参考: `110a502 → c5223b7 → 36cacad → 85a5f4a → 739aba2 → ...`

---

*Copyright (c) 2026 PilotDesk by 简意工作室 (jorryn)*
*本项目代码基于 MIT 协议开源*
