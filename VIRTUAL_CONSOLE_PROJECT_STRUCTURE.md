# PilotDesk 虚拟控制台项目结构

## 项目概述

PilotDesk 是一个基于 Tauri + React 的桌面应用，提供多 Agent 会话管理、统一消息交互、环境检测与 Agent 安装向导、灵感市集、技能系统、插件系统等功能。本项目已成功集成虚拟控制台功能，支持 Windows、macOS、Linux 三大平台。

## 正确的项目结构

```
pilotdesk/
├── src/                          # 前端源代码
│   ├── VirtualConsoleManager.tsx  # 虚拟控制台状态管理
│   ├── components/               # React 组件
│   │   ├── layout/              # 布局组件
│   │   │   └── TitleBar.tsx     # 标题栏组件（已修改）
│   │   └── VirtualConsolePanel.tsx # 虚拟控制台面板组件
│   ├── App.tsx                  # 主应用组件（已修改）
│   └── ...                      # 其他前端文件
├── src-tauri/                   # Tauri 后端源代码
│   ├── src/                     # Rust 源代码
│   │   ├── lib.rs               # 主库文件（已修改）
│   │   ├── commands/            # Tauri 命令
│   │   │   ├── mod.rs          # 命令模块入口（已修改）
│   │   │   └── virtual_console.rs # 虚拟控制台命令
│   │   └── virtual_console/     # 虚拟控制台模块
│   │       ├── mod.rs          # 模块入口
│   │       ├─
