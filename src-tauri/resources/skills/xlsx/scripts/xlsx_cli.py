#!/usr/bin/env python3
"""
XLSX CLI — 统一 XLSX 脚本执行与公式校验入口。

用法：
    xlsx_cli.py build  <script.py> [script_args...]
    xlsx_cli.py recalc <file.xlsx> [--timeout 30]
    xlsx_cli.py audit  <file.xlsx>
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).parent


def _insert_scripts() -> None:
    scripts = str(_SCRIPTS_DIR)
    if scripts not in sys.path:
        sys.path.insert(0, scripts)


def cmd_build(args: argparse.Namespace) -> None:
    script = Path(args.script)
    if not script.is_file():
        print(f"[ERROR] 脚本不存在：{script}", file=sys.stderr)
        sys.exit(1)

    cmd = [sys.executable, str(script), *args.script_args]
    result = subprocess.run(cmd)
    sys.exit(result.returncode)


def cmd_recalc(args: argparse.Namespace) -> None:
    path = Path(args.xlsx)
    if not path.is_file():
        print(json.dumps({"error": f"File {path} does not exist"}, ensure_ascii=False))
        sys.exit(1)

    _insert_scripts()
    from recalc import recalc

    result = recalc(str(path), timeout=args.timeout)
    print(json.dumps(result, ensure_ascii=False, indent=2))

    if result.get("error"):
        sys.exit(1)
    if result.get("status") != "success":
        sys.exit(1)


def cmd_audit(args: argparse.Namespace) -> None:
    path = Path(args.xlsx)
    if not path.is_file():
        print(json.dumps({"error": f"File {path} does not exist"}, ensure_ascii=False))
        sys.exit(1)

    _insert_scripts()
    from audit import audit_formulas

    result = audit_formulas(str(path))
    print(json.dumps(result, ensure_ascii=False, indent=2))

    if result.get("status") != "success":
        sys.exit(1)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="xlsx_cli.py",
        description="XLSX 脚本执行与公式校验",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_build = sub.add_parser("build", help="执行任务脚本（如 report.py）")
    p_build.add_argument("script", help="Python 脚本路径")
    p_build.add_argument("script_args", nargs=argparse.REMAINDER, help="传给脚本的参数")

    p_recalc = sub.add_parser("recalc", help="重算并校验 xlsx 公式")
    p_recalc.add_argument("xlsx", help="xlsx 文件路径")
    p_recalc.add_argument(
        "--timeout",
        type=int,
        default=30,
        help="重算超时秒数，默认 30",
    )

    p_audit = sub.add_parser("audit", help="公式结构审计")
    p_audit.add_argument("xlsx", help="xlsx 文件路径")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()

    commands = {
        "build": cmd_build,
        "recalc": cmd_recalc,
        "audit": cmd_audit,
    }
    commands[args.command](args)


if __name__ == "__main__":
    main()
