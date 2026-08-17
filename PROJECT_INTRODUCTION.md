# PilotDesk 项目介绍

## 一、项目概述

**PilotDesk** 是一款基于 Tauri 2.x 构建的 AI Agent 统一桌面客户端，将 Claude Code、Hermes Agent 等 CLI Agent 及 LLM API 直连会话集成到单一应用，提供统一会话管理、工作流引擎、插件生态与多 Agent 群聊能力。

| 项目信息 | 内容 |
|---------|------|
| 项目名称 | PilotDesk |
| 版本 | v0.1.0 |
| 协议 | MIT |
| 平台 | Windows / macOS / Linux |

---

## 二、技术栈

| 层级 | 技术 |
|------|------|
| 桌面框架 | Tauri 2.x（Rust） |
| 前端 | React 19 + TypeScript + Tailwind CSS v4 |
| 状态管理 | Zustand 5 |
| 数据库 | SQLite（rusqlite + r2d2） |
| 插件运行时 | boa_engine（JS） |
| 构建工具 | Vite 8 |

---

## 三、核心功能

1. **多 Agent 统一接入**：支持 Claude Code、Hermes Agent、CodeX CLI 等 CLI Agent 子进程交互，以及 OpenAI/Anthropic 格式 API Agent 直连。
2. **五种对话模式**：原生、快速、深度思考、专家、规划模式，支持自定义 System Prompt。
3. **工作流引擎**：可视化 DAG 系统，支持 Agent 任务、API 调用、代码转换、人工交互节点，支持 Cron 定时调度。
4. **多 Agent 群聊**：创建群聊房间，邀请多 Agent 参与，支持轮次发言与立场追踪。
5. **插件系统**：基于 boa_engine，支持面板、命令、工作流节点扩展，提供独立 API 实例与事件总线。
6. **灵感市集**：记录想法与代码片段，支持标签分类和全文搜索。
7. **记忆系统**：内置 KV 记忆库，与 CLI Agent 内部记忆隔离，支持 skill 按需加载。

---

## 四、技术架构

```
前端 (React + TS) ──Tauri Invoke──▶ 后端 (Rust) ──▶ SQLite
   ├─ 会话界面                          ├─ AgentMgr
   ├─ 工作流编辑器                      ├─ API Agent
   └─ 群聊界面                          └─ Plugin / Terminal
```

---

## 五、数据存储

- **数据库表**：sessions、messages、api_providers、agents、workflows、inspirations 等
- **用户数据目录**：
  - Windows：`%APPDATA%/PilotDesk/`
  - macOS：`~/Library/Application Support/PilotDesk/`
  - Linux：`~/.config/PilotDesk/`

---

## 六、安全设计

| 风险等级 | 规则 | 处理 |
|---------|------|------|
| Safe | 只读操作 | 直接执行 |
| Medium | 写入/修改/删除 | 用户确认 |
| Blocked | 系统破坏性操作 | 直接拦截 |

- API Key 使用 AES-GCM 加密存储
- 路径安全限制，拒绝访问系统关键目录
- 执行超时：命令 30s、Python 30s、Agent Loop 180s

---

## 七、使用指南

```bash
npm install          # 安装依赖
npm run tauri dev    # 启动开发模式
npm run tauri build  # 构建生产版本
```

环境要求：Node.js ≥ 18、Rust ≥ 1.70、内存 ≥ 8GB。

---

## 八、项目结构

```
PilotDesk/
├── src/                    # 前端源码
│   ├── components/         # React 组件
│   ├── pages/              # 页面（会话/工作流/群聊/设置）
│   ├── stores/             # Zustand 状态管理
│   └── plugin/             # 插件系统核心
├── src-tauri/src/          # Rust 后端
│   ├── agent/              # CLI Agent 管理
│   ├── api_agent/          # API Agent
│   ├── workflow/           # 工作流引擎
│   ├── groupchat/          # 群聊管理
│   ├── plugin/             # 插件系统
│   ├── terminal/           # 终端管理
│   └── db/                 # 数据库层
├── docs/                   # 设计文档
└── scripts/                # 构建脚本
```

---

## 九、技术亮点

1. **Windows 控制台编码处理**：智能解码 GBK/UTF-8，避免中文乱码
2. **智能文件截断**：大文件采用 head+tail 策略，保留首尾关键内容
3. **增量摘要生成**：双阈值上下文持久化，长会话自动压缩历史
4. **并行审批机制**：tokio::sync::oneshot 非阻塞审批，不冻结工作线程

---

## 十、总结

PilotDesk 是一个功能完善的 AI Agent 桌面工作台，通过统一界面管理多种 Agent，提供会话管理、群组聊天、工作流自动化、插件扩展等核心能力，采用 Tauri 2.x 架构，注重安全性、可扩展性和用户体验。

---

*文档版本：v2.0*
*更新日期：2026-07-10*
