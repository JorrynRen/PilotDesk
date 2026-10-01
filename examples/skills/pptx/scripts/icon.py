"""
Iconify 图标工具模块

提供图标搜索与下载功能，基于 Iconify API：
- 搜索图标：支持按关键词搜索，并自动过滤到白名单图标包
- 下载图标：支持指定尺寸与颜色，下载 SVG 文件到本地，禁止下载白名单外图标包

白名单图标包：
  tabler      (MIT)
  lucide      (ISC)
  heroicons   (MIT)
  phosphor    (MIT)
  mdi         (Apache 2.0)
"""

import os
import re
import urllib.request
import urllib.parse
import json
from typing import Optional


# ─────────────────────────────────────────────────────────────────────────────
# 白名单配置
# ─────────────────────────────────────────────────────────────────────────────

# 允许使用的图标包前缀及其许可证
ALLOWED_ICON_PACKS: dict[str, str] = {
    "tabler": "MIT",
    "lucide": "ISC",
    "heroicons": "MIT",
    "ph": "MIT",        # phosphor 图标包在 Iconify 中的前缀为 ph
    "mdi": "Apache 2.0",
}

# Iconify API 基础地址
_ICONIFY_API = "https://api.iconify.design"


# ─────────────────────────────────────────────────────────────────────────────
# 内部辅助函数
# ─────────────────────────────────────────────────────────────────────────────

def _get_icon_prefix(icon_id: str) -> str:
    """
    从图标 ID（格式：prefix:name）中提取图标包前缀。

    参数:
        icon_id: 图标 ID，例如 "tabler:home"

    返回:
        图标包前缀，例如 "tabler"

    抛出:
        ValueError: 当 icon_id 格式不正确时
    """
    parts = icon_id.split(":", 1)
    if len(parts) != 2 or not parts[0] or not parts[1]:
        raise ValueError(f"无效的图标 ID 格式：'{icon_id}'，期望格式为 'prefix:name'")
    return parts[0]


def _is_allowed(prefix: str) -> bool:
    """判断图标包前缀是否在白名单中"""
    return prefix in ALLOWED_ICON_PACKS


def _http_get(url: str, timeout: int = 10) -> bytes:
    """
    发送 HTTP GET 请求并返回响应体字节数据。

    参数:
        url: 请求地址
        timeout: 超时秒数

    返回:
        响应体的字节内容

    抛出:
        RuntimeError: 请求失败时
    """
    try:
        req = urllib.request.Request(
            url,
            headers={"User-Agent": "cooffice-pptx-icon/1.0"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read()
    except Exception as e:
        raise RuntimeError(f"HTTP 请求失败：{url}\n原因：{e}") from e


# ─────────────────────────────────────────────────────────────────────────────
# 公开函数
# ─────────────────────────────────────────────────────────────────────────────

def search_icons(
    query: str,
    limit: int = 20,
) -> list[dict]:
    """
    搜索 Iconify 图标，仅返回白名单图标包中的结果。

    调用 Iconify Search API，并自动过滤掉不在白名单内的图标包，
    适合 AI 用于选择合适的开源图标。

    参数:
        query: 搜索关键词，例如 "home"、"arrow-right"
        limit: 期望搜索返回的最大图标总数（含未过滤结果），默认 20。
               实际返回条数 ≤ limit，且只含白名单图标包中的图标。

    返回:
        图标信息列表，每项为一个字典：
        {
            "id":      str,   # 图标 ID，例如 "tabler:home"
            "prefix":  str,   # 图标包前缀，例如 "tabler"
            "name":    str,   # 图标名称，例如 "home"
            "pack_name": str, # 图标包显示名称，例如 "Tabler Icons"
            "license": str,   # 许可证，例如 "MIT"
        }

    示例:
        >>> results = search_icons("home")
        >>> for r in results:
        ...     print(r["id"], r["license"])
        tabler:home MIT
        lucide:home ISC
        ...

    抛出:
        RuntimeError: 网络请求失败时
    """
    # 构造搜索 URL，多请求一些以确保过滤后有足够结果
    fetch_limit = max(limit * 3, 60)
    params = urllib.parse.urlencode({
        "query": query,
        "limit": fetch_limit,
    })
    url = f"{_ICONIFY_API}/search?{params}"

    raw = _http_get(url)
    data = json.loads(raw.decode("utf-8"))

    icons_raw: list[str] = data.get("icons", [])
    collections: dict = data.get("collections", {})

    results: list[dict] = []
    for icon_id in icons_raw:
        try:
            prefix = _get_icon_prefix(icon_id)
        except ValueError:
            continue

        if not _is_allowed(prefix):
            continue

        name = icon_id.split(":", 1)[1]
        pack_info = collections.get(prefix, {})
        pack_name = pack_info.get("name", prefix)

        results.append({
            "id": icon_id,
            "prefix": prefix,
            "name": name,
            "pack_name": pack_name,
            "license": ALLOWED_ICON_PACKS[prefix],
        })

        if len(results) >= limit:
            break

    return results


def download_icon(
    icon_id: str,
    save_path: str,
    width: Optional[int] = None,
    height: Optional[int] = None,
    color: Optional[str] = None,
) -> str:
    """
    从 Iconify 下载 SVG 图标到本地文件，仅允许白名单内的图标包。

    参数:
        icon_id:   图标 ID，格式为 "prefix:name"，例如 "tabler:home"
        save_path: 保存路径，例如 "/tmp/home.svg" 或 "icons/home.svg"
                   若目录不存在，会自动创建。
        width:     SVG 宽度（像素），可选。仅指定一个时另一维度自动计算。
        height:    SVG 高度（像素），可选。
        color:     图标颜色，支持以下格式：
                   - 十六进制颜色（带或不带 #），例如 "#ff0000" 或 "ff0000"
                   - CSS 颜色名，例如 "red"、"blue"
                   仅对单色（无硬编码调色板）图标生效。可选。

    返回:
        已保存的 SVG 文件的绝对路径。

    抛出:
        PermissionError: 图标包不在白名单中时。
        ValueError:      icon_id 格式不正确时。
        RuntimeError:    网络请求失败或写文件失败时。

    示例:
        >>> path = download_icon("tabler:home", "icons/home.svg", height=24, color="#333333")
        >>> print(path)
        /abs/path/to/icons/home.svg

        >>> # 尝试下载非白名单图标包
        >>> download_icon("noto:home", "icons/home.svg")
        PermissionError: 禁止下载：图标包 'noto' 不在白名单中 ...
    """
    # ── 1. 校验图标包白名单 ──────────────────────────────────────────────────
    prefix = _get_icon_prefix(icon_id)
    if not _is_allowed(prefix):
        allowed_list = ", ".join(
            f"{k}({v})" for k, v in ALLOWED_ICON_PACKS.items()
        )
        raise PermissionError(
            f"禁止下载：图标包 '{prefix}' 不在白名单中。\n"
            f"允许的图标包：{allowed_list}"
        )

    # ── 2. 构造下载 URL ───────────────────────────────────────────────────────
    name = icon_id.split(":", 1)[1]
    params: dict = {}

    if width is not None:
        params["width"] = width
    if height is not None:
        params["height"] = height
    if color is not None:
        # 去掉可能存在的 # 前缀，再用 %23 编码，API 要求不含原始 #
        hex_color = color.lstrip("#")
        # 判断是否是十六进制颜色（3 或 6 位）
        if re.fullmatch(r"[0-9a-fA-F]{3}|[0-9a-fA-F]{6}", hex_color):
            params["color"] = f"%23{hex_color}"
        else:
            # CSS 颜色名直接传递
            params["color"] = color

    query_string = "&".join(f"{k}={v}" for k, v in params.items())
    svg_url = f"{_ICONIFY_API}/{prefix}/{name}.svg"
    if query_string:
        svg_url = f"{svg_url}?{query_string}"

    # ── 3. 下载 SVG ───────────────────────────────────────────────────────────
    svg_bytes = _http_get(svg_url)

    # ── 4. 保存文件 ───────────────────────────────────────────────────────────
    abs_path = os.path.abspath(save_path)
    dir_path = os.path.dirname(abs_path)
    if dir_path:
        os.makedirs(dir_path, exist_ok=True)

    try:
        with open(abs_path, "wb") as f:
            f.write(svg_bytes)
    except OSError as e:
        raise RuntimeError(f"写入文件失败：{abs_path}\n原因：{e}") from e

    return abs_path


def get_allowed_packs() -> list[dict]:
    """
    返回当前白名单图标包列表。

    返回:
        图标包信息列表，每项为：
        {
            "prefix":  str,  # 图标包前缀，例如 "tabler"
            "license": str,  # 许可证，例如 "MIT"
        }

    示例:
        >>> packs = get_allowed_packs()
        >>> for p in packs:
        ...     print(p["prefix"], p["license"])
        tabler MIT
        lucide ISC
        ...
    """
    return [
        {"prefix": prefix, "license": license_}
        for prefix, license_ in ALLOWED_ICON_PACKS.items()
    ]
