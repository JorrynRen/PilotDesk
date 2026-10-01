#!/usr/bin/env python3
"""
Skill Creator CLI — 统一技能操作入口。

用法：
    skill_creator_cli.py init     <skill-name> [--path <dir>] [--resources scripts,references,assets] [--examples]
    skill_creator_cli.py package  <skill-dir> [--output <dir>]
    skill_creator_cli.py validate <skill-dir>
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).parent


def _insert_scripts():
    sys.path.insert(0, str(_SCRIPTS_DIR))


def cmd_init(args: argparse.Namespace) -> None:
    _insert_scripts()
    # init_skill.py 是独立脚本，直接 exec 它的 main()
    import importlib.util
    spec = importlib.util.spec_from_file_location("init_skill", _SCRIPTS_DIR / "init_skill.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)

    init_args = [args.skill_name]
    if args.path:
        init_args += ["--path", args.path]
    if args.resources:
        init_args += ["--resources", args.resources]
    if args.examples:
        init_args += ["--examples"]

    sys.argv = ["init_skill.py"] + init_args
    mod.main()


def cmd_package(args: argparse.Namespace) -> None:
    _insert_scripts()
    import importlib.util
    spec = importlib.util.spec_from_file_location("package_skill", _SCRIPTS_DIR / "package_skill.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)

    result = mod.package_skill(args.skill_dir, args.output)
    if result is None:
        sys.exit(1)
    print(f"打包成功：{result}")


def cmd_validate(args: argparse.Namespace) -> None:
    _insert_scripts()
    from quick_validate import validate_skill

    valid, message = validate_skill(args.skill_dir)
    if valid:
        print(f"校验通过：{args.skill_dir}")
    else:
        print(f"[ERROR] {message}", file=sys.stderr)
        sys.exit(1)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="skill_creator_cli.py",
        description="Skill Creator 统一操作入口",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_init = sub.add_parser("init", help="初始化新技能目录")
    p_init.add_argument("skill_name", help="技能名称（kebab-case）")
    p_init.add_argument("--path", default="", help="输出目录（默认 WORKSPACE_DIR 或当前目录）")
    p_init.add_argument(
        "--resources",
        default="",
        help="创建资源目录，逗号分隔：scripts,references,assets",
    )
    p_init.add_argument("--examples", action="store_true", help="生成示例文件")

    p_pkg = sub.add_parser("package", help="打包技能为 .skill 文件")
    p_pkg.add_argument("skill_dir", help="技能目录路径")
    p_pkg.add_argument("--output", default="", help="输出目录（默认当前目录）")

    p_val = sub.add_parser("validate", help="校验技能目录")
    p_val.add_argument("skill_dir", help="技能目录路径")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()

    commands = {
        "init": cmd_init,
        "package": cmd_package,
        "validate": cmd_validate,
    }
    commands[args.command](args)


if __name__ == "__main__":
    main()
