"""论文字数统计工具 — 解析写作中的 JS 源文件，提取 h.p() / h.h1() 等文本统计正文字数。

写作过程中 docx 尚未生成，能统计的只有 JS 源文件中的文本内容。
全部写完并 run_node_docx 导出后，也可解析 .docx 做最终校验。

用法（在 python_cell_exec 中）::

    import word_counter

    # 写作过程中 — 统计 JS 源文件
    word_counter.count('output/paper.js', target=10000, section_targets={
        '引言': 1500, '文献综述': 2500, '研究方法': 2000, '研究结果': 2000, '结论': 1000,
    })

    # 最终校验 — 统计已生成的 .docx
    word_counter.count('output/paper.docx', target=10000)
"""

from __future__ import annotations

import re
from pathlib import Path


# ── 字数计算 ────────────────────────────────────────────────

def _char_count(text: str) -> int:
    """中文按字符计，英文按单词计，与学术论文字数统计惯例一致。"""
    text = text.strip()
    if not text:
        return 0
    zh_chars = len(re.findall(r"[\u4e00-\u9fff\u3400-\u4dbf]", text))
    remaining = re.sub(r"[\u4e00-\u9fff\u3400-\u4dbf]", " ", text)
    en_words = len(remaining.split())
    return zh_chars + en_words


# ── JS 源文件解析 ──────────────────────────────────────────

_SKIP_SECTIONS = {
    "摘要", "abstract", "关键词", "keywords", "key words",
    "参考文献", "references", "bibliography",
    "致谢", "acknowledgements", "acknowledgments",
    "附录", "appendix", "appendices",
    "目  录", "目录", "table of contents",
}

# h.h1('...') / h.h2('...') / h.h3('...')
_HEADING_RE = re.compile(r"""h\.h([123])\(\s*(['"`])(.+?)\2""")

# h.p('...') 中的第一个字符串参数（单引号、双引号、反引号）
# 也匹配 h.p(`...`) 模板字面量
_PARAGRAPH_RE = re.compile(
    r"""h\.p\(\s*(['"`])((?:(?!\1)[\s\S])*?)\1"""
)

# h.bullet('...') / h.numbered('...')
_LIST_RE = re.compile(
    r"""h\.(?:bullet|numbered)\(\s*(['"`])((?:(?!\1)[\s\S])*?)\1"""
)


def _should_skip(title: str) -> bool:
    t = title.strip().lower()
    t = re.sub(r"^[一二三四五六七八九十百\d]+[、.．章节]\s*", "", t).strip()
    return t in _SKIP_SECTIONS or any(s in t for s in _SKIP_SECTIONS)


def _strip_inline_refs(text: str) -> str:
    """移除 [@key] 引用标记和 [N] 上标编号，不计入字数。"""
    text = re.sub(r"\[@[\w,\s]+\]", "", text)
    text = re.sub(r"\[\d+(?:[,，-]\d+)*\]", "", text)
    return text


def _parse_js(source: str) -> list[dict]:
    """从 JS 源码提取各章节及其文本字数。"""
    sections: list[dict] = []

    full_text = source

    # 先收集所有标题的行位置，用于按顺序切割
    heading_positions: list[tuple[int, str, int]] = []
    for m in _HEADING_RE.finditer(full_text):
        level = int(m.group(1))
        title = m.group(3)
        pos = m.start()
        heading_positions.append((pos, title, level))

    heading_positions.sort(key=lambda x: x[0])

    if not heading_positions:
        # 无标题结构，统计全部文本
        total = 0
        for m in _PARAGRAPH_RE.finditer(full_text):
            total += _char_count(_strip_inline_refs(m.group(2)))
        for m in _LIST_RE.finditer(full_text):
            total += _char_count(_strip_inline_refs(m.group(2)))
        return [{"title": "(全文)", "words": total}]

    # 按标题位置切割源码，分段统计
    for i, (pos, title, level) in enumerate(heading_positions):
        end_pos = heading_positions[i + 1][0] if i + 1 < len(heading_positions) else len(full_text)
        chunk = full_text[pos:end_pos]

        if _should_skip(title):
            continue

        words = 0
        for m in _PARAGRAPH_RE.finditer(chunk):
            words += _char_count(_strip_inline_refs(m.group(2)))
        for m in _LIST_RE.finditer(chunk):
            words += _char_count(_strip_inline_refs(m.group(2)))

        if sections and level > 1:
            # 二级/三级标题归入上一个一级章节
            sections[-1]["words"] += words
            sections[-1]["subsections"].append({"title": title, "words": words})
        else:
            sections.append({"title": title, "words": words, "subsections": []})

    return sections


# ── DOCX 解析（最终校验用）──────────────────────────────────

def _parse_docx(docx_path: str) -> list[dict]:
    """解析已生成的 .docx 文件，按章节统计字数。"""
    from docx import Document

    doc = Document(docx_path)
    sections: list[dict] = []
    current_title = "(前言/无标题)"
    current_words = 0
    in_body = False

    _ZH_HEADING = re.compile(
        r"^[一二三四五六七八九十百]+[、.．]\s*"
        r"|^第[一二三四五六七八九十百]+[章节]\s*"
        r"|^\d+[、.．]\s*(?!\d)"
    )
    _EN_HEADING = re.compile(
        r"^(?:\d+\.?\s+)"
        r"|^(?:Chapter\s+\d+)"
        r"|^(?:[IVX]+\.\s+)",
        re.IGNORECASE,
    )

    def is_heading(para) -> bool:
        style_name = (para.style.name or "").lower()
        if "heading" in style_name:
            return True
        text = para.text.strip()
        return bool(text and (_ZH_HEADING.match(text) or _EN_HEADING.match(text)))

    for para in doc.paragraphs:
        if is_heading(para):
            if in_body:
                sections.append({"title": current_title, "words": current_words, "subsections": []})
            current_title = para.text.strip()
            current_words = 0
            in_body = not _should_skip(current_title)
            continue
        if in_body:
            current_words += _char_count(para.text)

    if in_body and current_words > 0:
        sections.append({"title": current_title, "words": current_words, "subsections": []})

    return sections


# ── 写作目标规划 ────────────────────────────────────────────

def plan(target: int, section_targets: dict[str, int], refs: int = 0) -> dict:
    """在步骤 1 调用，打印写作目标规划。让模型在写作前就明确各章节字数要求。

    Parameters
    ----------
    target : int
        文体最低正文字数（如 10000）
    section_targets : dict
        各章节目标字数，如 {"引言": 1500, "文献综述": 2500, ...}
    refs : int
        参考文献最低数量（0 = 不检查）

    Returns
    -------
    dict
        {"actual_target": int, "sections": dict}
    """
    actual = int(target * 1.2)
    actual_sections = {k: int(v * 1.2) for k, v in section_targets.items()}
    allocated = sum(actual_sections.values())

    print("=" * 56)
    print("📋 论文写作目标规划")
    print("=" * 56)
    print(f"  文体最低字数: {target}")
    print(f"  实际目标 (×1.2): {actual}")
    if refs > 0:
        print(f"  参考文献最低: {refs} 篇")
    print("-" * 56)
    print("  各章节目标 (×1.2):")
    for title, words in actual_sections.items():
        print(f"    📝 {title}: {words} 字")
    print(f"  已分配: {allocated} / {actual}")
    if allocated < actual:
        print(f"  ⚠️ 差额 {actual - allocated} 字需在各章中补足")
    print("=" * 56)
    print("\n后续每写完一章，调用 word_counter.count() 核对进度。")

    return {"actual_target": actual, "sections": actual_sections}


# ── 字数统计 ────────────────────────────────────────────────

def count(
    file_path: str,
    target: int = 0,
    section_targets: dict[str, int] | None = None,
    current_section: str = "",
) -> dict:
    """统计字数并打印进度报告 + 下一步行动指令。自动识别 .js 或 .docx 文件。

    Parameters
    ----------
    file_path : str
        .js 源文件（写作过程中）或 .docx 文件（最终校验）
    target : int
        全文正文目标字数（0 = 不检查）
    section_targets : dict, optional
        各章节目标字数，key 为章节标题关键词（模糊匹配）
    current_section : str
        刚写完的章节名称（用于生成精准的下一步指令）

    Returns
    -------
    dict
        {"total": int, "sections": [...], "action": str, "deficient_sections": [...]}
    """
    p = Path(file_path).expanduser().resolve()
    if not p.exists():
        print(f"❌ 文件不存在: {p}")
        return {"total": 0, "sections": [], "action": "ERROR", "deficient_sections": []}

    section_targets = section_targets or {}

    if p.suffix.lower() == ".docx":
        sections = _parse_docx(str(p))
        source_label = "DOCX"
    else:
        source = p.read_text(encoding="utf-8")
        sections = _parse_js(source)
        source_label = "JS 源码"

    total = sum(s["words"] for s in sections)

    # 匹配章节目标
    deficient: list[str] = []
    for sec in sections:
        sec["target"] = 0
        sec["ok"] = True
        for key, tgt in section_targets.items():
            if key in sec["title"]:
                sec["target"] = tgt
                sec["ok"] = sec["words"] >= tgt * 0.8
                if not sec["ok"]:
                    deficient.append(
                        f"{sec['title']}({sec['words']}/{tgt}, 差{tgt - sec['words']}字)"
                    )
                break

    # 当前章节的达标情况
    current_ok = True
    current_info = ""
    if current_section:
        for sec in sections:
            if current_section in sec["title"]:
                current_ok = sec["ok"]
                if sec["target"] > 0:
                    current_info = f"{sec['words']}/{sec['target']}字"
                break

    # ── 打印报告 ──
    print("=" * 56)
    print(f"📊 论文字数统计 ({source_label}): {p.name}")
    print("=" * 56)

    for sec in sections:
        status = "✅" if sec["ok"] else "⚠️"
        line = f"  {status} {sec['title']}: {sec['words']} 字"
        if sec["target"] > 0:
            pct = sec["words"] / sec["target"] * 100
            line += f" (目标 {sec['target']}，{pct:.0f}%)"
        print(line)
        for sub in sec.get("subsections", []):
            print(f"       └ {sub['title']}: {sub['words']} 字")

    print("-" * 56)
    if target > 0:
        pct = total / target * 100
        ok = "✅" if total >= target else "⚠️ 不足"
        print(f"  正文总字数: {total} / {target} ({pct:.0f}%) {ok}")
        if total < target:
            print(f"  ⚠️ 还差 {target - total} 字")
    else:
        print(f"  正文总字数: {total}")

    # ── 下一步行动指令（强制模型遵循）──
    print()
    print("=" * 56)
    if current_section and not current_ok:
        action = "EXPAND_CURRENT"
        print(f"🚫 STOP — 当前章节「{current_section}」字数不足({current_info})")
        print(f"   ❯❯ 必须立即 append 补写「{current_section}」")
        print(f"   ❯❯ 补写后再次调用 word_counter.count() 核对")
        print(f"   ❯❯ 禁止跳到下一章")
    elif deficient:
        action = "EXPAND_DEFICIENT"
        print(f"⚠️ 以下章节字数不足，需要补写:")
        for d in deficient:
            print(f"   ❯❯ {d}")
        print(f"   ❯❯ 按顺序逐章补写，每章补写后再 count() 核对")
    else:
        action = "NEXT_SECTION"
        if current_section:
            print(f"✅ 「{current_section}」已达标，可以写下一章")
        else:
            print(f"✅ 已写章节全部达标")
        if target > 0 and total < target:
            print(f"   ❯❯ 总字数尚未达标({total}/{target})，继续写下一章")
        elif target > 0:
            print(f"   ❯❯ 总字数已达标，进入收尾阶段")
    print("=" * 56)

    return {
        "total": total,
        "sections": sections,
        "action": action,
        "deficient_sections": deficient,
    }
