from __future__ import annotations

import json
import math
import re
import urllib.parse
from collections import Counter
from dataclasses import asdict, dataclass, field
from typing import Any

try:
    import requests
    from bs4 import BeautifulSoup
except ImportError as e:
    raise ImportError(
        f"web_style_extractor 依赖缺失，请先执行 pip install requests beautifulsoup4。原始错误：{e}"
    ) from e


# ── HTTP ─────────────────────────────────────────────────────────────────────
_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
        "AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/124.0.0.0 Safari/537.36"
    ),
    "Accept-Language": "zh-CN,zh;q=0.9,en;q=0.8",
}
_MAX_CSS_FETCH = 6
_CSS_FETCH_TIMEOUT = 6

# ── 颜色格式解析 ──────────────────────────────────────────────────────────────
_RE_HEX = re.compile(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b")
_RE_RGB = re.compile(r"rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})", re.I)
_RE_HSL = re.compile(r"hsla?\(\s*([\d.]+)\s*,\s*([\d.]+)%\s*,\s*([\d.]+)%", re.I)

# 颜色属性（只从明确的颜色 property 提取）
_RE_COLOR_PROP = re.compile(
    r"(?:color|background(?:-color)?|border(?:-\w+)?-color|fill|stroke"
    r"|outline-color|accent-color|caret-color)\s*:\s*([^;}{/\n]+)",
    re.IGNORECASE,
)

# 背景属性（用于判断页面背景深浅）
_RE_BG_PROP = re.compile(
    r"(?:body|html|:root)\s*\{[^}]*background(?:-color)?\s*:\s*([^;}{]+)",
    re.IGNORECASE | re.DOTALL,
)

# CSS 自定义属性（捕获所有 --var 及其完整值）
_RE_CSS_VAR = re.compile(r"--([\w-]+)\s*:\s*([^;}{]+?)(?:\s*;|\s*\})", re.DOTALL)

# ── 字体解析 ──────────────────────────────────────────────────────────────────
_RE_FONT_FACE = re.compile(r"@font-face\s*\{([^}]+)\}", re.DOTALL | re.IGNORECASE)
_RE_FONT_FAMILY_VAL = re.compile(r"font-family\s*:\s*([^;}{]+?)(?:\s*;|\s*\})", re.IGNORECASE | re.DOTALL)

# 选择器 → 规则块（用于按选择器提取 font-family）
_RE_RULE_BLOCK = re.compile(r"([^{}/]+)\{([^{}]*font-family[^{}]*)\}", re.IGNORECASE | re.DOTALL)

# ── Logo CSS 背景图 ───────────────────────────────────────────────────────────
_RE_CSS_BG_LOGO = re.compile(
    r"(?:\.logo|#logo|\.brand|\.site-logo|\.navbar-brand|\.header-logo"
    r"|\.logo-wrap|\.logo-area|\.site-mark|\.brand-logo"
    r"|header\s+img|nav\s+img)"
    r"[^{]*\{[^}]*background(?:-image)?\s*:\s*url\([\"']?([^\"')\s]+)[\"']?\)",
    re.IGNORECASE | re.DOTALL,
)

# ── 颜色过滤阈值 ──────────────────────────────────────────────────────────────
_MAX_COLORS = 6
_DEDUP_DIST = 35     # RGB 欧氏距离低于此值视为相似色，保留权重更高者
_BRIGHT_WHITE = 235
_BRIGHT_BLACK = 20
_SAT_MIN = 22        # 最低饱和度（max-min < 此值视为灰色）

# ── 颜色信号来源权重 ──────────────────────────────────────────────────────────
_W_META_THEME = 1000           # <meta name="theme-color">
_W_META_TILE = 900             # <meta name="msapplication-TileColor">
_W_VAR_PRIMARY = 500           # --primary/--brand/--main/--accent/--key 等命名
_W_VAR_SECONDARY = 300         # --secondary/--sub 等
_W_VAR_COLOR = 150             # 其他含颜色值的 CSS 变量
_W_INLINE_ZONE = 120           # header/nav 内联 style 中的颜色
_W_FREQ_HIGH = 80              # 在 CSS 规则中出现 ≥ 5 次
_W_FREQ_MID = 40               # 出现 2-4 次
_W_FREQ_LOW = 10               # 出现 1 次


# ── 数据模型 ──────────────────────────────────────────────────────────────────

@dataclass
class WebStyleSnapshot:
    """从网页 HTML/CSS 解析得到的样式快照（配色、字体、Logo 等）。"""

    primary_color: str | None = None
    """可信度最高的主色（#RRGGBB）"""

    colors: list[str] = field(default_factory=list)
    """主色与辅色列表（已去重、已过滤近白近黑低饱和、按可信度降序）"""

    bg_is_dark: bool | None = None
    """True = 页面偏深色背景；False = 偏浅色；None = 无法判断"""

    logo_url: str | None = None
    """Logo 或站点图标的完整 URL，未找到为 None"""

    heading_font: str | None = None
    """大标题字体（来自 h1/h2 选择器或 @font-face 推断）"""

    body_font: str | None = None
    """正文字体（来自 body 选择器）"""

    all_fonts: list[str] = field(default_factory=list)
    """页面声明的所有字体名（含 @font-face 声明，去重，供映射到 Web-safe 字体）"""

    color_vars: dict[str, str] = field(default_factory=dict)
    """颜色相关 CSS 自定义属性（--varname: #hex），供参考"""

    error: str | None = None
    """非 None 表示提取失败，内容为原因描述"""

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    def is_sufficient(self) -> bool:
        return self.error is None and (self.primary_color is not None or len(self.colors) >= 1)


# ── 入口函数 ──────────────────────────────────────────────────────────────────

def extract_web_page_styles(url: str, timeout: int = 10) -> WebStyleSnapshot:
    """
    访问目标网页，从 HTML 与 CSS 中提取配色、字体、Logo 等核心样式特征。

    Args:
        url:     页面地址（须含 http/https scheme）
        timeout: 主页面请求超时秒数

    Returns:
        WebStyleSnapshot；error 字段非 None 表示提取失败。

    Usage::

        import os, sys, json
        sys.path.insert(0, os.path.join(os.environ["SKILL_PATH"], "pptx", "scripts"))
        from web_style_extractor import extract_web_page_styles

        result = extract_web_page_styles("https://www.example.com")
        print(result)
    """
    try:
        resp = requests.get(url, headers=_HEADERS, timeout=timeout)
        resp.raise_for_status()
    except Exception as exc:
        return WebStyleSnapshot(error=f"页面请求失败：{exc}")

    soup = BeautifulSoup(resp.text, "html.parser")
    css_text = _collect_css(soup, url)
    snapshot = WebStyleSnapshot()

    # 颜色系统
    color_vars = _extract_color_vars(css_text)
    snapshot.color_vars = color_vars
    snapshot.colors, snapshot.bg_is_dark = _extract_colors(css_text, color_vars, soup)
    snapshot.primary_color = snapshot.colors[0] if snapshot.colors else None

    # Logo / 站点图标
    snapshot.logo_url = _extract_logo(soup, url, css_text)

    # 字体系统
    snapshot.heading_font, snapshot.body_font, snapshot.all_fonts = _extract_fonts(css_text)

    if not snapshot.is_sufficient():
        snapshot.error = "颜色提取结果不足，页面可能是 JS 动态渲染（SPA），静态 HTML 未含完整样式"

    result = json.dumps(snapshot.to_dict(), ensure_ascii=False, indent=2)
    if snapshot.logo_url:
        return f"{result}\n\nlogo_url:{snapshot.logo_url}，将该logo文件下载到工作区，供后续使用本地路径将logo写入封面 HTML 中"
    else:
        return result


# ── CSS 收集 ──────────────────────────────────────────────────────────────────

def _collect_css(soup: BeautifulSoup, base_url: str) -> str:
    """收集内联 <style> 块 + 最多 _MAX_CSS_FETCH 个外链 CSS 文本。"""
    parts: list[str] = [tag.get_text() for tag in soup.find_all("style")]

    fetched = 0
    for link in soup.find_all("link", rel=lambda r: r and "stylesheet" in r):
        if fetched >= _MAX_CSS_FETCH:
            break
        href = link.get("href", "")
        if not href or href.startswith("data:"):
            continue
        css_url = _to_abs(href, base_url)
        if not css_url:
            continue
        try:
            r = requests.get(css_url, headers=_HEADERS, timeout=_CSS_FETCH_TIMEOUT)
            if r.ok:
                parts.append(r.text)
                fetched += 1
        except Exception:
            pass

    return "\n".join(parts)


# ── 颜色系统 ──────────────────────────────────────────────────────────────────

def _extract_color_vars(css: str) -> dict[str, str]:
    """
    提取所有值为颜色格式的 CSS 自定义属性 (--varname: <color>)。
    支持 hex / rgb / hsl 格式。
    """
    result: dict[str, str] = {}
    for m in _RE_CSS_VAR.finditer(css):
        name, raw_val = m.group(1), m.group(2).strip()
        color = _parse_color(raw_val)
        if color:
            result[f"--{name}"] = color
    return result


def _extract_colors(
    css: str,
    color_vars: dict[str, str],
    soup: BeautifulSoup,
) -> tuple[list[str], bool | None]:
    """
    按权重收集品牌色候选，去重后返回 (sorted_colors, bg_is_dark)。

    权重来源（从高到低）：
      meta theme-color → 高优先 CSS 变量名 → 普通颜色变量
      → header/nav 内联样式 → CSS 属性出现频率
    """
    weighted: list[tuple[int, str]] = []   # (weight, #RRGGBB)

    def add(color_str: str, weight: int) -> None:
        c = _parse_color(color_str)
        if c and _is_brand_color(c):
            weighted.append((weight, c))

    # 1. <meta name="theme-color"> / <meta name="msapplication-TileColor">
    for meta in soup.find_all("meta", attrs={"name": re.compile(r"theme.?color", re.I)}):
        add(meta.get("content", ""), _W_META_THEME)
    for meta in soup.find_all("meta", attrs={"name": re.compile(r"msapplication-tilecolor", re.I)}):
        add(meta.get("content", ""), _W_META_TILE)

    # 2. CSS 自定义属性（按变量名语义分级）
    _primary_kw = re.compile(r"primary|brand|main|accent|key|highlight|core|theme", re.I)
    _secondary_kw = re.compile(r"secondary|sub|support|minor|aux", re.I)
    for var_name, hex_val in color_vars.items():
        if _primary_kw.search(var_name):
            add(hex_val, _W_VAR_PRIMARY)
        elif _secondary_kw.search(var_name):
            add(hex_val, _W_VAR_SECONDARY)
        else:
            add(hex_val, _W_VAR_COLOR)

    # 3. header / nav 内联 style 中的颜色
    _zone_selectors = [("header", None), ("nav", None),
                       (None, {"role": "banner"}),
                       (None, {"class": re.compile(r"\b(?:header|navbar|topbar)\b", re.I)})]
    for tag_name, attrs in _zone_selectors:
        el = soup.find(tag_name, attrs) if (tag_name or attrs) else None
        if not el:
            continue
        style = el.get("style", "")
        for m in _RE_COLOR_PROP.finditer(style):
            add(m.group(1).strip(), _W_INLINE_ZONE)

    # 4. CSS 规则属性 — 统计出现频率后加权
    freq: Counter[str] = Counter()
    for m in _RE_COLOR_PROP.finditer(css):
        c = _parse_color(m.group(1).strip())
        if c and _is_brand_color(c):
            freq[c] += 1
    for color, count in freq.items():
        if count >= 5:
            add(color, _W_FREQ_HIGH)
        elif count >= 2:
            add(color, _W_FREQ_MID)
        else:
            add(color, _W_FREQ_LOW)

    # 按权重降序排列，去除相似色（保留权重更高者），取前 _MAX_COLORS 个
    weighted.sort(key=lambda x: x[0], reverse=True)
    result: list[str] = []
    for _, color in weighted:
        if not any(_color_dist(color, existing) < _DEDUP_DIST for existing in result):
            result.append(color)
        if len(result) >= _MAX_COLORS:
            break

    # 5. 背景深浅判断（从 body/html/:root 的 background 属性读取）
    bg_is_dark: bool | None = None
    for m in _RE_BG_PROP.finditer(css):
        c = _parse_color(m.group(1).strip())
        if c:
            bg_is_dark = _is_dark(c)
            break

    return result, bg_is_dark


# ── Logo 提取 ─────────────────────────────────────────────────────────────────


def _rel_is_icon_link(rel: Any) -> bool:
    """判断 <link rel=...> 是否为站点图标类（icon / shortcut icon / apple-touch / mask 等）。"""
    if not rel:
        return False
    rel_list = rel if isinstance(rel, list) else str(rel).split()
    rl = [str(x).lower() for x in rel_list]
    if "apple-touch-icon" in rl or any(x.startswith("apple-touch") for x in rl):
        return True
    if "icon" in rl:
        return True
    if "shortcut" in rl and "icon" in rl:
        return True
    if any("mask-icon" in x or x == "fluid-icon" for x in rl):
        return True
    return False


def _extract_logo_from_link_icons(soup: BeautifulSoup, base_url: str) -> str | None:
    """
    优先从 <link rel="icon"|shortcut icon|apple-touch-icon|...> 提取。

    - 支持 data:image/...;base64,...（直接作为 logo_url 返回）
    - 支持相对路径与协议相对 URL（经 _to_abs）
    - 文档顺序：先取「非 apple-touch」的 icon / shortcut icon；若无则取 apple-touch-icon
    """
    apple_candidates: list[str] = []

    for link in soup.find_all("link"):
        rel = link.get("rel")
        if not _rel_is_icon_link(rel):
            continue

        href = (link.get("href") or "").strip()
        if not href:
            continue

        rel_list = rel if isinstance(rel, list) else str(rel).split()
        rl = [str(x).lower() for x in rel_list]
        is_apple = "apple-touch-icon" in rl or any(x.startswith("apple-touch") for x in rl)

        if href.startswith("data:image"):
            return href

        url = _to_abs(href, base_url)
        if not url:
            continue

        if is_apple:
            apple_candidates.append(url)
        else:
            return url

    if apple_candidates:
        return apple_candidates[0]
    return None


def _extract_logo(soup: BeautifulSoup, base_url: str, css: str = "") -> str | None:
    """
    多策略提取品牌 Logo，按可信度从高到低：

    1. <link rel="icon"|shortcut icon|apple-touch-icon|data:image>（站点声明的图标）
    2. <img> 综合打分（语义区域加分 + 关键词加分）
    3. CSS background-image（logo 相关选择器）
    4. <meta property="og:image">
    """

    logo = _extract_logo_from_link_icons(soup, base_url)
    if logo:
        return logo

    def score_img(img_tag: Any, zone_bonus: int = 0) -> int:
        src = img_tag.get("src", "")
        alt = img_tag.get("alt", "").lower()
        cls = " ".join(img_tag.get("class", [])).lower()
        img_id = img_tag.get("id", "").lower()
        combined = f"{alt} {cls} {img_id} {src.lower()}"
        s = zone_bonus
        for kw, pts in (("logo", 10), ("brand", 5), ("mark", 3), ("icon", 2), ("site", 1)):
            if kw in combined:
                s += pts
        return s

    # ── 策略 2：<img> 打分 ──────────────────────────────────────────────────
    candidates: list[tuple[int, str]] = []
    seen: set[str] = set()

    zone_elements: list[tuple[Any, int]] = []
    if el := soup.find("header"):
        zone_elements.append((el, 6))
    if el := soup.find("nav"):
        zone_elements.append((el, 4))
    for el in soup.find_all(attrs={"role": "banner"}):
        zone_elements.append((el, 6))
    for el in soup.find_all(
        class_=re.compile(r"\b(?:header|navbar|nav-bar|topbar|site-header|page-header)\b", re.I)
    ):
        zone_elements.append((el, 3))

    for zone_el, bonus in zone_elements:
        for img in zone_el.find_all("img"):
            src = img.get("src", "")
            if not src or src.startswith("data:") or src in seen:
                continue
            seen.add(src)
            candidates.append((score_img(img, zone_bonus=bonus), src))

    for img in soup.find_all("img"):
        src = img.get("src", "")
        if not src or src.startswith("data:") or src in seen:
            continue
        s = score_img(img)
        if s > 0:
            seen.add(src)
            candidates.append((s, src))

    if candidates:
        candidates.sort(key=lambda x: x[0], reverse=True)
        url = _to_abs(candidates[0][1], base_url)
        if url:
            return url

    # ── 策略 3：CSS background-image ────────────────────────────────────────
    if css:
        for m in _RE_CSS_BG_LOGO.finditer(css):
            url = _to_abs(m.group(1), base_url)
            if url:
                return url

    # ── 策略 4：og:image ────────────────────────────────────────────────────
    og = soup.find("meta", attrs={"property": "og:image"})
    if og:
        url = _to_abs(og.get("content", ""), base_url)
        if url:
            return url

    return None


# ── 字体系统 ──────────────────────────────────────────────────────────────────

def _extract_fonts(css: str) -> tuple[str | None, str | None, list[str]]:
    """
    返回 (heading_font, body_font, all_fonts)。

    提取策略：
    1. @font-face 声明 → 收集页面实际加载的所有字体名
    2. 选择器规则 → h1/h2/h3 对应 heading_font，body/p/:root 对应 body_font
    3. 兜底：全文 font-family 出现顺序取前两个
    """

    def clean(raw: str) -> str:
        return raw.strip().split(",")[0].strip().strip("'\"").strip()

    # 1. 从 @font-face 提取所有已声明字体名
    face_fonts: list[str] = []
    for block_m in _RE_FONT_FACE.finditer(css):
        fm = _RE_FONT_FAMILY_VAL.search(block_m.group(1))
        if fm:
            name = clean(fm.group(1))
            if name and name not in face_fonts:
                face_fonts.append(name)

    # 2. 从选择器规则提取角色化字体
    _heading_re = re.compile(r"\bh[123]\b", re.I)
    _body_re = re.compile(r"\bbody\b|\bp\b|:root", re.I)

    heading: str | None = None
    body: str | None = None
    all_in_rules: list[str] = []

    for rule_m in _RE_RULE_BLOCK.finditer(css):
        selector = rule_m.group(1).strip()
        block = rule_m.group(2)
        fm = _RE_FONT_FAMILY_VAL.search(block)
        if not fm:
            continue
        name = clean(fm.group(1))
        if not name:
            continue
        if name not in all_in_rules:
            all_in_rules.append(name)
        if not heading and _heading_re.search(selector):
            heading = name
        if not body and _body_re.search(selector):
            body = name

    # 3. 兜底：全文按出现顺序
    all_global: list[str] = []
    for fm in _RE_FONT_FAMILY_VAL.finditer(css):
        name = clean(fm.group(1))
        if name and name not in all_global:
            all_global.append(name)
    if not heading and all_global:
        heading = all_global[0]
    if not body and len(all_global) > 1:
        body = all_global[1]

    # all_fonts: @font-face 声明优先，再补入规则中出现过的
    seen: set[str] = set(face_fonts)
    all_fonts = list(face_fonts)
    for f in all_in_rules + all_global:
        if f not in seen:
            seen.add(f)
            all_fonts.append(f)

    return heading, body, all_fonts[:12]


# ── 颜色工具函数 ──────────────────────────────────────────────────────────────

def _parse_color(value: str) -> str | None:
    """
    解析任意颜色字符串 → #RRGGBB，失败返回 None。
    支持 hex3/hex6/hex8、rgb/rgba、hsl/hsla。
    """
    value = value.strip()

    # hex
    m = _RE_HEX.search(value)
    if m:
        h = m.group(1)
        if len(h) == 3:
            h = h[0] * 2 + h[1] * 2 + h[2] * 2
        if len(h) == 8:   # hex8 → 去掉 alpha
            h = h[:6]
        try:
            int(h, 16)
        except ValueError:
            return None
        return f"#{h.upper()}"

    # rgb / rgba
    m = _RE_RGB.search(value)
    if m:
        r, g, b = int(m.group(1)), int(m.group(2)), int(m.group(3))
        if all(0 <= v <= 255 for v in (r, g, b)):
            return f"#{r:02X}{g:02X}{b:02X}"

    # hsl / hsla
    m = _RE_HSL.search(value)
    if m:
        h, s, l = float(m.group(1)), float(m.group(2)), float(m.group(3))
        return _hsl_to_hex(h, s, l)

    return None


def _hsl_to_hex(h: float, s: float, l: float) -> str:
    """HSL (h∈[0,360], s/l∈[0,100]) → #RRGGBB。"""
    s /= 100
    l /= 100
    c = (1 - abs(2 * l - 1)) * s
    x = c * (1 - abs((h / 60) % 2 - 1))
    m = l - c / 2
    if h < 60:
        r, g, b = c, x, 0.0
    elif h < 120:
        r, g, b = x, c, 0.0
    elif h < 180:
        r, g, b = 0.0, c, x
    elif h < 240:
        r, g, b = 0.0, x, c
    elif h < 300:
        r, g, b = x, 0.0, c
    else:
        r, g, b = c, 0.0, x
    return f"#{round((r + m) * 255):02X}{round((g + m) * 255):02X}{round((b + m) * 255):02X}"


def _is_brand_color(hex_color: str) -> bool:
    """过滤近白、近黑、低饱和灰，只保留有辨识度的品牌色。"""
    h = hex_color.lstrip("#")
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    brightness = (r + g + b) / 3
    saturation = max(r, g, b) - min(r, g, b)
    return not (brightness > _BRIGHT_WHITE or brightness < _BRIGHT_BLACK or saturation < _SAT_MIN)


def _is_dark(hex_color: str) -> bool:
    h = hex_color.lstrip("#")
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    return (r * 299 + g * 587 + b * 114) / 1000 < 128


def _color_dist(c1: str, c2: str) -> float:
    """两个 #RRGGBB 颜色的 RGB 欧氏距离。"""
    h1, h2 = c1.lstrip("#"), c2.lstrip("#")
    return math.sqrt(sum((int(h1[i:i+2], 16) - int(h2[i:i+2], 16)) ** 2 for i in (0, 2, 4)))


def _to_abs(src: str, base_url: str) -> str | None:
    """将相对/协议相对 URL 转为绝对 URL；无效则返回 None。"""
    if not src or src.startswith("data:"):
        return None
    if src.startswith("//"):
        scheme = urllib.parse.urlparse(base_url).scheme or "https"
        src = f"{scheme}:{src}"
    try:
        return urllib.parse.urljoin(base_url, src)
    except Exception:
        return None


if __name__ == "__main__":
    import sys

    target = sys.argv[1] if len(sys.argv) > 1 else "https://www.wps.cn"
    result = extract_web_page_styles(target)
    print(json.dumps(result.to_dict(), ensure_ascii=False, indent=2))
