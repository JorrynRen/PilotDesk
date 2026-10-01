"""
字体扫描公用模块 — 跨平台扫描系统已安装字体。

从 edit_operations.py 和 pptx_text.py 中提取合并，提供统一字体扫描能力：

- scan_system_fonts()     → 返回带分类的完整字体信息（名称、分类、文件路径）
- scan_system_font_names() → 仅返回字体名列表（支持按分类过滤）
- _classify_font()        → 判断字体属于 chinese / latin / other

依赖（按优先级）：
  1. matplotlib.font_manager（最快，直接返回可读名称）
  2. fontTools（跨平台，支持 TTC/OTC，解析字体元数据）
  3. 文件名回退（最不精确，仅当 1 和 2 都不可用时）
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import json
from functools import lru_cache
from typing import Any

# ── CJK 字符判断（与 edit_operations.py 保持一致） ──────────────────────────────

_CJK_RE = re.compile(
    r'[⺀-⻿⼀-⿟　-〿㇀-㇯'
    r'㈀-㋿㌀-㏿㐀-䶿一-鿿'
    r'豈-﫿︰-﹏\U00020000-\U0002a6df]'
)

_CJK_NAME_HINT_RE = re.compile(
    r"\b("
    r"pingfang|hiragino|songti|heiti|kaiti|yuanti|fangsong|stsong|stheiti|"
    r"microsoft\s*yahei|yahei|simsun|simhei|kaiti|fangsong|dengxian"
    r")\b",
    re.IGNORECASE,
)


def _classify_font(name: str) -> str:
    """判断字体属于 chinese / latin / other。

    基于字体名中是否包含 CJK 字符来判断，与 edit_operations.py 的 category 过滤逻辑一致。
    """
    if _CJK_RE.search(name) or _CJK_NAME_HINT_RE.search(name):
        return "chinese"
    return "latin"


# ── 字体目录（跨平台） ──────────────────────────────────────────────────────────

def _system_font_dirs() -> list[str]:
    """返回当前操作系统的常见字体目录列表。"""
    if sys.platform == 'win32':
        windir = os.environ.get('WINDIR', r'C:\Windows')
        localappdata = os.environ.get('LOCALAPPDATA', '')
        dirs = [
            os.path.join(windir, 'Fonts'),
        ]
        if localappdata:
            dirs.append(os.path.join(localappdata, 'Microsoft', 'Windows', 'Fonts'))
        return dirs
    elif sys.platform == 'darwin':
        return [
            '/Library/Fonts',
            '/System/Library/Fonts',
            os.path.expanduser('~/Library/Fonts'),
        ]
    else:
        # Linux / 其他 Unix
        return [
            '/usr/share/fonts',
            '/usr/local/share/fonts',
            os.path.expanduser('~/.fonts'),
            os.path.expanduser('~/.local/share/fonts'),
        ]


# ── fontTools 回退扫描 ─────────────────────────────────────────────────────────

def _register_font_names(tt, fpath: str, mapping: dict, ttc_index: int = 0):
    """从 TTFont 对象中读取所有 name 记录并写入 mapping。

    读取 nameID=1（字体家族名）和 nameID=4（完整字体名），
    将 {规范化名称: (文件路径, ttc_index)} 写入 mapping。
    """
    try:
        name_table = tt['name']
    except Exception:
        return

    collected_names: set[str] = set()
    for record in name_table.names:
        if record.nameID not in (1, 4):
            continue
        try:
            name_str = record.toUnicode()
        except Exception:
            continue
        if name_str:
            collected_names.add(name_str.strip())

    for name_str in collected_names:
        key = name_str.lower()
        if key not in mapping:
            mapping[key] = (fpath, ttc_index)


def _iter_font_files():
    """递归遍历系统字体目录中的字体文件路径。"""
    font_exts = {'.ttf', '.otf', '.ttc', '.otc'}
    for font_dir in _system_font_dirs():
        if not os.path.isdir(font_dir):
            continue
        for root, _, files in os.walk(font_dir):
            for fname in files:
                ext = os.path.splitext(fname)[1].lower()
                if ext in font_exts:
                    yield os.path.join(root, fname)


def _scan_with_fonttools() -> dict[str, tuple[str, int]]:
    """使用 fontTools 扫描系统字体目录，返回 {规范化字体名(小写): (文件路径, ttc_index)}。"""
    font_name_to_path: dict[str, tuple[str, int]] = {}

    try:
        from fontTools.ttLib import TTFont, TTCollection
    except ImportError:
        return font_name_to_path

    for fpath in _iter_font_files():
        ext = os.path.splitext(fpath)[1].lower()
        try:
            if ext in ('.ttc', '.otc'):
                collection = TTCollection(fpath)
                for i, tt in enumerate(collection.fonts):
                    _register_font_names(tt, fpath, font_name_to_path, ttc_index=i)
            else:
                tt = TTFont(fpath, fontNumber=0)
                _register_font_names(tt, fpath, font_name_to_path)
        except Exception:
            continue

    return font_name_to_path


def _scan_with_filename_fallback() -> list[str]:
    """最终回退：使用字体文件名（去掉扩展名）充当字体名。"""
    fonts: set[str] = set()
    for fpath in _iter_font_files():
        fonts.add(os.path.splitext(os.path.basename(fpath))[0])
    return sorted(fonts)


def _scan_with_system_profiler() -> set[str]:
    """macOS 专用：通过 system_profiler 获取系统已启用字体。"""
    if sys.platform != "darwin":
        return set()

    try:
        proc = subprocess.run(
            ["system_profiler", "SPFontsDataType", "-json"],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
        payload = json.loads(proc.stdout)
    except Exception:
        return set()

    names: set[str] = set()
    for item in payload.get("SPFontsDataType", []):
        for key in ("_name", "family", "fullname"):
            value = item.get(key)
            if isinstance(value, str) and value.strip():
                names.add(value.strip())
        for face in item.get("typefaces", []):
            if not isinstance(face, dict):
                continue
            for key in ("_name", "family", "fullname"):
                value = face.get(key)
                if isinstance(value, str) and value.strip():
                    names.add(value.strip())
    return names


# ── 统一扫描函数 ───────────────────────────────────────────────────────────────

@lru_cache(maxsize=1)
def _get_system_font_names_set() -> set[str]:
    """获取系统已安装字体的名称集合（缓存，仅扫描一次）。

    优先级：
      1. matplotlib.font_manager（最快，直接返回名称列表）
      2. fonttools（跨平台，支持 TTC，从字体元数据读取）
      3. 文件名回退（最不精确）
    """
    # macOS：优先用 system_profiler，能拿到 Font Book/系统保留字体的真实 family/fullname。
    if sys.platform == "darwin":
        names = _scan_with_system_profiler()
        if names:
            return names

    # 路径 1：matplotlib
    try:
        from matplotlib import font_manager as fm
        return {f.name for f in fm.fontManager.ttflist if f.name}
    except ImportError:
        pass

    # 路径 2：fonttools
    font_map = _scan_with_fonttools()
    if font_map:
        # font_map 的 key 是 lower() 后的名称，value 中的 key 是原始名称
        # 我们需要收集所有原始名称
        # 重建扫描收集原始名：由于 _register_font_names 只收集 lower→(path,index)
        # 我们在 _scan_with_fonttools 内部无法保留原始名，需要重新从 name table 收集
        raw_names: set[str] = set()
        for fpath in _iter_font_files():
            ext = os.path.splitext(fpath)[1].lower()
            try:
                if ext in ('.ttc', '.otc'):
                    from fontTools.ttLib import TTCollection
                    collection = TTCollection(fpath)
                    for tt in collection.fonts:
                        _collect_raw_names(tt, raw_names)
                else:
                    from fontTools.ttLib import TTFont
                    tt = TTFont(fpath, fontNumber=0)
                    _collect_raw_names(tt, raw_names)
            except Exception:
                continue
        if raw_names:
            return raw_names
        # fallback: 从 font_map 的 key 重建（不精确，但比没有好）
        return set(font_map.keys())

    # 路径 3：文件名回退
    return set(_scan_with_filename_fallback())


def _collect_raw_names(tt, raw_names: set[str]):
    """从 TTFont 对象中收集原始字体名称到集合。"""
    try:
        name_table = tt['name']
    except Exception:
        return
    for record in name_table.names:
        if record.nameID not in (1, 4):
            continue
        try:
            name_str = record.toUnicode()
        except Exception:
            continue
        if name_str:
            raw_names.add(name_str.strip())


# ── 公开 API ───────────────────────────────────────────────────────────────────


def scan_system_font_names(category: str | None = None) -> list[str]:
    """获取系统已安装字体的名称列表。

    Args:
        category: 可选过滤分类：
            - "chinese" / "zh": 仅返回含 CJK 字形的字体
            - "latin" / "en":   仅返回拉丁/英文字体
            - None（默认）:     返回全部字体

    Returns:
        排序后的字体名列表。
    """
    all_fonts = sorted(_get_system_font_names_set())

    norm_cat = (category or "").lower().strip()
    if norm_cat in ("chinese", "zh"):
        return [f for f in all_fonts if _classify_font(f) == "chinese"]
    elif norm_cat in ("latin", "en"):
        return [f for f in all_fonts if _classify_font(f) == "latin"]
    return all_fonts


def scan_system_fonts() -> dict[str, Any]:
    """扫描系统已安装字体，返回带分类的完整信息。

    Returns:
        {
            "fonts": [
                {"name": "微软雅黑", "category": "chinese"},
                {"name": "Arial", "category": "latin"},
                ...
            ],
            "total": 42,
            "error": None
        }
    """
    try:
        raw_names = _get_system_font_names_set()
        fonts = sorted(
            ({"name": name, "category": _classify_font(name)} for name in raw_names),
            key=lambda x: x["name"],
        )
        return {
            "fonts": fonts,
            "total": len(fonts),
            "error": None,
        }
    except Exception as e:
        return {
            "fonts": [],
            "total": 0,
            "error": str(e),
        }
