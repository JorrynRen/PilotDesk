#!/usr/bin/env python3
"""
PDF CLI — 统一 PDF 操作入口。

用法：
    pdf_cli.py info           <pdf>
    pdf_cli.py page-count     <pdf>
    pdf_cli.py extract-text   <pdf> [--pages 1,3,5] [--fast]
    pdf_cli.py extract-table  <pdf> [--page 1] [--format json|csv]
    pdf_cli.py to-md          <pdf> -o <output_dir>
    pdf_cli.py merge          <pdf1> <pdf2> ... -o <output.pdf>
    pdf_cli.py split          <pdf> -o <output_dir> [--pages 1,3,5-7]
    pdf_cli.py delete-pages   <pdf> -o <output.pdf> --pages 1,3,5-7
    pdf_cli.py rotate         <pdf> -o <output.pdf> [--pages 1,3,5] --angle 90
    pdf_cli.py crop           <pdf> -o <output.pdf> --bbox x0,y0,x1,y1 [--pages ...]
    pdf_cli.py watermark      <pdf> -o <output.pdf> --watermark <watermark.pdf>
    pdf_cli.py compress       <pdf> -o <output.pdf>
    pdf_cli.py encrypt        <pdf> -o <output.pdf> --password <pass>
    pdf_cli.py decrypt        <pdf> -o <output.pdf> --password <pass>
    pdf_cli.py render         <pdf> -o <output_dir> [--pages 1,3,5] [--dpi 150]
    pdf_cli.py extract-images <pdf> -o <output_dir>
    pdf_cli.py batch          <input_dir> <subcmd> [-o <output_dir>] [--pattern *.pdf] [-- extra_args...]
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def _parse_page_set(pages_str: str) -> set[int]:
    """解析页码字符串，如 '1,3,5-7' -> {1,3,5,6,7}。"""
    result: set[int] = set()
    for token in pages_str.split(","):
        token = token.strip()
        if not token:
            continue
        if "-" in token:
            a, b = token.split("-", 1)
            result.update(range(int(a), int(b) + 1))
        else:
            result.add(int(token))
    return result


def _read_pdf(path: str, *, strict: bool = False):
    """读取 PDF 并检查加密状态。"""
    from pypdf import PdfReader
    reader = PdfReader(path, strict=strict)
    if reader.is_encrypted:
        print("PDF 已加密，请先用 decrypt 命令解密", file=sys.stderr)
        sys.exit(1)
    return reader


def _write_pdf(writer, output: str | Path) -> None:
    """将 PdfWriter 写入文件。"""
    out = Path(output)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "wb") as f:
        writer.write(f)


# ============================================================
# 命令实现
# ============================================================

def cmd_info(args: argparse.Namespace) -> None:
    from pypdf import PdfReader
    reader = PdfReader(args.pdf, strict=False)
    if reader.is_encrypted:
        print("加密状态: 是\n总页数: 未知（需先解密）")
        return
    print(f"文件: {args.pdf}")
    print(f"总页数: {len(reader.pages)}")
    if reader.pages:
        p = reader.pages[0]
        w, h = float(p.mediabox.width), float(p.mediabox.height)
        print(f"首页尺寸: {w:.1f} x {h:.1f} pt ({w/72:.2f} x {h/72:.2f} in)")
    print("加密状态: 否")
    meta = reader.metadata
    if meta:
        for key in ("/Title", "/Author", "/Subject", "/Creator", "/Producer"):
            val = meta.get(key, "")
            if val:
                print(f"{key}: {val}")


def cmd_page_count(args: argparse.Namespace) -> None:
    from pypdf import PdfReader
    reader = PdfReader(args.pdf, strict=False)
    if reader.is_encrypted:
        print("PDF 已加密，请先解密", file=sys.stderr)
        sys.exit(1)
    print(len(reader.pages))


def cmd_extract_text(args: argparse.Namespace) -> None:
    if getattr(args, "fast", False):
        from pypdf import PdfReader
        reader = PdfReader(args.pdf, strict=False)
        if reader.is_encrypted:
            print("PDF 已加密，请先解密", file=sys.stderr)
            sys.exit(1)
        pages = _parse_page_set(args.pages) if args.pages else set()
        for i, page in enumerate(reader.pages, 1):
            if pages and i not in pages:
                continue
            text = page.extract_text() or ""
            print(f"--- 第 {i} 页 ---")
            print(text)
    else:
        import pdfplumber
        with pdfplumber.open(args.pdf) as pdf:
            pages = _parse_page_set(args.pages) if args.pages else set()
            for i, page in enumerate(pdf.pages, 1):
                if pages and i not in pages:
                    continue
                text = page.extract_text() or ""
                print(f"--- 第 {i} 页 ---")
                print(text)


def cmd_extract_table(args: argparse.Namespace) -> None:
    import json as json_mod

    import pdfplumber
    with pdfplumber.open(args.pdf) as pdf:
        page_idx = int(args.page) - 1 if args.page else 0
        if page_idx < 0 or page_idx >= len(pdf.pages):
            print(f"页码超出范围（共 {len(pdf.pages)} 页）", file=sys.stderr)
            sys.exit(1)
        tables = pdf.pages[page_idx].extract_tables()
        if not tables:
            print("未检测到表格")
            return
        for idx, table in enumerate(tables):
            if args.format == "csv":
                import csv
                import io
                buf = io.StringIO()
                csv.writer(buf).writerows(table)
                print(buf.getvalue())
            else:
                print(json_mod.dumps(table, ensure_ascii=False, indent=2))


def cmd_to_md(args: argparse.Namespace) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from pdf_to_md import parse
    parse(args.pdf, args.output)


def cmd_merge(args: argparse.Namespace) -> None:
    from pypdf import PdfReader, PdfWriter

    writer = PdfWriter()
    for src in args.pdfs:
        for page in PdfReader(src).pages:
            writer.add_page(page)
    _write_pdf(writer, args.output)
    print(f"已合并 {len(args.pdfs)} 个文件，保存为 {args.output}")


def cmd_split(args: argparse.Namespace) -> None:
    from pypdf import PdfWriter

    reader = _read_pdf(args.pdf)
    total = len(reader.pages)
    page_set = _parse_page_set(args.pages) if args.pages else set(range(1, total + 1))
    out_dir = Path(args.output)
    out_dir.mkdir(parents=True, exist_ok=True)
    saved = 0
    for i, page in enumerate(reader.pages, 1):
        if i not in page_set:
            continue
        w = PdfWriter()
        w.add_page(page)
        out_path = out_dir / f"page_{i:03d}.pdf"
        with open(out_path, "wb") as f:
            w.write(f)
        saved += 1
    print(f"已拆分 {saved} 页，保存到 {out_dir}/")


def cmd_delete_pages(args: argparse.Namespace) -> None:
    from pypdf import PdfWriter

    reader = _read_pdf(args.pdf)
    to_delete = _parse_page_set(args.pages)
    total = len(reader.pages)
    if len(to_delete) >= total:
        print("不能删除所有页面", file=sys.stderr)
        sys.exit(1)

    writer = PdfWriter()
    for i, page in enumerate(reader.pages, 1):
        if i not in to_delete:
            writer.add_page(page)
    _write_pdf(writer, args.output)
    print(f"已删除 {len(to_delete)} 页（保留 {total - len(to_delete)} 页），保存为 {args.output}")


def cmd_rotate(args: argparse.Namespace) -> None:
    from pypdf import PdfWriter

    reader = _read_pdf(args.pdf)
    writer = PdfWriter()
    target_pages = _parse_page_set(args.pages) if args.pages else set()

    for i, page in enumerate(reader.pages, 1):
        if not target_pages or i in target_pages:
            page.rotate(args.angle)
        writer.add_page(page)

    _write_pdf(writer, args.output)
    print(f"已旋转，保存为 {args.output}")


def cmd_crop(args: argparse.Namespace) -> None:
    from pypdf import PdfWriter

    reader = _read_pdf(args.pdf)
    writer = PdfWriter()
    target_pages = _parse_page_set(args.pages) if args.pages else set()
    parts = args.bbox.split(",")
    if len(parts) != 4:
        print("--bbox 格式错误，需要 x0,y0,x1,y1（4 个数字）", file=sys.stderr)
        sys.exit(1)
    x0, y0, x1, y1 = (float(v) for v in parts)

    for i, page in enumerate(reader.pages, 1):
        if not target_pages or i in target_pages:
            page.mediabox.left = x0
            page.mediabox.bottom = y0
            page.mediabox.right = x1
            page.mediabox.top = y1
        writer.add_page(page)

    _write_pdf(writer, args.output)
    print(f"已裁剪，保存为 {args.output}")


def cmd_watermark(args: argparse.Namespace) -> None:
    from pypdf import PdfReader, PdfWriter

    reader = _read_pdf(args.pdf)
    watermark_page = PdfReader(args.watermark).pages[0]
    writer = PdfWriter()
    for page in reader.pages:
        page.merge_page(watermark_page)
        writer.add_page(page)
    _write_pdf(writer, args.output)
    print(f"已添加水印，保存为 {args.output}")


def cmd_compress(args: argparse.Namespace) -> None:
    import os

    from pypdf import PdfWriter

    reader = _read_pdf(args.pdf)
    writer = PdfWriter()
    for page in reader.pages:
        writer.add_page(page)
    for page in writer.pages:
        page.compress_content_streams()
    writer.compress_identical_objects(remove_identicals=True, remove_orphans=True)
    _write_pdf(writer, args.output)
    orig_size = os.path.getsize(args.pdf)
    new_size = os.path.getsize(args.output)
    ratio = (1 - new_size / orig_size) * 100 if orig_size > 0 else 0
    print(f"已压缩 {orig_size} bytes 至 {new_size} bytes（减小 {ratio:.1f}%），保存为 {args.output}")


def cmd_encrypt(args: argparse.Namespace) -> None:
    from pypdf import PdfReader, PdfWriter

    reader = PdfReader(args.pdf)
    writer = PdfWriter()
    for page in reader.pages:
        writer.add_page(page)
    writer.encrypt(args.password)
    _write_pdf(writer, args.output)
    print(f"已加密，保存为 {args.output}")


def cmd_decrypt(args: argparse.Namespace) -> None:
    from pypdf import PdfReader, PdfWriter

    reader = PdfReader(args.pdf)
    if reader.is_encrypted:
        reader.decrypt(args.password)
    writer = PdfWriter()
    for page in reader.pages:
        writer.add_page(page)
    _write_pdf(writer, args.output)
    print(f"已解密，保存为 {args.output}")


def cmd_render(args: argparse.Namespace) -> None:
    import pypdfium2 as pdfium

    pdf = pdfium.PdfDocument(args.pdf)
    out_dir = Path(args.output)
    out_dir.mkdir(parents=True, exist_ok=True)
    pages = _parse_page_set(args.pages) if args.pages else set()
    scale = args.dpi / 72.0
    count = 0
    for i, page in enumerate(pdf, 1):
        if pages and i not in pages:
            continue
        bitmap = page.render(scale=scale)
        bitmap.to_pil().save(str(out_dir / f"page_{i:03d}.png"))
        count += 1
    print(f"已渲染 {count} 页（{args.dpi} DPI），保存到 {out_dir}/")


def cmd_extract_images(args: argparse.Namespace) -> None:
    from PIL import Image
    import io

    reader = _read_pdf(args.pdf)
    out_dir = Path(args.output)
    out_dir.mkdir(parents=True, exist_ok=True)
    count = 0
    for page_num, page in enumerate(reader.pages, 1):
        for img_idx, img_obj in enumerate(page.images):
            try:
                img = Image.open(io.BytesIO(img_obj.data))
                ext = img.format.lower() if img.format else "png"
                out_path = out_dir / f"p{page_num}_img{img_idx + 1}.{ext}"
                img.save(str(out_path))
                count += 1
            except Exception:
                continue
    print(f"已提取 {count} 张图片，保存到 {out_dir}/")


def cmd_batch(args: argparse.Namespace) -> None:
    """对目录下所有 PDF 批量执行某个子命令。"""
    import subprocess

    input_dir = Path(args.input_dir)
    if not input_dir.is_dir():
        print(f"目录不存在: {input_dir}", file=sys.stderr)
        sys.exit(1)

    pattern = args.pattern or "*.pdf"
    pdf_files = sorted(input_dir.glob(pattern))
    if not pdf_files:
        print(f"未找到匹配 '{pattern}' 的 PDF 文件", file=sys.stderr)
        sys.exit(1)

    output_dir = Path(args.output) if args.output else None
    if output_dir:
        output_dir.mkdir(parents=True, exist_ok=True)

    subcmd = args.subcmd
    extra_args = args.extra_args or []

    success, fail = 0, 0
    for pdf_file in pdf_files:
        stem = pdf_file.stem
        if output_dir:
            out_file = str(output_dir / f"{stem}.pdf")
            cmd_args = [sys.executable, str(Path(__file__).resolve()), subcmd, str(pdf_file), "-o", out_file] + extra_args
        else:
            cmd_args = [sys.executable, str(Path(__file__).resolve()), subcmd, str(pdf_file)] + extra_args

        result = subprocess.run(cmd_args, capture_output=True, text=True)
        if result.returncode == 0:
            success += 1
            if result.stdout.strip():
                print(f"  [OK] {pdf_file.name}: {result.stdout.strip()}")
            else:
                print(f"  [OK] {pdf_file.name}")
        else:
            fail += 1
            err_msg = result.stderr.strip().split("\n")[-1] if result.stderr.strip() else "未知错误"
            print(f"  [FAIL] {pdf_file.name}: {err_msg}")

    print(f"\n批处理完成: {success} 成功, {fail} 失败 (共 {success + fail} 个文件)")
    if fail > 0:
        sys.exit(1)


# ============================================================
# argparse 注册
# ============================================================

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="pdf_cli.py", description="PDF 统一操作入口")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("info", help="查看 PDF 基本信息")
    p.add_argument("pdf")

    p = sub.add_parser("page-count", help="获取总页数")
    p.add_argument("pdf")

    p = sub.add_parser("extract-text", help="提取文本")
    p.add_argument("pdf"); p.add_argument("--pages", default="")
    p.add_argument("--fast", action="store_true", help="使用 pypdf 快速模式")

    p = sub.add_parser("extract-table", help="提取表格")
    p.add_argument("pdf"); p.add_argument("--page", default="", help="页码（默认第 1 页）")
    p.add_argument("--format", default="json", choices=["json", "csv"])

    p = sub.add_parser("to-md", help="PDF 转 Markdown（含图片）")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)

    p = sub.add_parser("merge", help="合并多个 PDF")
    p.add_argument("pdfs", nargs="+"); p.add_argument("-o", "--output", required=True)

    p = sub.add_parser("split", help="将 PDF 拆分为单页文件")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--pages", default="", help="只拆分指定页码，如 1,3,5-7；不指定则全部拆分")

    p = sub.add_parser("delete-pages", help="删除指定页面")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--pages", required=True, help="要删除的页码，如 1,3,5-7")

    p = sub.add_parser("rotate", help="旋转 PDF 页面")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--pages", default="", help="页码范围，如 1,3,5-7；不指定则全部旋转")
    p.add_argument("--angle", type=int, default=90, choices=[90, 180, 270])

    p = sub.add_parser("crop", help="裁剪页面可视区域")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--bbox", required=True, help="裁剪区域 x0,y0,x1,y1（PDF 坐标系）")
    p.add_argument("--pages", default="", help="页码范围；不指定则全部裁剪")

    p = sub.add_parser("watermark", help="添加整页水印")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--watermark", required=True, help="水印 PDF 文件路径")

    p = sub.add_parser("compress", help="压缩 PDF 体积")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)

    p = sub.add_parser("encrypt", help="加密 PDF")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--password", required=True)

    p = sub.add_parser("decrypt", help="解密 PDF")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--password", required=True)

    p = sub.add_parser("render", help="渲染 PDF 为图片")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)
    p.add_argument("--pages", default=""); p.add_argument("--dpi", type=int, default=150)

    p = sub.add_parser("extract-images", help="提取嵌入图片")
    p.add_argument("pdf"); p.add_argument("-o", "--output", required=True)

    p = sub.add_parser("batch", help="对目录下所有 PDF 批量执行某个子命令")
    p.add_argument("input_dir", help="输入目录")
    p.add_argument("subcmd", help="要执行的子命令（如 compress, rotate 等）")
    p.add_argument("-o", "--output", default="", help="输出目录")
    p.add_argument("--pattern", default="*.pdf", help="文件匹配模式（默认 *.pdf）")
    p.add_argument("extra_args", nargs="*", help="传给子命令的额外参数")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    commands = {
        "info": cmd_info,
        "page-count": cmd_page_count,
        "extract-text": cmd_extract_text,
        "extract-table": cmd_extract_table,
        "to-md": cmd_to_md,
        "merge": cmd_merge,
        "split": cmd_split,
        "delete-pages": cmd_delete_pages,
        "rotate": cmd_rotate,
        "crop": cmd_crop,
        "watermark": cmd_watermark,
        "compress": cmd_compress,
        "encrypt": cmd_encrypt,
        "decrypt": cmd_decrypt,
        "render": cmd_render,
        "extract-images": cmd_extract_images,
        "batch": cmd_batch,
    }
    commands[args.command](args)


if __name__ == "__main__":
    main()
