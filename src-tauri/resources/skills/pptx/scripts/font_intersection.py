"""
字体白名单交集模块 — 获取白名单 ∩ 系统已安装字体的结果。

提供给模型调用的友好接口：

- get_allowed_fonts() → 按分类组织字体名列表

依赖：
  - font_whitelist.json（白名单配置）
  - font_scanner.py（系统字体扫描）
"""

from __future__ import annotations

import json
import os
import re
from typing import Any

from font_scanner import scan_system_font_names

# 白名单交集为空时的保守回退字体（CSS 通用字体族，任何 HTML 渲染环境均可用）
_FALLBACK_FONTS: dict[str, list[str]] = {

    "chinese": [
      {
        "name": "微软雅黑",
        "alias": ["Microsoft YaHei", "Microsoft YaHei UI"]
      },
      { "name": "宋体", "alias": ["SimSun", "Songti SC"] },
      { "name": "黑体", "alias": ["SimHei", "Heiti SC"] },
      { "name": "楷体", "alias": ["KaiTi", "Kaiti SC"] },
      { "name": "仿宋", "alias": ["FangSong", "Fangsong"] },
      { "name": "华文宋体", "alias": ["STSong", "STSongti"] },
      { "name": "等线", "alias": ["DengXian"] }
    ]
}

_STYLE_SUFFIX_RE = re.compile(
    r"(?:"
    r"ultralight|thin|light|regular|medium|semibold|demibold|bold|heavy|black|"
    r"italic|oblique|condensed|display|text|ui|watch|default|w[0-9]+"
    r")$",
    re.IGNORECASE,
)


def _normalize_font_name(name: str) -> str:
    """将字体名规整为便于比对的 key。"""
    lowered = name.strip().lower().lstrip(".")
    return re.sub(r"[\s\-_]+", "", lowered)


def _build_font_keys(name: str) -> set[str]:
    """为同一字体名生成多组兼容 key，覆盖 family/fullname/weight 变体。"""
    base = _normalize_font_name(name)
    if not base:
        return set()

    keys = {base}
    trimmed = base
    while True:
        next_trimmed = _STYLE_SUFFIX_RE.sub("", trimmed)
        if next_trimmed == trimmed or not next_trimmed:
            break
        keys.add(next_trimmed)
        trimmed = next_trimmed
    return keys


def _load_whitelist() -> dict[str, list[dict[str, Any]]]:
    """从 font_whitelist.json 加载白名单字体。

    Returns:
        {分类名: [{"name": "字体名", "alias": [...]}, ...]}
        白名单文件找不到或格式异常时返回空 dict。
    """
    script_dir = os.path.dirname(os.path.abspath(__file__))
    candidates = [
        os.path.join(script_dir, "font_whitelist.json"),
    ]
    skill_path = os.environ.get("SKILL_PATH", "")
    if skill_path:
        candidates.append(os.path.join(skill_path, "pptx", "scripts", "font_whitelist.json"))

    for path in candidates:
        if os.path.isfile(path):
            try:
                with open(path, encoding="utf-8") as f:
                    data = json.load(f)
                raw = data.get("fonts", {})
                if not isinstance(raw, dict):
                    return {}
                return raw
            except (json.JSONDecodeError, OSError):
                continue

    return {}


def get_allowed_fonts() -> dict[str, Any]:
    """获取当前系统允许使用的字体列表（白名单 ∩ 系统已安装字体）。

    按白名单中的分类（chinese / western_sans_serif / western_serif 等）返回可用字体名列表。
    匹配时同时检查字体的主名称和别名，命中任一即视为可用。

    Returns:
        正常情况:
            {
                "chinese": ["微软雅黑", "宋体", "黑体"],
                "western_sans_serif": ["Arial", "Calibri", ...],
                "western_serif": ["Times New Roman", "Georgia", ...],
                "western_monospace": ["Consolas"],
                "western_display": ["Wingdings"]
            }

        白名单 ∩ 系统字体为空时（系统极度精简），返回保守回退:
            {
                "chinese": ["宋体", "黑体"],
                "western_sans_serif": ["Arial"],
                "western_serif": ["Times New Roman"],
                "western_monospace": ["Courier New"]
            }

        出错时:
            {"error": "font_whitelist.json 未找到"}
    """
    whitelist = _load_whitelist()
    if not whitelist:
        return {"error": "font_whitelist.json 未找到"}

    # 获取系统字体名集合（统一小写用于匹配）
    try:
        system_font_keys = set()
        for name in scan_system_font_names():
            system_font_keys.update(_build_font_keys(name))
    except Exception as e:
        return {"error": f"系统字体扫描失败: {e}"}

    categorized: dict[str, list[str]] = {}
    has_any = False

    for category, items in whitelist.items():
        available: list[str] = []
        for item in items:
            name = item["name"]
            aliases = item.get("alias", [])

            # 检查主名称和所有别名，任一匹配即视为可用
            all_names = [name] + aliases
            matched = False
            for candidate in all_names:
                if not candidate:
                    continue
                candidate_keys = _build_font_keys(candidate)
                if candidate_keys & system_font_keys:
                    matched = True
                    break
            if matched:
                available.append(name)
                has_any = True

        if available:
            categorized[category] = available

    # 交集为空时回退到保守字体集合
    if not has_any:
        return dict(_FALLBACK_FONTS)

    return categorized
