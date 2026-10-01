"""
Transcoder API 客户端 — 布局检测 & PPTX 转换 & 截图（异步版本）

服务端 office-transcoder 已从同步接口迁移至异步任务队列。
所有耗时操作（布局检测、PPTX 转换、截图）均通过异步任务提交 + 轮询完成。

用法：
    # 底层异步 API（供模型直接调用）
    task_id = submit_check_layout(["slide.html"], mode="full")
    task_id = submit_html_to_pptx(["slide1.html", "slide2.html"])
    task_id = submit_screenshot(["slide.html"])
    status = query_task(task_id)
    cancel_task(task_id)

    # 高层自动轮询（CLI 使用，内部自动 submit → poll → 获取结果）
    result = check_layout(["slide.html"], mode="full")
    result = html_to_pptx(["slide1.html", "slide2.html"], "output.pptx")
    result = screenshot(["slide.html"], output_dir="./screenshots")
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any

import requests

# ── 常量 ──────────────────────────────────────────────────────────────

BASE_URL = "https://lingxi.wps.cn"
POLL_INTERVAL = 3  # 轮询间隔（秒）
DEFAULT_TIMEOUT = 600  # 任务超时（秒），与服务端 10 分钟一致
MAX_LAYOUT_CHECK_COUNT = 5  # 每个 PPT 项目最多 5 次布局检测

# HTML 中资源引用正则
_RE_IMG_SRC = re.compile(
    r'(<img\b[^>]*\bsrc\s*=\s*)(?:"([^"]*)"|\'([^\']*)\'|([^\s>]+))',
    re.IGNORECASE | re.DOTALL,
)
_RE_LINK_HREF = re.compile(
    r'(<link\b[^>]*\bhref\s*=\s*)(?:"([^"]*)"|\'([^\']*)\'|([^\s>]+))',
    re.IGNORECASE | re.DOTALL,
)
_RE_CSS_URL = re.compile(
    r"""(\burl\s*\(\s*)(['"]?)([^)'"]+)\2\s*\)""",
    re.IGNORECASE,
)

_PPTX_MIME = "application/vnd.openxmlformats-officedocument.presentationml.presentation"
_ZIP_MIME = "application/zip"

# 短链前缀 — 转换服务内部仅支持 HTTP 协议请求
_SHORT_LINK_PREFIX = "https://lingxi.wps.cn/api/aioffice/v1/short_link/"
_SHORT_LINK_PREFIX_HTTP = "http://lingxi.wps.cn/api/aioffice/v1/short_link/"


# ── 工具函数 ──────────────────────────────────────────────────────────


def _get_wps_sid() -> str:
    """获取 wps_sid：从环境变量 TMP_LX_UUID 获取。

    Raises:
        ValueError: 当环境变量 TMP_LX_UUID 未设置或为空时。
    """
    sid = os.environ.get("TMP_LX_UUID", "").strip()
    if not sid:
        raise ValueError("环境变量 TMP_LX_UUID 未设置，请设置后使用")
    return sid


def _is_local_resource(path: str) -> bool:
    """判断路径是否指向本地资源（非远程 URL 或内联数据）。"""
    if not path:
        return False
    if path.startswith(("#", "data:", "javascript:", "blob:")):
        return False
    if path.startswith(("http://", "https://", "//")):
        return False
    return True


def _parse_resource_path(path: str) -> Path | None:
    """将资源路径统一转换为 Path 对象，支持 file:// 协议和多种路径格式。

    支持的格式：
      - 相对路径: images/foo.png
      - 系统绝对路径: /home/user/foo.png, C:\\Users\\foo.png, C:/Users/foo.png
      - file URI (空 host): file:///home/user/foo.png, file:///C:/Users/foo.png
      - file URI (pipe 符号): file:///C|/Users/foo.png
      - file URI (单斜杠): file:/C:/Users/foo.png
      - file URI (explicit localhost): file://localhost/C:/Users/foo.png
    """
    if not path:
        return None

    # 处理 file:// 协议
    if path.startswith("file://"):
        local = path[7:]  # strip "file://"
        # 标准格式 file:///path → local = "/path"
        # 但可能碰到 file://localhost/path
        if local.startswith("localhost/"):
            local = local[len("localhost/"):]
        # 处理 Windows pipe 符号: file:///C|/path → C:/path
        if len(local) >= 2 and local[1] == "|":
            local = local[0] + ":" + local[2:]
        # Windows: file:///C:/path → local = "/C:/path"，Path.is_absolute() 不认带前导 / 的盘符路径
        if len(local) >= 3 and local[0] in ("/", "\\") and local[2] == ":":
            local = local[1:]
        return Path(local)

    # file:/ 协议（单斜杠变体，较少见但 RFC 3986 允许）
    if path.startswith("file:/"):
        local = path[6:]
        if len(local) >= 3 and local[0] in ("/", "\\") and local[2] == ":":
            local = local[1:]
        return Path(local)

    return Path(path)


def _resolve_resource_path(path: str, base_dir: Path) -> Path | None:
    """将 HTML 中的资源路径解析为本地文件路径，不存在时返回 None。"""
    p = _parse_resource_path(path)
    if p is None:
        return None
    if p.is_absolute():
        return p if p.is_file() else None
    candidate = base_dir / p
    return candidate if candidate.is_file() else None


def _resolve_flat_name(local_path: Path, name_registry: dict[str, Path]) -> str:
    """为本地资源文件生成唯一的扁平化文件名。

    所有资源文件与 HTML 处于同一级目录，避免服务端遇到 ../ 等跨目录路径。

    策略：
      1. 优先用文件名本身（如 logo.png）
      2. 冲突时，从父目录逐层向前缀拼（如 assets_logo.png）
      3. 极端情况加数字后缀（如 assets_logo_1.png）

    name_registry 同时承担"已分配名称 → 本地路径"的映射职责，
    相同本地路径返回相同名称，天然去重。
    """
    # 如果这个本地路径已经分配过名字，直接复用
    for name, path in name_registry.items():
        if path == local_path:
            return name

    # 尝试用纯文件名
    name = local_path.name
    if name not in name_registry:
        name_registry[name] = local_path
        return name

    # 冲突：从后往前逐层拼父目录名
    parts = local_path.parts
    for depth in range(2, len(parts) + 1):
        candidate = "_".join(parts[-depth:])
        if candidate not in name_registry:
            name_registry[candidate] = local_path
            return candidate

    # 极端情况：所有层级组合都冲突，加数字后缀
    base = "_".join(parts)
    counter = 1
    while f"{base}_{counter}" in name_registry:
        counter += 1
    candidate = f"{base}_{counter}"
    name_registry[candidate] = local_path
    return candidate


def scan_html_resources(
    html_content: str,
    html_dir: Path,
    name_registry: dict[str, Path] | None = None,
) -> tuple[str, dict[str, Path]]:
    """
    扫描 HTML 中的本地资源引用，将所有路径改写为扁平化文件名。

    改写的资源路径不含任何目录层级，所有资源视为与 HTML 同级。
    跨 HTML 文件共享 name_registry 以自动处理命名冲突。

    同时将短链 URL（https://lingxi.wps.cn/api/aioffice/v1/short_link/…）
    的协议从 https 改写为 http，因为转换服务内部仅支持 HTTP 请求短链。

    返回：
        (rewritten_html, {flat_name: local_path})
    """
    if name_registry is None:
        name_registry = {}

    # 将短链 https → http（转换服务内部只能走 HTTP）
    html_content = html_content.replace(_SHORT_LINK_PREFIX, _SHORT_LINK_PREFIX_HTTP)
    ref_files: dict[str, Path] = {}

    def _replace_img(m: re.Match) -> str:
        pre = m.group(1)
        src = m.group(2) or m.group(3) or m.group(4) or ""
        if not _is_local_resource(src):
            return m.group(0)
        local = _resolve_resource_path(src, html_dir)
        if local is None:
            return m.group(0)
        flat_name = _resolve_flat_name(local, name_registry)
        ref_files[flat_name] = local
        return f'{pre}"{flat_name}"'

    html_content = _RE_IMG_SRC.sub(_replace_img, html_content)

    def _replace_link(m: re.Match) -> str:
        pre = m.group(1)
        href = m.group(2) or m.group(3) or m.group(4) or ""
        if not _is_local_resource(href):
            return m.group(0)
        local = _resolve_resource_path(href, html_dir)
        if local is None:
            return m.group(0)
        flat_name = _resolve_flat_name(local, name_registry)
        ref_files[flat_name] = local
        return f'{pre}"{flat_name}"'

    html_content = _RE_LINK_HREF.sub(_replace_link, html_content)

    def _replace_css_url(m: re.Match) -> str:
        pre = m.group(1)
        inner = m.group(3).strip()
        if not _is_local_resource(inner):
            return m.group(0)
        local = _resolve_resource_path(inner, html_dir)
        if local is None:
            return m.group(0)
        flat_name = _resolve_flat_name(local, name_registry)
        ref_files[flat_name] = local
        return f'{pre}"{flat_name}")'

    html_content = _RE_CSS_URL.sub(_replace_css_url, html_content)

    return html_content, ref_files


def _build_headers() -> dict[str, str]:
    """构建通用请求头。"""
    return {"Connection": "keep-alive"}


def _build_cookies() -> dict[str, str]:
    """构建通用 Cookie。"""
    return {"wps_sid": _get_wps_sid()}


def _check_wps_sid() -> str | None:
    """检查 wps_sid 是否可用，不可用返回错误信息。"""
    try:
        _get_wps_sid()
        return None
    except ValueError as e:
        return str(e)


def _parse_api_response(resp: requests.Response) -> dict[str, Any]:
    """解析标准 API JSON 响应，统一返回格式。

    只提取业务数据字段，过滤掉 api_version、request_id 等元数据。
    """
    body = resp.json()
    v7code = body.get("v7code", -1)
    if v7code != 0 or body.get("result") != "ok":
        return {"error": body.get("error", f"API returned v7code={v7code}")}
    data = body.get("data")
    if isinstance(data, dict):
        # 过滤掉元数据字段
        meta_keys = {"api_version", "request_id", "server_time", "trace_id", "apiVersion", "requestId"}
        return {k: v for k, v in data.items() if k not in meta_keys}
    if isinstance(data, str):
        return {"task_id": data}
    return {}


# ── 辅助：构建 multipart 请求 ──────────────────────────────────────


def _build_layout_check_multipart(
    html_paths: list[str],
    mode: str,
) -> tuple[dict[str, Any], dict[str, Any], list[Any]]:
    """
    构建布局检测的 multipart 请求体。

    返回：
        (data, files, file_handles)
        调用方负责关闭 file_handles。
    """
    files: dict[str, Any] = {}
    data: dict[str, Any] = {}
    _file_handles: list[Any] = []
    name_registry: dict[str, Path] = {}

    if mode == "static" and len(html_paths) == 1:
        html_content, ref_files = scan_html_resources(
            Path(html_paths[0]).read_text(encoding="utf-8"),
            Path(html_paths[0]).parent,
            name_registry=name_registry,
        )
        slides_data = {
            "slides": [
                {"content": html_content, "name": Path(html_paths[0]).name}
            ]
        }
        data["slides"] = json.dumps(slides_data, ensure_ascii=False)
        for ref_path, local_path in ref_files.items():
            fh = open(local_path, "rb")
            _file_handles.append(fh)
            files[f"ref:{ref_path}"] = (ref_path, fh, "application/octet-stream")
    else:
        data["count"] = str(len(html_paths))
        all_refs: dict[str, Path] = {}

        for i, hp in enumerate(html_paths):
            html_content, ref_files = scan_html_resources(
                Path(hp).read_text(encoding="utf-8"),
                Path(hp).parent,
                name_registry=name_registry,
            )
            files[f"html.{i}"] = (
                Path(hp).name,
                html_content.encode("utf-8"),
                "text/html; charset=utf-8",
            )
            for k, v in ref_files.items():
                all_refs[k] = v

        for ref_path, local_path in all_refs.items():
            fh = open(local_path, "rb")
            _file_handles.append(fh)
            files[f"ref:{ref_path}"] = (ref_path, fh, "application/octet-stream")

    return data, files, _file_handles


def _build_pptx_convert_multipart(
    html_paths: list[str],
) -> tuple[str, dict[str, Any], dict[str, Any], list[Any]]:
    """
    构建 PPTX 转换的 multipart 请求体，自动判断单页/批量。

    返回：
        (endpoint_suffix, data, files, file_handles)
        endpoint_suffix: "convert/async" 或 "convert/batch/async"
    """
    files: dict[str, Any] = {}
    data: dict[str, Any] = {}
    _file_handles: list[Any] = []

    is_batch = len(html_paths) > 1
    endpoint = "convert/batch/async" if is_batch else "convert/async"
    all_refs: dict[str, Path] = {}
    name_registry: dict[str, Path] = {}

    if is_batch:
        data["count"] = str(len(html_paths))
        for i, hp in enumerate(html_paths):
            html_content, ref_files = scan_html_resources(
                Path(hp).read_text(encoding="utf-8"),
                Path(hp).parent,
                name_registry=name_registry,
            )
            files[f"html.{i}"] = (
                Path(hp).name,
                html_content.encode("utf-8"),
                "text/html; charset=utf-8",
            )
            for k, v in ref_files.items():
                all_refs[k] = v
    else:
        html_content, ref_files = scan_html_resources(
            Path(html_paths[0]).read_text(encoding="utf-8"),
            Path(html_paths[0]).parent,
            name_registry=name_registry,
        )
        files["html"] = (
            Path(html_paths[0]).name,
            html_content.encode("utf-8"),
            "text/html; charset=utf-8",
        )
        all_refs.update(ref_files)

    for ref_path, local_path in all_refs.items():
        fh = open(local_path, "rb")
        _file_handles.append(fh)
        files[f"ref:{ref_path}"] = (ref_path, fh, "application/octet-stream")

    return endpoint, data, files, _file_handles


def _build_screenshot_multipart(
    html_paths: list[str],
    viewport_width: int = 1280,
    viewport_height: int = 720,
    load_timeout: int = 15,
    visual_ready_timeout: int = 8,
) -> tuple[dict[str, Any], dict[str, Any], list[Any]]:
    """
    构建截图任务的 multipart 请求体。

    使用 slides JSON 格式（与布局检测的 slides 格式一致）。

    返回：
        (data, files, file_handles)
    """
    files: dict[str, Any] = {}
    data: dict[str, Any] = {}
    _file_handles: list[Any] = []

    slides_list = []
    all_refs: dict[str, Path] = {}
    name_registry: dict[str, Path] = {}

    for i, hp in enumerate(html_paths):
        html_content, ref_files = scan_html_resources(
            Path(hp).read_text(encoding="utf-8"),
            Path(hp).parent,
            name_registry=name_registry,
        )
        slides_list.append({"content": html_content, "name": Path(hp).name})
        for k, v in ref_files.items():
            all_refs[k] = v

    data["slides"] = json.dumps({"slides": slides_list}, ensure_ascii=False)
    data["viewport_width"] = str(viewport_width)
    data["viewport_height"] = str(viewport_height)
    data["load_timeout"] = str(load_timeout)
    data["visual_ready_timeout"] = str(visual_ready_timeout)

    for ref_path, local_path in all_refs.items():
        fh = open(local_path, "rb")
        _file_handles.append(fh)
        files[f"ref:{ref_path}"] = (ref_path, fh, "application/octet-stream")

    return data, files, _file_handles


def _cleanup_file_handles(handles: list[Any]) -> None:
    """关闭所有打开的文件句柄。"""
    for fh in handles:
        try:
            fh.close()
        except Exception:
            pass


# ── 布局检测次数限制 ────────────────────────────────────────────────


def _get_ppt_project_dir(html_paths: list[str]) -> Path | None:
    """从 HTML 文件路径推导项目目录（计数文件存放位置）。

    取所有 HTML 文件的公共父目录。例如：
        output/myppt/pages/slide_01.html + slide_02.html
        → output/myppt/pages/

    注意：os.path.commonpath 在只有一个路径时返回路径本身而非父目录，
    因此需要额外处理取 parent。
    """
    if not html_paths:
        return None
    try:
        paths = [Path(p) for p in html_paths]
        common = os.path.commonpath([str(p) for p in paths])
        common_path = Path(common)
        # 只有一个文件时 commonpath 返回文件本身，需要取其父目录
        if common_path in paths:
            common_path = common_path.parent
        return common_path
    except (ValueError, OSError):
        return None


def _read_layout_check_count(project_dir: Path) -> int:
    """读取当前项目的布局检测计数，文件不存在或损坏返回 0。"""
    counter_file = project_dir / ".layout_check_count"
    if not counter_file.is_file():
        return 0
    try:
        return int(counter_file.read_text(encoding="utf-8").strip())
    except (ValueError, OSError):
        return 0


def _increment_layout_check_count(project_dir: Path) -> int:
    """递增布局检测计数并返回新值。"""
    counter_file = project_dir / ".layout_check_count"
    current = _read_layout_check_count(project_dir)
    new_count = current + 1
    counter_file.write_text(str(new_count), encoding="utf-8")
    return new_count


# ═══════════════════════════════════════════════════════════════════════════════
# 底层异步 API — 供模型直接调用
# ═══════════════════════════════════════════════════════════════════════════════


def submit_check_layout(
    html_paths: list[str],
    mode: str = "full",
) -> dict[str, Any]:
    """
    提交异步布局检测任务。

    参数：
        html_paths - HTML 文件路径列表
        mode       - 检测模式: "static" / "browser" / "full"

    返回：
        成功: {"task_id": "uuid-..."}
        失败: {"error": "..."}
    """
    err = _check_wps_sid()
    if err:
        return {"error": err}

    url = f"{BASE_URL}/api/transcoder/layout-check/{mode}/async"
    data, files, _file_handles = _build_layout_check_multipart(html_paths, mode)

    try:
        resp = requests.post(
            url,
            headers=_build_headers(),
            cookies=_build_cookies(),
            data=data,
            files=files,
            timeout=30,
        )
        resp.raise_for_status()
        return _parse_api_response(resp)

    except requests.RequestException as e:
        status = getattr(e.response, "status_code", None)
        body = ""
        try:
            body = e.response.text[:500] if e.response else ""
        except Exception:
            pass
        return {"error": f"提交布局检测任务失败: {e}", "status_code": status, "response_body": body}
    finally:
        _cleanup_file_handles(_file_handles)


def submit_html_to_pptx(
    html_paths: list[str],
) -> dict[str, Any]:
    """
    提交异步 PPTX 转换任务（自动判断单页/批量）。

    参数：
        html_paths - HTML 文件路径列表

    返回：
        成功: {"task_id": "uuid-..."}
        失败: {"error": "..."}
    """
    err = _check_wps_sid()
    if err:
        return {"error": err}

    endpoint, data, files, _file_handles = _build_pptx_convert_multipart(html_paths)
    url = f"{BASE_URL}/api/transcoder/html2pptx/{endpoint}"

    try:
        resp = requests.post(
            url,
            headers=_build_headers(),
            cookies=_build_cookies(),
            data=data,
            files=files,
            timeout=30,
        )
        resp.raise_for_status()
        return _parse_api_response(resp)

    except requests.RequestException as e:
        status = getattr(e.response, "status_code", None)
        body = ""
        try:
            body = e.response.text[:500] if e.response else ""
        except Exception:
            pass
        return {"error": f"提交 PPTX 转换任务失败: {e}", "status_code": status, "response_body": body}
    finally:
        _cleanup_file_handles(_file_handles)


def query_task(task_id: str) -> dict[str, Any]:
    """
    查询异步任务状态。

    参数：
        task_id - 任务 ID

    返回：
        进行中: {"status": "pending"|"running", "task_id": "..."}
        完成（布局检测）: {"status": "completed", "task_id": "...", "result": {"report": "..."}}
        完成（PPTX 转换）: {"status": "completed", "task_id": "...", "result_bytes": b"...", "content_type": "..."}
        失败: {"status": "failed", "task_id": "...", "error": "..."}
        错误: {"error": "..."}
    """
    err = _check_wps_sid()
    if err:
        return {"error": err}

    url = f"{BASE_URL}/api/transcoder/tasks/{task_id}"

    try:
        resp = requests.get(
            url,
            headers=_build_headers(),
            cookies=_build_cookies(),
            timeout=30,
        )
        resp.raise_for_status()

        # 判断响应类型
        content_type = resp.headers.get("Content-Type", "")

        # PPTX 二进制文件 → 转换完成
        if _PPTX_MIME in content_type:
            return {
                "status": "completed",
                "task_id": task_id,
                "result_bytes": resp.content,
                "content_type": content_type,
            }

        # ZIP 二进制文件 → 截图完成
        if _ZIP_MIME in content_type:
            return {
                "status": "completed",
                "task_id": task_id,
                "result_bytes": resp.content,
                "content_type": content_type,
            }

        # JSON 响应
        return _parse_api_response(resp)

    except requests.RequestException as e:
        status = getattr(e.response, "status_code", None)
        body = ""
        try:
            body = e.response.text[:500] if e.response else ""
        except Exception:
            pass
        return {"error": f"查询任务状态失败: {e}", "status_code": status, "response_body": body}


def cancel_task(task_id: str) -> dict[str, Any]:
    """
    取消异步任务。

    参数：
        task_id - 任务 ID

    返回：
        成功: {"task_id": "..."}
        失败: {"error": "..."}
    """
    err = _check_wps_sid()
    if err:
        return {"error": err}

    url = f"{BASE_URL}/api/transcoder/tasks/{task_id}/cancel"

    try:
        resp = requests.post(
            url,
            headers=_build_headers(),
            cookies=_build_cookies(),
            timeout=30,
        )
        resp.raise_for_status()
        return _parse_api_response(resp)

    except requests.RequestException as e:
        status = getattr(e.response, "status_code", None)
        body = ""
        try:
            body = e.response.text[:500] if e.response else ""
        except Exception:
            pass
        return {"error": f"取消任务失败: {e}", "status_code": status, "response_body": body}


def submit_screenshot(
    html_paths: list[str],
    viewport_width: int = 1280,
    viewport_height: int = 720,
    load_timeout: int = 15,
    visual_ready_timeout: int = 8,
) -> dict[str, Any]:
    """
    提交异步截图任务。

    参数：
        html_paths          - HTML 文件路径列表
        viewport_width      - 视口宽度，默认 1280
        viewport_height     - 视口高度，默认 720
        load_timeout        - 页面加载超时（秒），默认 15
        visual_ready_timeout - 视觉就绪超时（秒），默认 8

    返回：
        成功: {"task_id": "uuid-..."}
        失败: {"error": "..."}
    """
    err = _check_wps_sid()
    if err:
        return {"error": err}

    url = f"{BASE_URL}/api/transcoder/screenshot/async"
    data, files, _file_handles = _build_screenshot_multipart(
        html_paths,
        viewport_width=viewport_width,
        viewport_height=viewport_height,
        load_timeout=load_timeout,
        visual_ready_timeout=visual_ready_timeout,
    )

    try:
        resp = requests.post(
            url,
            headers=_build_headers(),
            cookies=_build_cookies(),
            data=data,
            files=files,
            timeout=30,
        )
        resp.raise_for_status()
        return _parse_api_response(resp)

    except requests.RequestException as e:
        status = getattr(e.response, "status_code", None)
        body = ""
        try:
            body = e.response.text[:500] if e.response else ""
        except Exception:
            pass
        return {"error": f"提交截图任务失败: {e}", "status_code": status, "response_body": body}
    finally:
        _cleanup_file_handles(_file_handles)


# ═══════════════════════════════════════════════════════════════════════════════
# 高层自动轮询 — CLI 使用，内部自动 submit → poll → 获取结果
# ═══════════════════════════════════════════════════════════════════════════════


def _poll_task(
    task_id: str,
    timeout: int = DEFAULT_TIMEOUT,
    poll_interval: float = POLL_INTERVAL,
) -> dict[str, Any]:
    """
    轮询任务直到完成/失败/超时。

    返回：
        同 query_task() 的返回格式，但增加 {"error": "task timed out after Xs"} 的超时情况。
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = query_task(task_id) or {}
        # 查询本身出错
        if "error" in result and "status" not in result:
            return result

        status = result.get("status")
        if status in ("completed", "failed"):
            print(f"[Task completed] task_id: {task_id}, status: {status}", file=sys.stderr)
            return result

        time.sleep(poll_interval)

    return {"error": f"task timed out after {timeout}s", "status": "failed", "task_id": task_id}


def check_layout(
    html_paths: list[str],
    mode: str = "full",
    timeout: int = DEFAULT_TIMEOUT,
) -> dict[str, Any]:
    """
    布局检测：自动提交 → 打印 task_id → 轮询 → 返回检测报告。

    参数：
        html_paths - HTML 文件路径列表
        mode       - 检测模式: "static" / "browser" / "full"
        timeout    - 超时秒数（默认 600s / 10 分钟）

    返回：
        成功: {"success": True, "report": "..."}
        失败: {"error": "...", ...}
        超限: {"success": False, "error": "...", "limit_reached": True}
    """
    # ── 布局检测次数限制检查 ──
    project_dir = _get_ppt_project_dir(html_paths)
    if project_dir is not None:
        current_count = _read_layout_check_count(project_dir)
        if current_count >= MAX_LAYOUT_CHECK_COUNT:
            msg = (
                f"布局检测次数已达上限（{current_count}/{MAX_LAYOUT_CHECK_COUNT}），"
                f"请检查 HTML 后进入下一步（导出 PPTX）。"
            )
            print(f"[Layout check] {msg}", file=sys.stderr)
            return {"success": False, "error": msg, "limit_reached": True}

    # 提交任务
    submit_result = submit_check_layout(html_paths, mode)
    if "error" in submit_result:
        return submit_result

    task_id = submit_result.get("task_id", "")
    # 打印 task_id 让模型感知
    print(f"[Task submitted] task_id: {task_id}", file=sys.stderr)

    # 轮询等待
    poll_result = _poll_task(task_id, timeout=timeout)
    if "error" in poll_result:
        return poll_result

    status = poll_result.get("status")
    if status == "completed":
        # 仅在检测成功完成后递增计数
        if project_dir is not None:
            new_count = _increment_layout_check_count(project_dir)
            print(f"[Layout check] completed ({new_count}/{MAX_LAYOUT_CHECK_COUNT})", file=sys.stderr)
        result_data = poll_result.get("result", {})
        report = result_data.get("report", "")
        return {"success": True, "report": report}

    # failed — 不消耗检测次数
    return {"error": poll_result.get("error", "task failed"), "task_id": task_id}


def html_to_pptx(
    html_paths: list[str],
    output_path: str,
    timeout: int = DEFAULT_TIMEOUT,
) -> dict[str, Any]:
    """
    HTML→PPTX 转换：自动提交 → 打印 task_id → 轮询 → 下载文件。

    参数：
        html_paths  - HTML 文件路径列表（单页或批量）
        output_path - 输出 .pptx 文件路径
        timeout     - 超时秒数（默认 600s / 10 分钟）

    返回：
        成功: {"success": True, "path": "...", "size_kb": ...}
        失败: {"error": "...", ...}
    """
    # 提交任务
    submit_result = submit_html_to_pptx(html_paths)
    if "error" in submit_result:
        return submit_result

    task_id = submit_result.get("task_id", "")
    # 打印 task_id 让模型感知
    print(f"[Task submitted] task_id: {task_id}", file=sys.stderr)

    # 轮询等待
    poll_result = _poll_task(task_id, timeout=timeout)
    if "error" in poll_result:
        return poll_result

    status = poll_result.get("status")
    if status == "completed":
        # PPTX 转换完成 → 二进制文件
        result_bytes = poll_result.get("result_bytes")
        if result_bytes:
            out = Path(output_path)
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(result_bytes)
            return {
                "success": True,
                "path": str(out),
                "size_kb": round(len(result_bytes) / 1024, 1),
            }

        # 也可能是 JSON 结果（布局检测格式兜底）
        result_data = poll_result.get("result", {})
        return {"success": True, **result_data}

    # failed
    return {"error": poll_result.get("error", "task failed"), "task_id": task_id}


def screenshot(
    html_files: list[str],
    output_dir: str = ".",
    viewport_width: int = 1280,
    viewport_height: int = 720,
    load_timeout: int = 15,
    visual_ready_timeout: int = 8,
    timeout: int = DEFAULT_TIMEOUT,
) -> dict[str, Any]:
    """
    HTML 幻灯片截图：自动提交 → 打印 task_id → 轮询 → 下载 ZIP → 解压 PNG。

    参数：
        html_files          - HTML 文件路径列表
        output_dir          - 输出目录（ZIP 在此解压，默认当前目录）
        viewport_width      - 视口宽度，默认 1280
        viewport_height     - 视口高度，默认 720
        load_timeout        - 页面加载超时（秒），默认 15
        visual_ready_timeout - 视觉就绪超时（秒），默认 8
        timeout             - 任务超时秒数（默认 600s / 10 分钟）

    返回：
        成功: {"success": True, "paths": ["/abs/path/to/slide_01.png", ...]}
        失败: {"error": "...", ...}
    """
    submit_result = submit_screenshot(
        html_files,
        viewport_width=viewport_width,
        viewport_height=viewport_height,
        load_timeout=load_timeout,
        visual_ready_timeout=visual_ready_timeout,
    )
    if "error" in submit_result:
        return submit_result

    task_id = submit_result.get("task_id", "")
    print(f"[Task submitted] task_id: {task_id}", file=sys.stderr)

    poll_result = _poll_task(task_id, timeout=timeout)
    if "error" in poll_result:
        return poll_result

    status = poll_result.get("status")
    if status != "completed":
        return {"error": poll_result.get("error", "task failed"), "task_id": task_id}

    result_bytes = poll_result.get("result_bytes")
    if not result_bytes:
        return {"error": "screenshot task completed but no binary data returned", "task_id": task_id}

    # 解压 ZIP
    import io
    import zipfile

    out_dir = Path(output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    png_paths: list[str] = []
    with zipfile.ZipFile(io.BytesIO(result_bytes)) as zf:
        for name in zf.namelist():
            if not name.endswith(".png"):
                continue
            dest = out_dir / Path(name).name
            dest.write_bytes(zf.read(name))
            png_paths.append(str(dest))

    return {"success": True, "paths": sorted(png_paths)}
