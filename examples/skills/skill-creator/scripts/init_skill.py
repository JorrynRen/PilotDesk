#!/usr/bin/env python3
"""
技能初始化器 - 根据模板创建新技能

用法：
    init_skill.py <skill-name> [--path <path>] [--resources scripts,references,assets] [--examples]

--path 默认使用当前工作目录（WORKSPACE_DIR）。

示例：
    init_skill.py my-new-skill
    init_skill.py my-new-skill --resources scripts,references
    init_skill.py my-api-helper --path /custom/location --resources scripts --examples
"""

import argparse
import os
import re
import sys
from pathlib import Path

MAX_SKILL_NAME_LENGTH = 64
ALLOWED_RESOURCES = {"scripts", "references", "assets"}

SKILL_TEMPLATE = """---
name: {skill_name}
description: [TODO: 用完整且明确的话说明该技能的作用以及何时使用。务必包含“何时使用这个技能”这一点，例如具体场景、文件类型或会触发它的任务。]
---

# {skill_title}

## 概述

[TODO: 用 1 到 2 句话说明本技能提供什么能力]

## 如何组织本技能

[TODO: 选择最适合本技能目标的结构。常见模式如下：

**1. 基于工作流**（最适合顺序流程）
- 适合存在清晰分步流程的场景
- 示例：DOCX 技能可按“工作流决策树” -> “读取” -> “创建” -> “编辑”来组织
- 结构：## 概述 -> ## 工作流决策树 -> ## 第 1 步 -> ## 第 2 步...

**2. 基于任务**（最适合工具集合）
- 适合技能提供多种不同操作 / 能力的场景
- 示例：PDF 技能可按“快速开始” -> “合并 PDF” -> “拆分 PDF” -> “提取文本”来组织
- 结构：## 概述 -> ## 快速开始 -> ## 任务类别 1 -> ## 任务类别 2...

**3. 基于参考 / 指南**（最适合标准或规范）
- 适合品牌指南、编码规范或需求约束
- 示例：品牌样式可按“品牌指南” -> “颜色” -> “字体” -> “功能”来组织
- 结构：## 概述 -> ## 指南 -> ## 规范 -> ## 用法...

**4. 基于能力**（最适合集成系统）
- 适合技能提供多个彼此关联的能力
- 示例：产品管理技能可按“核心能力” -> 编号能力清单来组织
- 结构：## 概述 -> ## 核心能力 -> ### 1. 功能 -> ### 2. 功能...

这些模式可以按需混用。大多数技能都会组合使用多种模式（例如先按任务组织，再为复杂操作补充工作流）。

完成后删除整个“如何组织本技能”章节，它只是一段指导说明。]

## [TODO: 根据选定结构，替换为第一个主章节标题]

[TODO: 在这里补充内容。可参考已有技能中的常见形式：
- 技术类技能可提供代码示例
- 复杂流程可提供决策树
- 给出贴近真实请求的具体示例
- 按需引用脚本 / 模板 / 参考资料]

## 资源（可选）

只创建本技能真正需要的资源目录。如果不需要任何资源，请删除本章节。

### scripts/
可直接运行、用于执行特定操作的代码（Python/Bash 等）。

**其他技能中的示例：**
- PDF 技能：`fill_fillable_fields.py`、`extract_form_field_info.py` - PDF 操作工具
- DOCX 技能：`document.py`、`utilities.py` - 文档处理模块

**适合放这里的内容：** Python 脚本、Shell 脚本，或任何用于自动化、数据处理或特定操作的可执行代码。

**注意：** 脚本可以不读入上下文而直接执行，但 agent 仍可能读取它们来打补丁或做环境适配。

### references/
供 agent 按需加载到上下文中的文档和参考资料，用于支持其过程与思考。

**其他技能中的示例：**
- 产品管理：`communication.md`、`context_building.md` - 详细工作流指南
- BigQuery：API 参考文档和查询示例
- 财务：模式文档、公司政策

**适合放这里的内容：** 深度文档、API 参考、数据库模式、完整指南，或任何 agent 在工作时应查阅的详细信息。

### assets/
这类文件不是用来加载进上下文的，而是供 agent 在最终输出中直接使用。

**其他技能中的示例：**
- 品牌样式：PowerPoint 模板文件（`.pptx`）、Logo 文件
- 前端构建：HTML/React 样板工程目录
- 字体：字体文件（`.ttf`、`.woff2`）

**适合放这里的内容：** 模板、样板代码、文档模板、图片、图标、字体，或任何需要在最终产出里被复制 / 使用的文件。

---

**并不是每个技能都需要这三类资源。**
"""

EXAMPLE_SCRIPT = '''#!/usr/bin/env python3
"""
{skill_name} 的示例辅助脚本

这是一个可直接执行的占位脚本。
如果需要，请替换为真实实现；如果不需要，请删除。

其他技能中的真实脚本示例：
- pdf/scripts/fill_fillable_fields.py - 填写 PDF 表单字段
- pdf/scripts/convert_pdf_to_images.py - 将 PDF 页面转换为图片
"""

def main():
    print("这是 {skill_name} 的示例脚本")
    # TODO: 在这里补充真实脚本逻辑
    # 例如：数据处理、文件转换、API 调用等

if __name__ == "__main__":
    main()
'''

EXAMPLE_REFERENCE = """# {skill_title} 的参考文档

这是详细参考文档的占位文件。
如果需要，请替换为真实参考内容；如果不需要，请删除。

其他技能中的真实参考文档示例：
- product-management/references/communication.md - 状态同步的完整指南
- product-management/references/context_building.md - 上下文收集的深入说明
- bigquery/references/ - API 参考和查询示例

## 什么时候适合使用参考文档

参考文档尤其适合承载：
- 完整的 API 文档
- 详细工作流指南
- 复杂的多步骤流程
- 不适合放进主 `SKILL.md` 的长内容
- 只在特定用例下才需要的内容

## 结构建议

### API 参考示例
- 概述
- 鉴权
- 各端点及示例
- 错误码
- 速率限制

### 工作流指南示例
- 前置条件
- 分步说明
- 常见模式
- 故障排查
- 最佳实践
"""

EXAMPLE_ASSET = """# 示例资源文件

这个占位文件表示资源文件应存放的位置。
如果需要，请替换为真实资源文件（模板、图片、字体等）；如果不需要，请删除。

资源文件**不是**用来加载进上下文的，而是供 agent 在最终输出中直接使用。

其他技能中的真实资源文件示例：
- 品牌指南：`logo.png`、`slides_template.pptx`
- 前端构建：包含 HTML/React 样板的 `hello-world/` 目录
- 字体：`custom-font.ttf`、`font-family.woff2`
- 数据：`sample_data.csv`、`test_dataset.json`

## 常见资源类型

- 模板：`.pptx`、`.docx`、样板目录
- 图片：`.png`、`.jpg`、`.svg`、`.gif`
- 字体：`.ttf`、`.otf`、`.woff`、`.woff2`
- 样板代码：工程目录、起始文件
- 图标：`.ico`、`.svg`
- 数据文件：`.csv`、`.json`、`.xml`、`.yaml`

注意：这只是一个文本占位文件。真实资源可以是任意文件类型。
"""


def normalize_skill_name(skill_name):
    """将技能名规范化为小写连字符格式。"""
    normalized = skill_name.strip().lower()
    normalized = re.sub(r"[^a-z0-9]+", "-", normalized)
    normalized = normalized.strip("-")
    normalized = re.sub(r"-{2,}", "-", normalized)
    return normalized


def title_case_skill_name(skill_name):
    """把连字符格式的技能名转换为展示用 Title Case。"""
    return " ".join(word.capitalize() for word in skill_name.split("-"))


def parse_resources(raw_resources):
    if not raw_resources:
        return []
    resources = [item.strip() for item in raw_resources.split(",") if item.strip()]
    invalid = sorted({item for item in resources if item not in ALLOWED_RESOURCES})
    if invalid:
        allowed = ", ".join(sorted(ALLOWED_RESOURCES))
        print(f"[ERROR] 未知资源类型：{', '.join(invalid)}")
        print(f"   允许值：{allowed}")
        sys.exit(1)
    deduped = []
    seen = set()
    for resource in resources:
        if resource not in seen:
            deduped.append(resource)
            seen.add(resource)
    return deduped


def create_resource_dirs(skill_dir, skill_name, skill_title, resources, include_examples):
    for resource in resources:
        resource_dir = skill_dir / resource
        resource_dir.mkdir(exist_ok=True)
        if resource == "scripts":
            if include_examples:
                example_script = resource_dir / "example.py"
                example_script.write_text(
                    EXAMPLE_SCRIPT.format(skill_name=skill_name),
                    encoding="utf-8",
                )
                example_script.chmod(0o755)
                print("[OK] 已创建 scripts/example.py")
            else:
                print("[OK] 已创建 scripts/")
        elif resource == "references":
            if include_examples:
                example_reference = resource_dir / "api_reference.md"
                example_reference.write_text(
                    EXAMPLE_REFERENCE.format(skill_title=skill_title),
                    encoding="utf-8",
                )
                print("[OK] 已创建 references/api_reference.md")
            else:
                print("[OK] 已创建 references/")
        elif resource == "assets":
            if include_examples:
                example_asset = resource_dir / "example_asset.txt"
                example_asset.write_text(EXAMPLE_ASSET, encoding="utf-8")
                print("[OK] 已创建 assets/example_asset.txt")
            else:
                print("[OK] 已创建 assets/")


def init_skill(skill_name, path, resources, include_examples):
    """
    用模板初始化一个新技能目录。

    Args:
        skill_name: 技能名称
        path: 创建技能目录的目标路径
        resources: 需要创建的资源目录
        include_examples: 是否在资源目录中生成示例文件

    Returns:
        创建出的技能目录路径；如出错则返回 None
    """
    # 确定技能目录路径
    skill_dir = Path(path).resolve() / skill_name

    # 检查目录是否已存在
    if skill_dir.exists():
        print(f"[ERROR] 技能目录已存在：{skill_dir}")
        return None

    # 创建技能目录
    try:
        skill_dir.mkdir(parents=True, exist_ok=False)
        print(f"[OK] 已创建技能目录：{skill_dir}")
    except Exception as e:
        print(f"[ERROR] 创建目录时出错：{e}")
        return None

    # 根据模板创建 SKILL.md
    skill_title = title_case_skill_name(skill_name)
    skill_content = SKILL_TEMPLATE.format(skill_name=skill_name, skill_title=skill_title)

    skill_md_path = skill_dir / "SKILL.md"
    try:
        skill_md_path.write_text(skill_content, encoding="utf-8")
        print("[OK] 已创建 SKILL.md")
    except Exception as e:
        print(f"[ERROR] 创建 SKILL.md 时出错：{e}")
        return None

    # 如有需要，创建资源目录
    if resources:
        try:
            create_resource_dirs(
                skill_dir,
                skill_name,
                skill_title,
                resources,
                include_examples,
            )
        except Exception as e:
            print(f"[ERROR] 创建资源目录时出错：{e}")
            return None

    # 输出下一步提示
    print(f"\n[OK] 技能 '{skill_name}' 已成功初始化到：{skill_dir}")
    print("\n下一步：")
    print("1. 编辑 SKILL.md，补完 TODO 项并更新 description")
    if resources:
        if include_examples:
            print("2. 按需定制或删除 scripts/、references/、assets/ 中的示例文件")
        else:
            print("2. 按需向 scripts/、references/、assets/ 中添加资源")
    else:
        print("2. 仅在需要时再创建资源目录（scripts/、references/、assets/）")
    print("3. 完成后运行校验脚本，检查技能结构是否正确")

    return skill_dir


def main():
    parser = argparse.ArgumentParser(
        description="创建一个带 SKILL.md 模板的新技能目录。",
    )
    parser.add_argument("skill_name", help="技能名称（会被规范化为小写连字符格式）")
    parser.add_argument(
        "--path",
        default=os.environ.get("WORKSPACE_DIR", "."),
        help="技能的输出目录（默认 WORKSPACE_DIR 或当前目录）",
    )
    parser.add_argument(
        "--resources",
        default="",
        help="逗号分隔列表：scripts,references,assets",
    )
    parser.add_argument(
        "--examples",
        action="store_true",
        help="在所选资源目录中创建示例文件",
    )
    args = parser.parse_args()

    raw_skill_name = args.skill_name
    skill_name = normalize_skill_name(raw_skill_name)
    if not skill_name:
        print("[ERROR] 技能名称必须至少包含一个字母或数字。")
        sys.exit(1)
    if len(skill_name) > MAX_SKILL_NAME_LENGTH:
        print(
            f"[ERROR] 技能名 '{skill_name}' 过长（{len(skill_name)} 个字符）。"
            f" 最大长度为 {MAX_SKILL_NAME_LENGTH} 个字符。"
        )
        sys.exit(1)
    if skill_name != raw_skill_name:
        print(f"提示：技能名已从 '{raw_skill_name}' 规范化为 '{skill_name}'。")

    resources = parse_resources(args.resources)
    if args.examples and not resources:
        print("[ERROR] 使用 --examples 时必须同时设置 --resources。")
        sys.exit(1)

    path = args.path

    print(f"正在初始化技能：{skill_name}")
    print(f"   位置：{path}")
    if resources:
        print(f"   资源：{', '.join(resources)}")
        if args.examples:
            print("   示例文件：已启用")
    else:
        print("   资源：无（按需创建）")
    print()

    result = init_skill(skill_name, path, resources, args.examples)

    if result:
        sys.exit(0)
    else:
        sys.exit(1)


if __name__ == "__main__":
    main()
