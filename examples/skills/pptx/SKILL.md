---
name: pptx
description: "当需要执行操作 PPT/幻灯片 的行为时使用本技能，包括：创建或生成幻灯片、演示文稿，读取/解析/提取现有 .pptx 文件中的内容，编辑PPT版式与内容、套用模板与布局等操作对象或产物为PPT的行为。"
---

> ## PilotDesk 环境适配
> 本技能适配 PilotDesk 桌面客户端，无需任何预设环境变量（不使用 bash、不使用 $SKILL_PATH_BASH、不依赖任何硬编码安装路径）。
> - 技能根目录：由 load_skill 返回内容顶部给出（形如 `> 技能根目录：C:\...\skills\pptx`）；
> - 所有脚本调用统一为：execute_command 运行 `python "<技能根目录>\scripts\pptx_cli.py" <参数>`；
> - 本文档中所有命令均按此方式理解与执行。
> - 注：check-layout / html-to-pptx / screenshot / query-task / cancel-task 依赖外部转换服务，可能不可用；本地 PPT 读取/编辑/生成请使用 python-pptx 相关命令。

# PPTX

## PPTX 读取
当用户要求：打开 / 解析 / 提取现有 PPT 内容时，请阅读 read_ppt.md 获取更多指导。

## PPTX 生成
当用户要求：创建 PPT / 将文档转换为 PPT 时，请阅读 gen_ppt.md 获取更多指导。

## PPTX 编辑
当用户要求修改上传的 PPT 时，请阅读 edit_ppt.md 获取更多指导。

## PPTX 讲稿
当用户要求基于 PPT 生成演讲稿时，请阅读 ppt_speech_writer.md 获取更多指导。

# 注意

## 基本指南
1. 路径指代约定：
- **产出文件**（写入或读取任务产物）：以 `<ppt-dir>/` 为根
- **指引文件**（技能内待阅读的规范/参考）：以本文档的目录为根
2. 文件读取规则：
- 对于指引文件，在任务执行过程中必须按照工作步骤执行到特定环节时，**按需读取**。**禁止**在执行任务前读取所有指南和规范文档。
3. 执行环境：PilotDesk 不注入 SKILL_PATH* 环境变量，脚本调用统一用 execute_command 运行 `python "<技能根目录>\scripts\pptx_cli.py" <子命令>`。