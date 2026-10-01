#!/usr/bin/env python3
"""
技能打包器 - 将技能目录打成可分发的 .skill 文件

用法：
    python package_skill.py <path/to/skill-folder> [output-directory]

示例：
    python package_skill.py skills/public/my-skill
    python package_skill.py skills/public/my-skill ./dist
"""

import sys
import zipfile
from pathlib import Path

from quick_validate import validate_skill


def _is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def package_skill(skill_path, output_dir=None):
    """
    将技能目录打包成 .skill 文件。

    Args:
        skill_path: 技能目录路径
        output_dir: 可选输出目录（默认当前目录）

    Returns:
        创建出的 .skill 文件路径；如出错则返回 None
    """
    skill_path = Path(skill_path).resolve()

    # 校验技能目录是否存在
    if not skill_path.exists():
        print(f"[ERROR] 未找到技能目录：{skill_path}")
        return None

    if not skill_path.is_dir():
        print(f"[ERROR] 路径不是目录：{skill_path}")
        return None

    # 校验 SKILL.md 是否存在
    skill_md = skill_path / "SKILL.md"
    if not skill_md.exists():
        print(f"[ERROR] 在 {skill_path} 中未找到 SKILL.md")
        return None

    # 打包前先执行校验
    print("正在校验技能...")
    valid, message = validate_skill(skill_path)
    if not valid:
        print(f"[ERROR] 校验失败：{message}")
        print("   请先修复校验错误，再重新打包。")
        return None
    print(f"[OK] {message}\n")

    # 确定输出位置
    skill_name = skill_path.name
    if output_dir:
        output_path = Path(output_dir).resolve()
        output_path.mkdir(parents=True, exist_ok=True)
    else:
        output_path = Path.cwd()

    skill_filename = output_path / f"{skill_name}.skill"

    EXCLUDED_DIRS = {".git", ".svn", ".hg", "__pycache__", "node_modules"}

    # 创建 .skill 文件（本质是 zip）
    try:
        with zipfile.ZipFile(skill_filename, "w", zipfile.ZIP_DEFLATED) as zipf:
            # 遍历技能目录
            for file_path in skill_path.rglob("*"):
                # 安全限制：绝不跟随或打包符号链接。
                if file_path.is_symlink():
                    print(f"[WARN] 跳过符号链接：{file_path}")
                    continue

                rel_parts = file_path.relative_to(skill_path).parts
                if any(part in EXCLUDED_DIRS for part in rel_parts):
                    continue

                if file_path.is_file():
                    resolved_file = file_path.resolve()
                    if not _is_within(resolved_file, skill_path):
                        print(f"[ERROR] 文件越过了技能根目录：{file_path}")
                        return None
                    # 如果输出路径位于 skill_path 之下，避免把归档文件打进自己。
                    if resolved_file == skill_filename.resolve():
                        print(f"[WARN] 跳过输出归档文件：{file_path}")
                        continue

                    # 计算 zip 内部的相对路径。
                    arcname = Path(skill_name) / file_path.relative_to(skill_path)
                    zipf.write(file_path, arcname)
                    print(f"  已添加：{arcname}")

        print(f"\n[OK] 技能已成功打包到：{skill_filename}")
        return skill_filename

    except Exception as e:
        print(f"[ERROR] 创建 .skill 文件时出错：{e}")
        return None


def main():
    if len(sys.argv) < 2:
        print("用法：python package_skill.py <path/to/skill-folder> [output-directory]")
        print("\n示例：")
        print("  python package_skill.py skills/public/my-skill")
        print("  python package_skill.py skills/public/my-skill ./dist")
        sys.exit(1)

    skill_path = sys.argv[1]
    output_dir = sys.argv[2] if len(sys.argv) > 2 else None

    print(f"正在打包技能：{skill_path}")
    if output_dir:
        print(f"   输出目录：{output_dir}")
    print()

    result = package_skill(skill_path, output_dir)

    if result:
        sys.exit(0)
    else:
        sys.exit(1)


if __name__ == "__main__":
    main()
