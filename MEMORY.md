# Project Memory (MEMORY.md)

## 项目概述
- **项目名称**: PilotDesk
- **技术栈**: Rust (Tauri 2.x) + React (TypeScript) + SQLite
- **项目定位**: 统一 AI Agent 工作台，支持多 Agent 编排（Claude Code / Hermes / Codex / API Agent）+ 工作流引擎 + 插件系统

## 工程规范
- 所有已记录的 Hard Constraints 和 Lessons Learned 均来自项目实际开发过程，修改相关逻辑时必须遵守
- API 会话使用 AgentLoop 编排（tool-calling 循环），CLI 会话使用子进程交互
- 技能系统采用 Progressive Disclosure 模式（name+description 注入 prompt，load_skill 工具按需加载）
- 记忆隔离：PilotDesk KV 记忆库不替代/不读取/不写入 CLI Agent 内部记忆

## 开发命令
```bash
# 前端开发
npm install
npm run tauri dev

# Rust 编译检查
cd src-tauri && cargo check

# 构建
npm run tauri build
```

## 数据存储
- 项目数据库: src-tauri 运行目录下的 project.db
- 用户配置: %APPDATA%/PilotDesk/（Windows）或 ~/.config/pilotdesk/（Linux/macOS）
- KV 记忆: %APPDATA%/PilotDesk/memories.json
- 技能目录: %APPDATA%/PilotDesk/skills/
