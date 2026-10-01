from __future__ import annotations

from pptx.enum.shapes import MSO_SHAPE_TYPE

from pptx_colors import emu_to_cm, cm_to_emu
from pptx_text import _convert_unit, calculate_text_size, calculate_text_frame_size


def _calculate_text_actual_position(shape):
	"""
	根据文本对齐方式计算文本的实际渲染位置和尺寸

	Args:
		shape: pptx shape 对象（包含文本框）

	Returns:
		tuple: (left, top, width, height) 文本实际位置和尺寸（EMU）
	"""
	if not shape.has_text_frame or not shape.text.strip():
		return shape.left, shape.top, shape.width, shape.height

	try:
		text_frame = shape.text_frame
		text_frame_width_cm = emu_to_cm(shape.width)
		text_frame_height_cm = emu_to_cm(shape.height)
		text_size_info = calculate_text_frame_size(text_frame, text_frame_width_cm)

		text_width_emu = cm_to_emu(text_size_info['total_width_cm'])
		text_height_emu = cm_to_emu(text_size_info['total_height_cm'])

		left = shape.left
		top = shape.top

		if text_frame.paragraphs:
			from pptx.enum.text import PP_ALIGN
			para_alignment = text_frame.paragraphs[0].alignment

			if para_alignment == PP_ALIGN.CENTER:
				left = shape.left + (shape.width - text_width_emu) // 2
			elif para_alignment == PP_ALIGN.RIGHT:
				left = shape.left + shape.width - text_width_emu

		if hasattr(text_frame, 'vertical_anchor'):
			from pptx.enum.text import MSO_ANCHOR
			vertical_anchor = text_frame.vertical_anchor

			if vertical_anchor == MSO_ANCHOR.MIDDLE:
				top = shape.top + (shape.height - text_height_emu) // 2
			elif vertical_anchor == MSO_ANCHOR.BOTTOM:
				top = shape.top + shape.height - text_height_emu

		return left, top, text_width_emu, text_height_emu

	except Exception:
		return shape.left, shape.top, shape.width, shape.height


def _get_text_line_boxes(shape):
	"""
	计算文本框中每一行文字的边界框（EMU）。

	对于自动换行的文本框，根据文本框宽度和每行的实际渲染宽度，
	计算每行的精确位置和尺寸。

	Args:
		shape: pptx shape 对象（包含文本框）

	Returns:
		list[dict]: 每行文字的边界框，每项包含 left/top/right/bottom (EMU)
	"""
	if not shape.has_text_frame or not shape.text.strip():
		return []

	try:
		text_frame = shape.text_frame
		frame_width_emu = shape.width
		frame_width_cm = emu_to_cm(frame_width_emu)

		from pptx.enum.text import PP_ALIGN
		para_alignment = text_frame.paragraphs[0].alignment if text_frame.paragraphs else None

		text_size_info = calculate_text_frame_size(text_frame, frame_width_cm)
		text_height_emu = cm_to_emu(text_size_info['total_height_cm'])

		top_offset_emu = 0
		if hasattr(text_frame, 'vertical_anchor'):
			from pptx.enum.text import MSO_ANCHOR
			va = text_frame.vertical_anchor
			if va == MSO_ANCHOR.MIDDLE:
				top_offset_emu = (shape.height - text_height_emu) // 2
			elif va == MSO_ANCHOR.BOTTOM:
				top_offset_emu = shape.height - text_height_emu

		line_boxes = []
		current_top_emu = shape.top + top_offset_emu

		for para_idx, para in enumerate(text_frame.paragraphs):
			if para_idx > 0 and para.space_before:
				space_before_emu = cm_to_emu(para.space_before.pt * 0.0353) if hasattr(para.space_before, 'pt') else 0
				current_top_emu += space_before_emu

			run_widths_cm = []
			for run in para.runs:
				run_text = run.text
				if not run_text.strip():
					run_widths_cm.append((0, 0, run_text, '', 12, False))
					continue
				font_name = run.font.name or 'Arial'
				font_size_pt = run.font.size.pt if run.font.size else 18
				is_bold = bool(run.font.bold)
				w_cm, h_cm = calculate_text_size(run_text.strip(), font_name, font_size_pt, is_bold)
				run_widths_cm.append((w_cm, h_cm, run_text, font_name, font_size_pt, is_bold))

			para_total_width_cm = sum(item[0] for item in run_widths_cm)
			para_max_height_cm = max((item[1] for item in run_widths_cm), default=0)
			para_max_height_emu = cm_to_emu(para_max_height_cm)

			line_spacing = 1.0
			is_fixed_line_spacing = False
			fixed_line_spacing_emu = 0
			if para.line_spacing:
				if hasattr(para.line_spacing, 'pt'):
					is_fixed_line_spacing = True
					fixed_line_spacing_emu = cm_to_emu(para.line_spacing.pt * 0.0353)
				else:
					line_spacing = float(para.line_spacing)

			if text_frame.word_wrap and para_total_width_cm > frame_width_cm:
				all_chars = []
				all_heights = []
				for item in run_widths_cm:
					w_cm, h_cm, run_text, font_name, font_size_pt, is_bold = item
					for ch in run_text:
						all_chars.append((ch, font_name, font_size_pt, is_bold))
						all_heights.append(h_cm)

				lines = []
				char_idx = 0
				while char_idx < len(all_chars):
					line_end = char_idx
					seg_start = char_idx
					seg_font = all_chars[char_idx][1:]
					line_width_cm = 0.0

					while line_end < len(all_chars):
						ch, fn, fs, ib = all_chars[line_end]
						if (fn, fs, ib) != seg_font:
							seg_text = ''.join(c for c, *_ in all_chars[seg_start:line_end])
							seg_width, _ = calculate_text_size(seg_text, *seg_font)
							line_width_cm += seg_width
							seg_start = line_end
							seg_font = (fn, fs, ib)

						prefix_text = ''.join(c for c, *_ in all_chars[seg_start:line_end + 1])
						prefix_width, _ = calculate_text_size(prefix_text, *seg_font)
						test_width = line_width_cm + prefix_width

						if test_width > frame_width_cm and line_end > char_idx:
							break
						line_end += 1

					if seg_start < line_end:
						seg_text = ''.join(c for c, *_ in all_chars[seg_start:line_end])
						seg_width, _ = calculate_text_size(seg_text, *seg_font)
						line_width_cm += seg_width

					line = []
					for i in range(char_idx, line_end):
						ch = all_chars[i][0]
						cw, _ = calculate_text_size(ch, *all_chars[i][1:])
						line.append((cw, all_heights[i], ch))
					lines.append(line)
					char_idx = line_end

				if not lines:
					lines = [[]]

				for line_idx, line_runs in enumerate(lines):
					line_width_cm = sum(w for w, h, t in line_runs)
					line_height_emu = para_max_height_emu

					if is_fixed_line_spacing:
						line_total_height_emu = fixed_line_spacing_emu
					else:
						line_total_height_emu = int(para_max_height_emu * line_spacing)

					line_left_emu = shape.left
					if para_alignment == PP_ALIGN.CENTER:
						line_content_width_emu = cm_to_emu(line_width_cm)
						line_left_emu = shape.left + (frame_width_emu - line_content_width_emu) // 2
					elif para_alignment == PP_ALIGN.RIGHT:
						line_content_width_emu = cm_to_emu(line_width_cm)
						line_left_emu = shape.left + frame_width_emu - line_content_width_emu

					line_content_right_emu = shape.left + cm_to_emu(line_width_cm)
					if para_alignment == PP_ALIGN.LEFT or para_alignment is None:
						line_right_emu = min(shape.left + frame_width_emu, line_content_right_emu)
					else:
						line_right_emu = shape.left + frame_width_emu

					line_boxes.append({
						'left': line_left_emu,
						'top': current_top_emu,
						'right': line_right_emu,
						'bottom': current_top_emu + line_total_height_emu,
					})

					current_top_emu += line_total_height_emu
			else:
				if is_fixed_line_spacing:
					line_total_height_emu = fixed_line_spacing_emu
				else:
					line_total_height_emu = int(para_max_height_emu * line_spacing)

				line_left_emu = shape.left
				if para_alignment == PP_ALIGN.CENTER:
					line_content_width_emu = cm_to_emu(para_total_width_cm)
					line_left_emu = shape.left + (frame_width_emu - line_content_width_emu) // 2
				elif para_alignment == PP_ALIGN.RIGHT:
					line_content_width_emu = cm_to_emu(para_total_width_cm)
					line_left_emu = shape.left + frame_width_emu - line_content_width_emu

				line_content_right_emu = shape.left + cm_to_emu(para_total_width_cm)
				if para_alignment == PP_ALIGN.LEFT or para_alignment is None:
					line_right_emu = min(shape.left + frame_width_emu, line_content_right_emu)
				else:
					line_right_emu = shape.left + frame_width_emu

				line_boxes.append({
					'left': line_left_emu,
					'top': current_top_emu,
					'right': line_right_emu,
					'bottom': current_top_emu + line_total_height_emu,
				})

				current_top_emu += line_total_height_emu

			if para.space_after:
				space_after_emu = cm_to_emu(para.space_after.pt * 0.0353) if hasattr(para.space_after, 'pt') else 0
				current_top_emu += space_after_emu

		return line_boxes
	except Exception:
		return []


def _collect_shapes_info(slide):
	"""
	递归收集幻灯片中所有叶子节点元素的边界框信息（展开 Group）

	对于包含文本的 shape，使用计算出的文本渲染宽高和对齐后的实际位置

	Returns:
		list[dict]: 每项包含 index / shape / left / top / width / height / right / bottom
	"""
	shapes_info = []

	def _get_shape_dimensions(shape):
		if shape.has_text_frame and shape.text.strip():
			return _calculate_text_actual_position(shape)
		return shape.left, shape.top, shape.width, shape.height

	def collect_group_shapes(group_shape, parent_idx):
		for sub_idx, sub_shape in enumerate(group_shape.shapes):
			if sub_shape.shape_type == MSO_SHAPE_TYPE.GROUP:
				collect_group_shapes(sub_shape, f"{parent_idx}.{sub_idx}")
			else:
				left, top, width, height = _get_shape_dimensions(sub_shape)
				shapes_info.append({
					'index': f"{parent_idx}.{sub_idx}",
					'shape': sub_shape,
					'left': left,
					'top': top,
					'width': width,
					'height': height,
					'right': left + width,
					'bottom': top + height,
				})

	for shape_idx, shape in enumerate(slide.shapes):
		if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
			collect_group_shapes(shape, shape_idx)
		else:
			left, top, width, height = _get_shape_dimensions(shape)
			shapes_info.append({
				'index': shape_idx,
				'shape': shape,
				'left': left,
				'top': top,
				'width': width,
				'height': height,
				'right': left + width,
				'bottom': top + height,
			})

	return shapes_info


def check_slide_layout(slide, slide_width, slide_height, slide_idx=None, unit='cm'):
	"""
	检查单页幻灯片的布局问题

	检查项目:
	1. 元素越界 - 元素位置或尺寸超出幻灯片页面范围
	2. 元素重叠 - 文本框或图表被其他元素遮挡

	Args:
		slide:        pptx Slide 对象
		slide_width:  幻灯片宽度（EMU）
		slide_height: 幻灯片高度（EMU）
		slide_idx:    当前页码（1-based，可选）
		unit:         输出单位，支持 'cm' / 'inch' / 'pt' / 'px'，默认 'cm'

	Returns:
		dict: 包含 slide_idx / unit / issue_count / issues
	"""
	issues = []
	shapes_info = _collect_shapes_info(slide)

	def _shape_base_info(shape, index, info=None):
		text_preview = None
		if shape.has_text_frame and shape.text.strip():
			text_preview = shape.text.strip().replace("\n", " ")[:50]
			if len(shape.text.strip()) > 50:
				text_preview += "..."

		if info is not None:
			left_cm = emu_to_cm(info['left'])
			top_cm = emu_to_cm(info['top'])
			width_cm = emu_to_cm(info['width'])
			height_cm = emu_to_cm(info['height'])
		else:
			left_cm = emu_to_cm(shape.left)
			top_cm = emu_to_cm(shape.top)
			width_cm = emu_to_cm(shape.width)
			height_cm = emu_to_cm(shape.height)

		return {
			"shape_index": str(index),
			"shape_id":    str(shape.shape_id),
			"shape_type":  shape.shape_type.name,
			"position": {
				"left": _convert_unit(left_cm, unit),
				"top":  _convert_unit(top_cm, unit),
			},
			"size": {
				"width":  _convert_unit(width_cm, unit),
				"height": _convert_unit(height_cm, unit),
			},
			"text_preview": text_preview,
		}

	# 1. 检查元素越界
	for info in shapes_info:
		shape = info['shape']
		details = []

		if info['left'] < 0:
			overflow_cm = abs(emu_to_cm(info['left']))
			if overflow_cm < 1:
				continue
			overflow_val = _convert_unit(overflow_cm, unit)
			details.append(f"左边界越界 {overflow_val}{unit}")
		if info['top'] < 0:
			overflow_cm = abs(emu_to_cm(info['top']))
			if overflow_cm < 1:
				continue
			overflow_val = _convert_unit(overflow_cm, unit)
			details.append(f"上边界越界 {overflow_val}{unit}")
		if info['right'] > slide_width:
			overflow_cm = emu_to_cm(info['right'] - slide_width)
			if overflow_cm < 1:
				continue
			overflow_val = _convert_unit(overflow_cm, unit)
			details.append(f"右边界越界 +{overflow_val}{unit}")
		if info['bottom'] > slide_height:
			overflow_cm = emu_to_cm(info['bottom'] - slide_height)
			if overflow_cm < 1:
				continue
			overflow_val = _convert_unit(overflow_cm, unit)
			details.append(f"下边界越界 +{overflow_val}{unit}")

		if details:
			issue = _shape_base_info(shape, info['index'], info)
			issue["type"] = "元素越界"
			issue["details"] = details
			issues.append(issue)

	# 2. 检查元素重叠
	for i, info_a in enumerate(shapes_info):
		shape_a = info_a['shape']
		is_text = shape_a.has_text_frame and shape_a.text.strip()
		is_chart = shape_a.has_chart

		if not (is_text or is_chart):
			continue

		text_line_boxes = _get_text_line_boxes(shape_a) if is_text else []

		for j, info_b in enumerate(shapes_info):
			if i >= j:
				continue

			if not _is_overlap(info_a, info_b):
				continue

			if text_line_boxes:
				actual_overlap = False
				for line_box in text_line_boxes:
					if _is_overlap_rect(line_box, info_b):
						actual_overlap = True
						break
				if not actual_overlap:
					continue

			overlap_area = _calculate_overlap_area(info_a, info_b)
			overlap_width_cm = emu_to_cm(overlap_area['width'])
			overlap_height_cm = emu_to_cm(overlap_area['height'])
			overlap_area_cm2 = overlap_width_cm * overlap_height_cm

			if overlap_area_cm2 < 0.2:
				continue

			overlap_width_unit = _convert_unit(overlap_width_cm, unit)
			overlap_height_unit = _convert_unit(overlap_height_cm, unit)
			overlap_area_unit = round(overlap_width_unit * overlap_height_unit, 2)

			blocked_type = "文本" if is_text else "图表"

			issue = _shape_base_info(shape_a, info_a['index'], info_a)
			issue["type"] = "元素重叠"
			issue["details"] = [f"{blocked_type}被遮挡，重叠面积 {overlap_area_unit} {unit}²"]
			issue["blocked_by"] = _shape_base_info(info_b['shape'], info_b['index'], info_b)
			issue["overlap_area"] = overlap_area_unit
			issues.append(issue)

	return {
		"slide_idx":   slide_idx,
		"unit":        unit,
		"issue_count": len(issues),
		"issues":      issues,
	}


def check_layouts(presentation, unit='cm'):
	"""
	检查演示文稿中每个幻灯片的布局问题

	Args:
		presentation: pptx.Presentation对象
		unit:         输出单位，支持 'cm' / 'inch' / 'pt' / 'px'，默认 'cm'

	Returns:
		dict: 包含 unit / slide_size / total_issue_count / slides
	"""
	slide_width = presentation.slide_width
	slide_height = presentation.slide_height

	slides_results = []
	total_issues = 0

	for slide_idx, slide in enumerate(presentation.slides, start=1):
		slide_result = check_slide_layout(slide, slide_width, slide_height, slide_idx, unit)
		if slide_result["issue_count"] > 0:
			slides_results.append(slide_result)
			total_issues += slide_result["issue_count"]

	slide_width_cm = emu_to_cm(slide_width)
	slide_height_cm = emu_to_cm(slide_height)

	return {
		"unit": unit,
		"slide_size": {
			"width": _convert_unit(slide_width_cm, unit),
			"height": _convert_unit(slide_height_cm, unit),
		},
		"total_issue_count": total_issues,
		"slides": slides_results,
	}


def _is_overlap_rect(rect_a, rect_b):
	"""判断两个矩形是否重叠（接收 left/top/right/bottom 字典）"""
	horizontal_separated = rect_a['right'] <= rect_b['left'] or rect_b['right'] <= rect_a['left']
	vertical_separated = rect_a['bottom'] <= rect_b['top'] or rect_b['bottom'] <= rect_a['top']
	return not (horizontal_separated or vertical_separated)


def _is_overlap(info_a, info_b):
	"""判断两个矩形是否重叠"""
	horizontal_separated = info_a['right'] <= info_b['left'] or info_b['right'] <= info_a['left']
	vertical_separated = info_a['bottom'] <= info_b['top'] or info_b['bottom'] <= info_a['top']
	return not (horizontal_separated or vertical_separated)


def _calculate_overlap_area(info_a, info_b):
	"""计算两个重叠矩形的交集区域"""
	overlap_left = max(info_a['left'], info_b['left'])
	overlap_top = max(info_a['top'], info_b['top'])
	overlap_right = min(info_a['right'], info_b['right'])
	overlap_bottom = min(info_a['bottom'], info_b['bottom'])

	return {
		'left': overlap_left,
		'top': overlap_top,
		'width': overlap_right - overlap_left,
		'height': overlap_bottom - overlap_top
	}


def _format_shape_issue(shape, index, issue_type, issue_detail):
	"""格式化单个元素的问题描述"""
	lines = []
	lines.append(f"【{issue_type}】元素 [{index}] ID:{shape.shape_id}")
	lines.append(f"  类型: {shape.shape_type.name}")
	lines.append(f"  位置: ({emu_to_cm(shape.left)}, {emu_to_cm(shape.top)}) cm")
	lines.append(f"  尺寸: {emu_to_cm(shape.width)} x {emu_to_cm(shape.height)} cm")

	if shape.has_text_frame and shape.text.strip():
		text_preview = shape.text.strip().replace("\n", " ")[:50]
		if len(shape.text.strip()) > 50:
			text_preview += "..."
		lines.append(f"  内容: \"{text_preview}\"")

	if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
		lines.append(f"  图片")

	if shape.has_table:
		table = shape.table
		lines.append(f"  表格: {len(table.rows)} x {len(table.columns)}")

	lines.append(f"  问题: {issue_detail}")

	return "\n".join(lines)


def _format_overlap_issue(shape_a, index_a, shape_b, index_b, overlap_area_cm2, blocked_type="元素"):
	"""格式化元素重叠问题描述"""
	lines = []
	lines.append(f"【{blocked_type}被遮挡】元素 [{index_a}] ID:{shape_a.shape_id} 被 元素 [{index_b}] ID:{shape_b.shape_id} 遮挡")

	lines.append(f"  元素A: {shape_a.shape_type.name}")
	lines.append(f"    位置: ({emu_to_cm(shape_a.left)}, {emu_to_cm(shape_a.top)}) cm")
	lines.append(f"    尺寸: {emu_to_cm(shape_a.width)} x {emu_to_cm(shape_a.height)} cm")
	if shape_a.has_text_frame and shape_a.text.strip():
		text_preview = shape_a.text.strip().replace("\n", " ")[:30]
		lines.append(f"    内容: \"{text_preview}...\"")

	lines.append(f"  元素B: {shape_b.shape_type.name}")
	lines.append(f"    位置: ({emu_to_cm(shape_b.left)}, {emu_to_cm(shape_b.top)}) cm")
	lines.append(f"    尺寸: {emu_to_cm(shape_b.width)} x {emu_to_cm(shape_b.height)} cm")
	if shape_b.has_text_frame and shape_b.text.strip():
		text_preview = shape_b.text.strip().replace("\n", " ")[:30]
		lines.append(f"    内容: \"{text_preview}...\"")

	lines.append(f"  重叠面积: {overlap_area_cm2} cm²")

	return "\n".join(lines)
