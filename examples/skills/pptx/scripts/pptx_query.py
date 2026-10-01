from __future__ import annotations

import hashlib
import os
from pptx.enum.shapes import MSO_SHAPE_TYPE

from pptx_colors import (
    EMU_PER_CM, _NS_A, emu_to_cm,
    _resolve_text_color, _extract_color_info,
    get_fill_info, get_line_info,
)
from pptx_text import calculate_text_size, calculate_text_frame_size


# ---------------------------------------------------------------------------
# 图片导出工具
# ---------------------------------------------------------------------------

def export_image_from_shape(shape, output_dir, prefix="img"):
    """
    从 shape 中导出图片文件到指定目录。

    支持独立图片（PICTURE 类型）和图片填充（fill）两种来源。

    Args:
        shape: pptx shape 对象（PICTURE 类型或含图片填充的 shape）
        output_dir: 输出目录
        prefix: 文件名前缀

    Returns:
        str | None: 导出后的文件路径，失败返回 None
    """
    try:
        image_part = None
        if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
            image_part = shape.image
        else:
            r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
            a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
            blips = shape.element.findall(f'.//{{{a_ns}}}blip')
            if blips:
                rId = blips[0].get(f'{{{r_ns}}}embed')
                if rId and rId in shape.part.rels:
                    image_part = shape.part.rels[rId].target_part

        if image_part is None:
            return None

        blob = image_part.blob
        ext = getattr(image_part, 'ext', None)
        if not ext:
            ct = getattr(image_part, 'content_type', '')
            ext = ct.split('/')[-1] if '/' in ct else 'png'
        ext = ext.lstrip('.') or 'png'

        os.makedirs(output_dir, exist_ok=True)
        filename = f"{prefix}_{shape.shape_id}.{ext}"
        filepath = os.path.join(output_dir, filename)

        with open(filepath, 'wb') as f:
            f.write(blob)

        return filepath
    except Exception:
        return None


def is_slide_bg_image(slide_width, slide_height, x_emu, y_emu, w_emu, h_emu):
    """
    判断图片是否为幻灯片背景图。
    标准：图片覆盖 > 90% 幻灯片面积 且 起始位置接近原点（< 1cm 偏移）。
    """
    area_shape = w_emu * h_emu
    area_slide = slide_width * slide_height
    coverage = area_shape / area_slide if area_slide > 0 else 0
    offset_x = x_emu / EMU_PER_CM
    offset_y = y_emu / EMU_PER_CM
    return coverage > 0.9 and offset_x < 1.0 and offset_y < 1.0


def _get_image_blob(shape):
    """从 shape 中获取图片 blob，支持 PICTURE 和图片填充两种来源。"""
    try:
        if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
            return shape.image.blob
        r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
        a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
        blips = shape.element.findall(f'.//{{{a_ns}}}blip')
        if blips:
            rId = blips[0].get(f'{{{r_ns}}}embed')
            if rId and rId in shape.part.rels:
                return shape.part.rels[rId].target_part.blob
    except Exception:
        pass
    return None


def extract_bg_images_from_layouts(
    presentation, output_dir=None, prefix="layout_bg",
):
    """
    扫描 Presentation 中所有 slide layout 和 slide master，提取背景级图片。

    覆盖两种来源：
    1. shapes 层的大面积图片（PICTURE shape 或图片填充 shape）
    2. bgPr 层的图片背景（<p:bgPr><a:blipFill>）

    通过 blob hash 自动去重，同一张底图只导出一次。

    Args:
        presentation: pptx Presentation 对象
        output_dir: 导出目录，None 则不导出文件
        prefix: 文件名前缀

    Returns:
        list[dict]: 背景图信息列表，每项包含 source / ext / is_bg，
            导出成功时附带 exported_path。
    """
    slide_width = presentation.slide_width
    slide_height = presentation.slide_height

    seen_hashes: set[str] = set()
    seen_layers: set[int] = set()

    _PICTURE_FILL_TYPES = (
        MSO_SHAPE_TYPE.AUTO_SHAPE,
        MSO_SHAPE_TYPE.FREEFORM,
        MSO_SHAPE_TYPE.PLACEHOLDER,
        MSO_SHAPE_TYPE.TEXT_BOX,
    )

    _NS_A = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    _NS_P = 'http://schemas.openxmlformats.org/presentationml/2006/main'
    _NS_R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'

    results = []

    def _try_add_blob(blob, ext, source_name):
        """去重并导出一张背景图，返回是否成功添加。"""
        blob_hash = hashlib.md5(blob).hexdigest()[:12]
        if blob_hash in seen_hashes:
            return
        seen_hashes.add(blob_hash)

        info = {"source": source_name, "ext": ext, "is_bg": True}

        if output_dir:
            os.makedirs(output_dir, exist_ok=True)
            filepath = os.path.join(output_dir, f"{prefix}_{blob_hash}.{ext}")
            with open(filepath, 'wb') as f:
                f.write(blob)
            info["exported_path"] = filepath

        results.append(info)

    for layout in presentation.slide_layouts:
        layers = [
            ("layout", layout),
            ("master", layout.slide_master),
        ]
        for source_name, layer_obj in layers:
            layer_id = id(layer_obj)
            if layer_id in seen_layers:
                continue
            seen_layers.add(layer_id)

            # --- A. shapes 层：大面积图片 shape ---
            try:
                shapes = layer_obj.shapes
            except Exception:
                shapes = []

            for shape in shapes:
                is_picture = shape.shape_type == MSO_SHAPE_TYPE.PICTURE
                is_fill_image = False
                if not is_picture and shape.shape_type in _PICTURE_FILL_TYPES:
                    try:
                        if shape.fill.type == 6:
                            is_fill_image = True
                    except Exception:
                        pass

                if not is_picture and not is_fill_image:
                    continue

                try:
                    x, y, w, h = shape.left, shape.top, shape.width, shape.height
                except Exception:
                    continue

                if not is_slide_bg_image(slide_width, slide_height, x, y, w, h):
                    continue

                blob = _get_image_blob(shape)
                if blob is None:
                    continue

                ext = "png"
                if is_picture:
                    try:
                        ext = shape.image.ext
                    except Exception:
                        pass

                _try_add_blob(blob, ext, source_name)

            # --- B. bgPr 层：PPT 原生图片背景 ---
            try:
                bg_pr = layer_obj._element.find(f'.//{{{_NS_P}}}bgPr')
                if bg_pr is None:
                    continue
                blip = bg_pr.find(f'.//{{{_NS_A}}}blip')
                if blip is None:
                    continue
                rId = blip.get(f'{{{_NS_R}}}embed')
                if not rId or rId not in layer_obj.part.rels:
                    continue
                img_part = layer_obj.part.rels[rId].target_part
                blob = img_part.blob
                ext = getattr(img_part, 'ext', None) or 'png'
                ext = ext.lstrip('.')
                _try_add_blob(blob, ext, source_name)
            except Exception:
                pass

    return results


# ---------------------------------------------------------------------------
# 形状树遍历
# ---------------------------------------------------------------------------

def _get_group_scale(group_shape):
    """
    读取 Group 的缩放系数 (ext / chExt)。
    读取失败时返回 (1.0, 1.0)。
    """
    try:
        ns_a = "http://schemas.openxmlformats.org/drawingml/2006/main"
        xfrm = group_shape._element.find(f".//{{{ns_a}}}xfrm")
        if xfrm is None:
            return 1.0, 1.0
        ext = xfrm.find(f"{{{ns_a}}}ext")
        ch_ext = xfrm.find(f"{{{ns_a}}}chExt")
        if ext is None or ch_ext is None:
            return 1.0, 1.0
        ext_cx = int(ext.get("cx", "0"))
        ext_cy = int(ext.get("cy", "0"))
        ch_cx = int(ch_ext.get("cx", "0"))
        ch_cy = int(ch_ext.get("cy", "0"))
        if ch_cx <= 0 or ch_cy <= 0:
            return 1.0, 1.0
        return ext_cx / ch_cx, ext_cy / ch_cy
    except Exception:
        return 1.0, 1.0


def _get_group_ch_off(group_shape):
    """
    读取 Group 的 chOff（子坐标系原点在 Group 内部坐标系中的偏移，EMU）。
    读取失败时返回 (0, 0)。
    """
    try:
        ns_a = "http://schemas.openxmlformats.org/drawingml/2006/main"
        xfrm = group_shape._element.find(f".//{{{ns_a}}}xfrm")
        if xfrm is None:
            return 0, 0
        ch_off = xfrm.find(f"{{{ns_a}}}chOff")
        if ch_off is None:
            return 0, 0
        return int(ch_off.get("x", "0")), int(ch_off.get("y", "0"))
    except Exception:
        return 0, 0


def _flatten_shapes(slide):
    """
    递归展开 slide 中的所有 shape（含 Group 子元素），返回平铺列表。

    Group 内子元素的坐标统一转换为绝对位置和绝对尺寸。

    Returns:
        list[tuple]: [(shape, z_order, abs_left_emu, abs_top_emu, abs_w_emu, abs_h_emu), ...]
    """
    result = []
    z = 0

    def _walk(shapes, parent_off_x, parent_off_y,
              parent_chOff_x, parent_chOff_y,
              cum_scale_x, cum_scale_y):
        nonlocal z
        for shape in shapes:
            if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
                g_scale_x, g_scale_y = _get_group_scale(shape)
                g_chOff_x, g_chOff_y = _get_group_ch_off(shape)
                grp_abs_x = parent_off_x + (shape.left - parent_chOff_x) * cum_scale_x
                grp_abs_y = parent_off_y + (shape.top - parent_chOff_y) * cum_scale_y
                new_scale_x = cum_scale_x * g_scale_x
                new_scale_y = cum_scale_y * g_scale_y
                _walk(shape.shapes,
                      grp_abs_x, grp_abs_y,
                      g_chOff_x, g_chOff_y,
                      new_scale_x, new_scale_y)
            else:
                abs_x = parent_off_x + (shape.left - parent_chOff_x) * cum_scale_x
                abs_y = parent_off_y + (shape.top - parent_chOff_y) * cum_scale_y
                abs_w = shape.width * cum_scale_x
                abs_h = shape.height * cum_scale_y
                result.append((shape, z, abs_x, abs_y, abs_w, abs_h))
                z += 1

    _walk(slide.shapes, 0, 0, 0, 0, 1.0, 1.0)
    return result


def _find_shape_by_id(slide, shape_id):
    """在 slide 中按 shape_id 查找 shape（含 Group 递归）。"""
    for shape in slide.shapes:
        if shape.shape_id == shape_id:
            return shape
        if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
            found = _find_shape_by_id_in_group(shape, shape_id)
            if found is not None:
                return found
    return None


def _find_shape_by_id_in_group(group, shape_id):
    """在 Group 内递归查找 shape_id"""
    for sub in group.shapes:
        if sub.shape_id == shape_id:
            return sub
        if sub.shape_type == MSO_SHAPE_TYPE.GROUP:
            found = _find_shape_by_id_in_group(sub, shape_id)
            if found is not None:
                return found
    return None


# ---------------------------------------------------------------------------
# 按需查询接口 — 白名单模式
# ---------------------------------------------------------------------------

_SLIDE_INCLUDE_KEYS = frozenset({"summary", "texts", "images", "tables", "charts", "layouts"})
_SHAPE_INCLUDE_KEYS = frozenset({"text", "style", "layout"})


def query_slide(slide, include):
    """
    按类别查询幻灯片页面信息（白名单模式）。

    每次调用从 slide 重新遍历 shapes，无状态，无缓存。
    Group 内子元素平铺，不嵌套。

    Args:
        slide: pptx Slide 对象
        include: list[str]，白名单，支持的值:
            - "summary": 元素总数 + 按类型分布
            - "texts":   所有含文本元素的 [{shape_id, text_preview}]
            - "images":  所有图片 [{shape_id, ext}]
            - "tables":  所有表格 [{shape_id, rows, cols}]
            - "charts":  所有图表 [{shape_id, chart_type}]
            - "layouts": 所有元素布局 [{shape_id, x, y, w, h, z_order}]

    Returns:
        dict: key 为 include 中指定的类别，value 为对应数据列表/字典

    Raises:
        ValueError: include 中包含不支持的 key
    """
    invalid = set(include) - _SLIDE_INCLUDE_KEYS
    if invalid:
        raise ValueError(
            f"query_slide: 不支持的 include key: {invalid}，"
            f"支持的 key: {sorted(_SLIDE_INCLUDE_KEYS)}"
        )

    include_set = set(include)
    result = {}

    needs_shapes = bool(include_set & {"summary", "texts", "images", "tables", "charts", "layouts"})

    shapes = _flatten_shapes(slide) if needs_shapes else []

    if "summary" in include_set:
        by_type = {}
        total = 0
        for shape, z, *_ in shapes:
            total += 1
            type_name = shape.shape_type.name
            by_type[type_name] = by_type.get(type_name, 0) + 1
        result["summary"] = {"total": total, "by_type": by_type}

    if "texts" in include_set:
        texts = []
        for shape, z, *_ in shapes:
            if shape.has_text_frame and shape.text.strip():
                preview = shape.text.strip().replace("\n", " ")
                if len(preview) > 30:
                    preview = preview[:30] + "..."
                texts.append({"shape_id": shape.shape_id, "text_preview": preview})
        result["texts"] = texts

    if "images" in include_set:
        images = []
        _FILL_IMAGE_TYPES = (
            MSO_SHAPE_TYPE.AUTO_SHAPE,
            MSO_SHAPE_TYPE.FREEFORM,
            MSO_SHAPE_TYPE.PLACEHOLDER,
            MSO_SHAPE_TYPE.TEXT_BOX,
        )
        r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
        a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

        for shape, z, *_ in shapes:
            if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
                try:
                    ext = shape.image.ext
                except Exception:
                    ext = "unknown"
                images.append({
                    "shape_id": shape.shape_id,
                    "ext": ext,
                    "fill_shape": False,
                })
            elif shape.shape_type in _FILL_IMAGE_TYPES:
                try:
                    fill_type = shape.fill.type
                    if fill_type == 6:  # PICTURE
                        blips = shape.element.findall(f'.//{{{a_ns}}}blip')
                        ext = "unknown"
                        if blips:
                            rId = blips[0].get(f'{{{r_ns}}}embed')
                            if rId and rId in shape.part.rels:
                                image_part = shape.part.rels[rId].target_part
                                partname = str(image_part.partname)
                                ext = os.path.splitext(partname)[1].lstrip('.')
                        images.append({
                            "shape_id": shape.shape_id,
                            "ext": ext,
                            "fill_shape": True,
                        })
                except Exception:
                    pass
        result["images"] = images

    if "tables" in include_set:
        tables = []
        for shape, z, *_ in shapes:
            if shape.has_table:
                table = shape.table
                tables.append({
                    "shape_id": shape.shape_id,
                    "rows": len(table.rows),
                    "cols": len(table.columns),
                })
        result["tables"] = tables

    if "charts" in include_set:
        charts = []
        for shape, z, *_ in shapes:
            if shape.has_chart:
                charts.append({
                    "shape_id": shape.shape_id,
                    "chart_type": str(shape.chart.chart_type),
                })
        result["charts"] = charts

    if "layouts" in include_set:
        layouts = []
        for shape, z_order, *_ in shapes:
            layout = _extract_layout(slide, shape.shape_id)
            if layout:
                layout["shape_id"] = shape.shape_id
                layout["z_order"] = z_order
                layouts.append(layout)
        result["layouts"] = layouts

    return result


def query_shape(slide, shape_id, include):
    """
    按类别查询单个 shape 的详细信息（白名单模式）。

    每次调用从 slide 重新查找，无状态，无缓存。
    当 shape_id 指向 Group 时，返回该 Group 下所有子元素的信息。

    布局信息使用绝对坐标（Group 内子元素已转换到页面坐标系）。

    Args:
        slide: pptx Slide 对象
        shape_id: int，目标元素的 shape_id
        include: list[str]，白名单，支持的值:
            - "text":       完整文本信息（段落、run、文字内容、字体格式），格式在 run.format 中返回
            - "style":      形状视觉样式（fill, line）
                             仅适用于 AUTO_SHAPE / FREEFORM / PLACEHOLDER / TEXT_BOX，
                             其他类型（PICTURE / GROUP / LINE / TABLE 等）返回 None
            - "layout":     布局（x, y, w, h, z_order）

    Returns:
        dict: key 为 include 中指定的类别

    Raises:
        ValueError: include 中包含不支持的 key
        LookupError: shape_id 不存在
    """
    invalid = set(include) - _SHAPE_INCLUDE_KEYS
    if invalid:
        raise ValueError(
            f"query_shape: 不支持的 include key: {invalid}，"
            f"支持的 key: {sorted(_SHAPE_INCLUDE_KEYS)}"
        )

    shape = _find_shape_by_id(slide, shape_id)
    if shape is None:
        raise LookupError(f"shape_id={shape_id} 在当前 slide 中未找到")

    if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
        return _query_shape_group(slide, shape, include)

    result = {}
    include_set = set(include)

    if "text" in include_set:
        result["text"] = _extract_text(shape, slide)

    if "style" in include_set:
        result["style"] = _extract_shape_style(shape, slide)

    if "layout" in include_set:
        result["layout"] = _extract_layout(slide, shape_id)

    return result


def _query_shape_group(slide, group, include):
    """
    Group 的 query_shape 处理：返回所有子元素的查询结果。
    布局信息使用绝对坐标（已考虑 Group 偏移和缩放）。
    """
    include_list = list(include)
    by_key = {key: [] for key in include_list}

    def _walk(shapes):
        for shape in shapes:
            if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
                _walk(shape.shapes)
            else:
                extracted = {}
                if "text" in include_list:
                    extracted["text"] = _extract_text(shape, slide)
                if "style" in include_list:
                    extracted["style"] = _extract_shape_style(shape, slide)
                if "layout" in include_list:
                    extracted["layout"] = _extract_layout(slide, shape.shape_id)

                for key in include_list:
                    item = {"shape_id": shape.shape_id, key: extracted[key]}
                    by_key[key].append(item)

    _walk(group.shapes)
    return by_key


# ---------------------------------------------------------------------------
# 提取器函数
# ---------------------------------------------------------------------------

def _extract_text(shape, slide=None):
    """提取 shape 的完整文本信息"""
    if not shape.has_text_frame or not shape.text.strip():
        return None

    text_frame = shape.text_frame
    paragraphs = []

    for para_idx, para in enumerate(text_frame.paragraphs):
        para_info = {
            "index": para_idx,
        }
        if para.alignment is not None:
            para_info["alignment"] = str(para.alignment)
        if para.line_spacing is not None:
            if hasattr(para.line_spacing, 'pt'):
                para_info["line_spacing"] = {"value_pt": round(para.line_spacing.pt, 1), "type": "fixed"}
            else:
                para_info["line_spacing"] = {"value": float(para.line_spacing), "type": "multiple"}

        runs = []
        for run_idx, run in enumerate(para.runs):
            run_info = {"index": run_idx, "text": run.text}
            fmt = {}
            if run.font.name:
                fmt["font_name"] = run.font.name
            if run.font.size:
                fmt["size_pt"] = run.font.size.pt
            if run.font.bold is not None:
                fmt["bold"] = run.font.bold
            if run.font.italic is not None:
                fmt["italic"] = run.font.italic
            if run.font.underline is not None:
                fmt["underline"] = run.font.underline
            try:
                ci = _resolve_text_color(run, slide)
                if ci:
                    if "color" in ci:
                        fmt["color_rgb"] = ci["color"]
                    if "theme" in ci:
                        fmt["color_theme"] = ci["theme"]
                    if "color_type" in ci:
                        fmt["color_type"] = ci["color_type"]
                    if "lumMod" in ci:
                        fmt["color_lumMod"] = ci["lumMod"]
                    if "lumOff" in ci:
                        fmt["color_lumOff"] = ci["lumOff"]
                    if ci.get("gradient"):
                        fmt["color_gradient"] = True
                        if "gradient_stops" in ci:
                            fmt["color_gradient_stops"] = ci["gradient_stops"]
                        if "gradient_angle" in ci:
                            fmt["color_gradient_angle"] = ci["gradient_angle"]
            except Exception:
                pass
            if fmt:
                run_info["format"] = fmt
            runs.append(run_info)

        if runs:
            para_info["runs"] = runs
        paragraphs.append(para_info)

    return {
        "word_wrap": text_frame.word_wrap,
        "paragraphs": paragraphs,
    }


def _extract_shape_style(shape, slide=None):
    """提取 shape 的视觉样式（fill + line）。

    适用范围：AUTO_SHAPE, FREEFORM, PLACEHOLDER, TEXT_BOX。
    不适用（返回 None）：PICTURE, GROUP, LINE, TABLE, CHART 等。
    """
    style = {}

    _SHAPE_TYPES_WITH_STYLE = (
        MSO_SHAPE_TYPE.AUTO_SHAPE,
        MSO_SHAPE_TYPE.FREEFORM,
        MSO_SHAPE_TYPE.PLACEHOLDER,
        MSO_SHAPE_TYPE.TEXT_BOX,
    )

    if shape.shape_type in _SHAPE_TYPES_WITH_STYLE:
        try:
            if hasattr(shape, 'fill'):
                fill = get_fill_info(shape.fill, shape, slide)
                if fill:
                    style["fill"] = fill
        except Exception:
            pass

        try:
            if hasattr(shape, 'line'):
                line = get_line_info(shape.line, slide)
                if line:
                    style["line"] = line
        except Exception:
            pass

    return style if style else None


def _extract_text_render_size(shape, frame_width_cm, slide=None):
    """
    计算文本框内文本的实际渲染尺寸（cm）。
    """
    if not shape.has_text_frame or not shape.text.strip():
        return None
    try:
        info = calculate_text_frame_size(shape.text_frame, frame_width_cm, slide)
        text_h = info['total_height_cm']
        text_w = info['total_width_cm']
        frame_h = emu_to_cm(shape.height)
        frame_w = frame_width_cm
        height_overflow = round(text_h, 2) > round(frame_h, 2)
        width_overflow = round(text_w, 2) > round(frame_w, 2)
        return {
            'text_width_cm': text_w,
            'text_height_cm': text_h,
            'is_overflow': height_overflow or width_overflow,
            'height_overflow': height_overflow,
            'width_overflow': width_overflow,
        }
    except Exception:
        return None


def _extract_layout(slide, shape_id):
    """
    提取 shape 的布局信息（绝对坐标，cm）。Group 内子元素自动转换到页面坐标系。

    对于文本类 shape，自动计算文本渲染尺寸并附加到结果中。
    """
    for shape, z, ax, ay, aw, ah in _flatten_shapes(slide):
        if shape.shape_id == shape_id:
            result = {
                "x": emu_to_cm(ax),
                "y": emu_to_cm(ay),
                "w": emu_to_cm(aw),
                "h": emu_to_cm(ah),
            }
            if shape.has_text_frame and shape.text.strip():
                text_render = _extract_text_render_size(shape, result["w"], slide)
                if text_render:
                    result["text_render"] = text_render
            return result
    return None
