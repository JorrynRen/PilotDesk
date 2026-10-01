#!/usr/bin/env python3
"""
PPTX CLI — 统一 PPTX 操作入口。

用法：
    # ── 查询 ──
    pptx_cli.py query        <pptx> [--slide N] [--include summary,texts,layouts,images,tables,charts]
    pptx_cli.py query-shape  <pptx> --slide N --shape-id N [--include text,style,layout]

    # ── 编辑 ──
    pptx_cli.py edit-text      <pptx> --slide N --shape-id N --operations '<json>'
    pptx_cli.py copy-shape     <pptx> --slide N --src-shape-id N [--left-cm X] [--top-cm Y] [--width-cm W] [--height-cm H]
    pptx_cli.py delete-shape   <pptx> --slide N --shape-id N
    pptx_cli.py move-shape     <pptx> --slide N --shape-id N [--left-cm X] [--top-cm Y] [--width-cm W] [--height-cm H]
    pptx_cli.py resize-shape   <pptx> --slide N --shape-id N [--width-cm W] [--height-cm H]
    pptx_cli.py slide-organize <pptx> --operations '<json>'
    pptx_cli.py export-image   <pptx> --slide N --shape-id N -o <output>
    pptx_cli.py replace-image  <pptx> --slide N --shape-id N --image <path>
    pptx_cli.py get-fonts      [--category chinese|latin] [--keyword ...]

    # ── 生成（通过服务端 API）──
    pptx_cli.py check-layout <html> [<html> ...] [--mode static|browser|full]
    pptx_cli.py html-to-pptx <html> [<html> ...] -o <output.pptx>
    pptx_cli.py screenshot   <html> [<html> ...] [-o <output_dir>]
                               [--viewport-width W] [--viewport-height H]
                               [--load-timeout S] [--visual-ready-timeout S]

    # ── 异步任务管理 ──
    pptx_cli.py cancel-task <task_id>
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).parent


def _insert_scripts():
    if str(_SCRIPTS_DIR) not in sys.path:
        sys.path.insert(0, str(_SCRIPTS_DIR))


def _load_prs(path: str):
    _insert_scripts()
    from pptx import Presentation
    return Presentation(path)


def _get_slide(prs, slide_1based: int):
    """根据 1-based 页码获取 slide，并做越界检查。"""
    slides = list(prs.slides)
    idx = slide_1based - 1
    if idx < 0 or idx >= len(slides):
        print(f"[ERROR] 幻灯片编号超出范围（共 {len(slides)} 页）", file=sys.stderr)
        sys.exit(1)
    return slides[idx], idx


# ═══════════════════════════════════════════════════════════════════════════════
# 查询命令
# ═══════════════════════════════════════════════════════════════════════════════

def cmd_query(args: argparse.Namespace) -> None:
    _insert_scripts()
    from pptx import Presentation
    from analyze_pptx import query_slide

    prs = Presentation(args.pptx)
    include = [s.strip() for s in args.include.split(",")]

    slides = list(prs.slides)
    if args.slide is not None:
        idx = args.slide - 1
        if idx < 0 or idx >= len(slides):
            print(f"[ERROR] 幻灯片编号超出范围（共 {len(slides)} 页）", file=sys.stderr)
            sys.exit(1)
        slide_indices = [idx]
    else:
        slide_indices = list(range(len(slides)))

    result = {}
    for i in slide_indices:
        result[f"slide_{i + 1}"] = query_slide(slides[i], include)

    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_query_shape(args: argparse.Namespace) -> None:
    _insert_scripts()
    from analyze_pptx import query_shape

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    include = [s.strip() for s in args.include.split(",")]
    try:
        result = query_shape(slide, args.shape_id, include)
    except LookupError as e:
        print(f"[ERROR] shape_id={args.shape_id} 在第 {args.slide} 页中不存在", file=sys.stderr)
        sys.exit(1)
    except ValueError as e:
        print(f"[ERROR] {e}", file=sys.stderr)
        sys.exit(1)
    print(json.dumps(result, ensure_ascii=False, indent=2))


# ═══════════════════════════════════════════════════════════════════════════════
# 编辑命令
# ═══════════════════════════════════════════════════════════════════════════════

def cmd_edit_text(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import edit_text

    prs = _load_prs(args.pptx)
    _, slide_idx = _get_slide(prs, args.slide)
    operations = json.loads(args.operations)
    result = edit_text(prs, slide_idx, shape_id=args.shape_id, operations=operations)
    if result.get("success"):
        prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_copy_shape(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import copy_shape

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    new_id = copy_shape(
        slide,
        src_shape_id=args.src_shape_id,
        left_cm=args.left_cm,
        top_cm=args.top_cm,
        width_cm=args.width_cm,
        height_cm=args.height_cm,
    )
    prs.save(args.pptx)
    print(json.dumps({"success": True, "new_shape_id": new_id}, ensure_ascii=False, indent=2))


def cmd_delete_shape(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import delete_shape

    prs = _load_prs(args.pptx)
    _, slide_idx = _get_slide(prs, args.slide)
    result = delete_shape(prs, slide_idx, shape_id=args.shape_id)
    if result.get("success"):
        prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_move_shape(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import move_shape

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    result = move_shape(
        slide,
        shape_id=args.shape_id,
        left_cm=args.left_cm,
        top_cm=args.top_cm,
        width_cm=args.width_cm,
        height_cm=args.height_cm,
    )
    if result.get("success"):
        prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_resize_shape(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import resize_shape

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    result = resize_shape(
        slide,
        shape_id=args.shape_id,
        width_cm=args.width_cm,
        height_cm=args.height_cm,
    )
    if result.get("success"):
        prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_slide_organize(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import slide_organize

    prs = _load_prs(args.pptx)
    operations = json.loads(args.operations)
    result = slide_organize(prs, operations)
    if result.get("success"):
        prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_export_image(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import export_image

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    result = export_image(slide, shape_id=args.shape_id, output_path=args.output)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_replace_image(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import replace_shape_image

    prs = _load_prs(args.pptx)
    slide, _ = _get_slide(prs, args.slide)
    result = replace_shape_image(slide, shape_id=args.shape_id, image_path=args.image)
    prs.save(args.pptx)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_get_fonts(args: argparse.Namespace) -> None:
    _insert_scripts()
    from edit_operations import get_available_fonts

    result = get_available_fonts(
        category=args.category,
        keyword=args.keyword,
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_get_allowed_fonts(args: argparse.Namespace) -> None:
    _insert_scripts()
    from font_intersection import get_allowed_fonts

    result = get_allowed_fonts()
    print(json.dumps(result, ensure_ascii=False, indent=2))


# ═══════════════════════════════════════════════════════════════════════════════
# 生成命令（通过服务端 API）— 内部自动轮询
# ═══════════════════════════════════════════════════════════════════════════════

def cmd_check_layout(args: argparse.Namespace) -> None:
    """提交异步布局检测任务 → 自动轮询 → 输出检测报告。"""
    from transcoder_api import check_layout

    result = check_layout(args.html_files, mode=args.mode)
    # check_layout 内部已在 stderr 打印 task_id
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_html_to_pptx(args: argparse.Namespace) -> None:
    """提交异步 PPTX 转换任务 → 自动轮询 → 下载文件。"""
    from transcoder_api import html_to_pptx

    result = html_to_pptx(args.html_files, output_path=args.output)
    # html_to_pptx 内部已在 stderr 打印 task_id
    if result.get("success"):
        print(f"pptx 已转换成功，保存路径为 {result['path']}（{result['size_kb']} KB）")
    else:
        print(json.dumps(result, ensure_ascii=False, indent=2))
        sys.exit(1)


# ═══════════════════════════════════════════════════════════════════════════════
# 异步任务管理命令（底层 API，供模型直接调用）
# ═══════════════════════════════════════════════════════════════════════════════


def cmd_cancel_task(args: argparse.Namespace) -> None:
    """取消异步任务。"""
    from transcoder_api import cancel_task

    result = cancel_task(args.task_id)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def cmd_screenshot(args: argparse.Namespace) -> None:
    """提交异步截图任务 → 自动轮询 → 下载 ZIP → 解压 PNG。"""
    from transcoder_api import screenshot

    result = screenshot(
        args.html_files,
        output_dir=args.output or ".",
        viewport_width=args.viewport_width,
        viewport_height=args.viewport_height,
        load_timeout=args.load_timeout,
        visual_ready_timeout=args.visual_ready_timeout,
    )
    # screenshot 内部已在 stderr 打印 task_id
    if result.get("success"):
        guide = f"""截图已完成，请按以下格式总结各方案展示给用户，每个方案仅包含两项，不附加任何额外说明，不做任何多余操作：
1. 一行方案描述：`方案 N：中文设计描述`
2. 紧接使用 **`sharing_files`** 格式输出截图链接
展示截图后，询问用户选择哪个方案。
[下一步] 用户选定风格方案后，读取 `gen_html_ppt.md` 进入生成阶段。
"""
        result["guide"] = guide
    print(json.dumps(result, ensure_ascii=False, indent=2))


# ═══════════════════════════════════════════════════════════════════════════════
# argparse 注册
# ═══════════════════════════════════════════════════════════════════════════════

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="pptx_cli.py",
        description="PPTX 统一操作入口",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    # ── 查询 ──────────────────────────────────────────────────────────────────
    p = sub.add_parser("query", help="查询幻灯片内容和布局")
    p.add_argument("pptx", help="PPTX 文件路径")
    p.add_argument("--slide", type=int, default=None, help="指定页码（从 1 开始），不指定则查全部")
    p.add_argument("--include", default="summary,texts,layouts",
                   help="查询内容（逗号分隔）：summary,texts,layouts,images,tables,charts")

    p = sub.add_parser("query-shape", help="查询指定元素的详细信息")
    p.add_argument("pptx", help="PPTX 文件路径")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("--include", default="text,style,layout",
                   help="查询内容（逗号分隔）：text,style,layout")

    # ── 编辑 ──────────────────────────────────────────────────────────────────
    p = sub.add_parser("edit-text", help="修改元素文字内容和格式")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("--operations", required=True,
                   help='操作列表 JSON 字符串，如 \'[{"action":"set_run_text","para_index":0,"run_index":0,"text":"新标题"}]\'')

    p = sub.add_parser("copy-shape", help="复制元素")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--src-shape-id", type=int, required=True, help="源元素 shape_id")
    p.add_argument("--left-cm", type=float, default=None, help="目标 X 坐标（cm）")
    p.add_argument("--top-cm", type=float, default=None, help="目标 Y 坐标（cm）")
    p.add_argument("--width-cm", type=float, default=None, help="目标宽度（cm）")
    p.add_argument("--height-cm", type=float, default=None, help="目标高度（cm）")

    p = sub.add_parser("delete-shape", help="删除元素")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")

    p = sub.add_parser("move-shape", help="移动元素或同时调整尺寸")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("--left-cm", type=float, default=None, help="X 坐标（cm）")
    p.add_argument("--top-cm", type=float, default=None, help="Y 坐标（cm）")
    p.add_argument("--width-cm", type=float, default=None, help="宽度（cm）")
    p.add_argument("--height-cm", type=float, default=None, help="高度（cm）")

    p = sub.add_parser("resize-shape", help="仅调整元素尺寸")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("--width-cm", type=float, default=None, help="宽度（cm）")
    p.add_argument("--height-cm", type=float, default=None, help="高度（cm）")

    p = sub.add_parser("slide-organize", help="页面增删/复制/排序")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--operations", required=True,
                   help='操作列表 JSON 字符串，如 \'[{"action":"delete","index":2},{"action":"copy","index":0,"position":1}]\'')

    p = sub.add_parser("export-image", help="导出元素中的图片")
    p.add_argument("pptx", help="PPTX 文件路径")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("-o", "--output", required=True, help="导出路径（扩展名决定格式）")

    p = sub.add_parser("replace-image", help="替换元素中的图片")
    p.add_argument("pptx", help="PPTX 文件路径（原地保存）")
    p.add_argument("--slide", type=int, required=True, help="页码（从 1 开始）")
    p.add_argument("--shape-id", type=int, required=True, help="元素 shape_id")
    p.add_argument("--image", required=True, help="新图片文件路径")

    p = sub.add_parser("get-fonts", help="查询系统可用字体")
    p.add_argument("--category", default=None, choices=["chinese", "latin"],
                   help="字体分类：chinese / latin，不指定则全部")
    p.add_argument("--keyword", default=None, help="按关键字筛选字体名")

    p = sub.add_parser("get-allowed-fonts", help="获取当前系统允许使用的字体列表（白名单 ∩ 系统已安装字体）")

    # ── 生成（通过服务端 API）─────────────────────────────────────────────────
    p = sub.add_parser("check-layout", help="检查 HTML 幻灯片跑版（通过服务端 API）")
    p.add_argument("html_files", nargs="+", help="HTML 文件路径列表")
    p.add_argument("--mode", default="full", choices=["static", "browser", "full"],
                   help="检测模式：static（静态）/ browser（浏览器）/ full（完整，默认）")

    p = sub.add_parser("html-to-pptx", help="HTML 幻灯片 → PPTX（通过服务端 API）")
    p.add_argument("html_files", nargs="+", help="HTML 文件路径列表（按顺序）")
    p.add_argument("-o", "--output", required=True, help="输出 PPTX 文件路径")

    p = sub.add_parser("screenshot", help="HTML 幻灯片截图为 PNG（通过服务端 API）")
    p.add_argument("html_files", nargs="+", help="HTML 文件路径列表")
    p.add_argument("-o", "--output", default=None, help="PNG 输出目录（默认当前目录）")
    p.add_argument("--viewport-width", type=int, default=1280, help="视口宽度（默认 1280）")
    p.add_argument("--viewport-height", type=int, default=720, help="视口高度（默认 720）")
    p.add_argument("--load-timeout", type=int, default=15, help="页面加载超时秒数（默认 15）")
    p.add_argument("--visual-ready-timeout", type=int, default=8, help="视觉就绪超时秒数（默认 8）")

    # ── 异步任务管理 ──────────────────────────────────────────────────────────
    p = sub.add_parser("cancel-task", help="取消异步任务（供模型手动调用）")
    p.add_argument("task_id", help="任务 ID")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()

    commands = {
        "query": cmd_query,
        "query-shape": cmd_query_shape,
        "edit-text": cmd_edit_text,
        "copy-shape": cmd_copy_shape,
        "delete-shape": cmd_delete_shape,
        "move-shape": cmd_move_shape,
        "resize-shape": cmd_resize_shape,
        "slide-organize": cmd_slide_organize,
        "export-image": cmd_export_image,
        "replace-image": cmd_replace_image,
        "get-fonts": cmd_get_fonts,
        "get-allowed-fonts": cmd_get_allowed_fonts,
        "check-layout": cmd_check_layout,
        "html-to-pptx": cmd_html_to_pptx,
        "screenshot": cmd_screenshot,
        "cancel-task": cmd_cancel_task,
    }
    commands[args.command](args)


if __name__ == "__main__":
    main()
