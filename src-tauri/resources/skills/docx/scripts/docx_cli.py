#!/usr/bin/env python3
"""
DOCX CLI — 统一 DOCX 操作入口。

用法：
    docx_cli.py create   <script.js> -o <output.docx>
    docx_cli.py edit     <input.docx> -o <output.docx> --replace "旧=新" [--replace "A=B"] [--regex] [--ignore-case]
    docx_cli.py unpack   <input.docx> <output_dir>
    docx_cli.py pack     <input_dir>  <output.docx>
    docx_cli.py restore  <script.js>
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def cmd_create(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from run_code_docx import run_node_docx

    run_node_docx(args.script, output=args.output)


def cmd_edit(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from edit_docx import edit_docx

    replacements = []
    for item in args.replace:
        if "=" not in item:
            print(f"[ERROR] --replace 参数格式应为 '旧文本=新文本'，实际值：{item!r}", file=sys.stderr)
            sys.exit(1)
        old, new = item.split("=", 1)
        replacements.append({"from": old, "to": new})

    result = edit_docx(
        input_docx=args.input,
        output_docx=args.output,
        replacements=replacements,
        parts=args.parts if args.parts else None,
        match_mode="regex" if args.regex else "literal",
        ignore_case=args.ignore_case,
    )

    if result.get("ok"):
        applied = result.get("replacements_applied", [])
        for r in applied:
            print(f"  '{r['from']}' → '{r['to']}'：命中 {r['count']} 次")
        print(f"编辑成功：{result.get('output_path')}")
    else:
        errs = result.get("validation", {}).get("errors", [])
        print(f"编辑失败：{errs}", file=sys.stderr)
        sys.exit(1)


def cmd_unpack(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from office.unpack import unpack_docx

    output = args.output or args.output_pos
    if not output:
        print("[ERROR] unpack 需要输出目录：使用 -o <output_dir>（或兼容旧写法第二个位置参数）", file=sys.stderr)
        sys.exit(1)
    unpack_docx(args.input, output)
    print(f"解包成功：{output}")


def cmd_pack(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from office.pack import pack_docx

    output = args.output or args.output_pos
    if not output:
        print("[ERROR] pack 需要输出 DOCX：使用 -o <output.docx>（或兼容旧写法第二个位置参数）", file=sys.stderr)
        sys.exit(1)
    pack_docx(args.input, output)
    print(f"打包成功：{output}")


def cmd_restore(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from run_code_docx import restore_backup

    restore_backup(args.script)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="docx_cli.py",
        description="DOCX 统一操作入口",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_create = sub.add_parser("create", help="执行 JS 脚本生成 DOCX")
    p_create.add_argument("script", help="JS 脚本路径")
    p_create.add_argument("-o", "--output", required=True, help="输出 DOCX 文件路径")

    p_edit = sub.add_parser("edit", help="DOCX 文本替换")
    p_edit.add_argument("input", help="输入 DOCX 文件路径")
    p_edit.add_argument("-o", "--output", required=True, help="输出 DOCX 文件路径")
    p_edit.add_argument(
        "--replace",
        metavar="旧=新",
        action="append",
        default=[],
        help="替换规则，格式为 '旧文本=新文本'，可多次指定",
    )
    p_edit.add_argument("--regex", action="store_true", help="使用正则匹配")
    p_edit.add_argument("--ignore-case", action="store_true", help="忽略大小写")
    p_edit.add_argument(
        "--parts",
        metavar="GLOB",
        action="append",
        default=[],
        help="限制处理的 XML 部件（glob 模式），可多次指定；默认处理 document.xml + header*.xml + footer*.xml",
    )

    p_unpack = sub.add_parser("unpack", help="解包 DOCX 为目录（合并相邻同样式 run）")
    p_unpack.add_argument("input", help="输入 DOCX 文件路径")
    p_unpack.add_argument("output_pos", nargs="?", help="输出目录路径（兼容旧写法，推荐使用 -o）")
    p_unpack.add_argument("-o", "--output", help="输出目录路径")

    p_pack = sub.add_parser("pack", help="将目录重打包为 DOCX（自动校验）")
    p_pack.add_argument("input", help="输入目录路径（unpack 的输出）")
    p_pack.add_argument("output_pos", nargs="?", help="输出 DOCX 文件路径（兼容旧写法，推荐使用 -o）")
    p_pack.add_argument("-o", "--output", help="输出 DOCX 文件路径")

    p_restore = sub.add_parser("restore", help="恢复 JS 脚本备份")
    p_restore.add_argument("script", help="JS 脚本路径")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()

    commands = {
        "create": cmd_create,
        "edit": cmd_edit,
        "unpack": cmd_unpack,
        "pack": cmd_pack,
        "restore": cmd_restore,
    }
    commands[args.command](args)


if __name__ == "__main__":
    main()
