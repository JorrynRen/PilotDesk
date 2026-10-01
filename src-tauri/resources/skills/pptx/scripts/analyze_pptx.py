"""
analyze_pptx — 兼容层

所有实现已拆分到以下子模块：
  pptx_colors        颜色 / 填充 / 线条 / 背景
  pptx_text          字体发现 / 文本测量
  pptx_query         形状遍历 / 查询 API / 图片导出
  pptx_layout_check  布局校验（越界 / 重叠）

本文件统一 re-export 全部公开符号，外部调用者无需修改。
"""
from __future__ import annotations

# --- pptx_colors ---
from pptx_colors import (  # noqa: F401
    EMU_PER_CM,
    _NS_A,
    emu_to_cm,
    cm_to_emu,
    _get_theme_colors_for_master,
    _extract_color_info,
    _find_solid_fill_element,
    _extract_run_color,
    _resolve_theme_color,
    _resolve_placeholder_inherited_props,
    _resolve_text_color,
    get_fill_info,
    get_line_info,
    get_slide_background,
)

# --- pptx_text ---
from pptx_text import (  # noqa: F401
    _get_system_dpi,
    _convert_unit,
    _is_cjk_char,
    _has_cjk,
    _scan_system_fonts,
    _register_font_names,
    _find_font_path,
    _estimate_text_size,
    calculate_text_size,
    calculate_text_frame_size,
)

# --- pptx_query ---
from pptx_query import (  # noqa: F401
    export_image_from_shape,
    is_slide_bg_image,
    _get_image_blob,
    extract_bg_images_from_layouts,
    _get_group_scale,
    _get_group_ch_off,
    _flatten_shapes,
    _find_shape_by_id,
    _find_shape_by_id_in_group,
    _SLIDE_INCLUDE_KEYS,
    _SHAPE_INCLUDE_KEYS,
    query_slide,
    query_shape,
    _query_shape_group,
    _extract_text,
    _extract_shape_style,
    _extract_text_render_size,
    _extract_layout,
)

# --- pptx_layout_check ---
from pptx_layout_check import (  # noqa: F401
    _calculate_text_actual_position,
    _get_text_line_boxes,
    _collect_shapes_info,
    check_slide_layout,
    check_layouts,
    _is_overlap_rect,
    _is_overlap,
    _calculate_overlap_area,
    _format_shape_issue,
    _format_overlap_issue,
)
