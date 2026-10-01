from __future__ import annotations

import os
import re
import sys
from functools import lru_cache
from PIL import Image, ImageDraw, ImageFont

from pptx_colors import EMU_PER_CM, emu_to_cm, cm_to_emu, _resolve_text_color


@lru_cache(maxsize=1)
def _get_system_dpi() -> int:
	"""
	获取系统 DPI 设置（跨平台）

	优先级：
	1. Windows: 通过 ctypes 读取系统 DPI
	2. macOS: 默认 72 DPI（Retina 显示器逻辑分辨率）
	3. Linux: 通过 Xlib 读取 X Server DPI，失败则读取 Xft.dpi
	4. 回退: 96 DPI（标准值）

	Returns:
		int: 系统 DPI 值
	"""
	# Windows
	if sys.platform == 'win32':
		try:
			import ctypes
			user32 = ctypes.windll.user32
			user32.SetProcessDPIAware()
			hdc = user32.GetDC(0)
			dpi = ctypes.windll.gdi32.GetDeviceCaps(hdc, 88)  # LOGPIXELSX = 88
			user32.ReleaseDC(0, hdc)
			if dpi > 0:
				return dpi
		except Exception:
			pass

	# macOS
	elif sys.platform == 'darwin':
		try:
			return 72
		except Exception:
			pass

	# Linux / Unix
	elif sys.platform.startswith('linux') or sys.platform.startswith('freebsd'):
		try:
			from Xlib import display
			d = display.Display()
			screen = d.screen()
			width_px = screen.width_in_pixels
			width_mm = screen.width_in_mms
			if width_mm > 0:
				dpi = int(width_px / width_mm * 25.4)
				if dpi > 0:
					return dpi
		except Exception:
			pass

		try:
			xrdb_output = os.popen('xrdb -query 2>/dev/null | grep "Xft.dpi"').read().strip()
			if xrdb_output:
				match = re.search(r'Xft\.dpi:\s*(\d+)', xrdb_output)
				if match:
					dpi = int(match.group(1))
					if dpi > 0:
						return dpi
		except Exception:
			pass

	return 96


def _convert_unit(value_cm: float, unit: str) -> float:
	"""
	将厘米值转换为指定单位

	Args:
		value_cm: 厘米值
		unit: 目标单位 ('cm', 'inch', 'pt', 'px')

	Returns:
		转换后的值
	"""
	if unit == 'cm':
		return round(value_cm, 2)
	elif unit == 'inch':
		return round(value_cm / 2.54, 2)
	elif unit == 'pt':
		return round(value_cm / 0.0353, 2)
	elif unit == 'px':
		dpi = _get_system_dpi()
		return round(value_cm * dpi / 2.54, 2)
	else:
		raise ValueError(f"不支持的单位: {unit}，支持的单位: cm, inch, pt, px")


def _is_cjk_char(ch: str) -> bool:
	"""判断字符是否为 CJK（中日韩）字符"""
	cp = ord(ch)
	return (
		0x4E00 <= cp <= 0x9FFF
		or 0x3400 <= cp <= 0x4DBF
		or 0x20000 <= cp <= 0x2A6DF
		or 0xF900 <= cp <= 0xFAFF
		or 0x2E80 <= cp <= 0x2EFF
		or 0x3000 <= cp <= 0x303F
		or 0xFF00 <= cp <= 0xFFEF
	)


def _has_cjk(text: str) -> bool:
	"""判断字符串是否包含 CJK 字符"""
	return any(_is_cjk_char(ch) for ch in text)


@lru_cache(maxsize=None)
def _scan_system_fonts() -> dict:
	"""
	扫描系统字体目录，使用 fontTools 解析字体名称，
	返回 {规范化字体名(小写): 字体文件路径} 的映射字典。
	"""
	font_name_to_path: dict = {}

	font_dirs = []
	if os.name == 'nt':
		windir = os.environ.get('WINDIR', 'C:\\Windows')
		font_dirs = [
			os.path.join(windir, 'Fonts'),
		]
		local_appdata = os.environ.get('LOCALAPPDATA', '')
		if local_appdata:
			font_dirs.append(os.path.join(local_appdata, 'Microsoft', 'Windows', 'Fonts'))
	else:
		font_dirs = [
			'/usr/share/fonts',
			'/usr/local/share/fonts',
			os.path.expanduser('~/.fonts'),
			os.path.expanduser('~/Library/Fonts'),
			'/Library/Fonts',
			'/System/Library/Fonts',
		]

	try:
		from fontTools.ttLib import TTFont, TTCollection
	except ImportError:
		return font_name_to_path

	_FONT_EXTS = {'.ttf', '.otf', '.ttc', '.otc'}

	for font_dir in font_dirs:
		if not os.path.isdir(font_dir):
			continue
		for fname in os.listdir(font_dir):
			ext = os.path.splitext(fname)[1].lower()
			if ext not in _FONT_EXTS:
				continue
			fpath = os.path.join(font_dir, fname)
			try:
				if ext in ('.ttc', '.otc'):
					collection = TTCollection(fpath)
					for i, tt in enumerate(collection.fonts):
						_register_font_names(tt, fpath, font_name_to_path, ttc_index=i)
				else:
					tt = TTFont(fpath, fontNumber=0)
					_register_font_names(tt, fpath, font_name_to_path)
			except Exception:
				pass

	return font_name_to_path


def _register_font_names(tt, fpath: str, mapping: dict, ttc_index: int = 0):
	"""从 TTFont 对象中读取所有 name 记录并写入 mapping"""
	try:
		name_table = tt['name']
	except Exception:
		return

	collected_names = set()
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


@lru_cache(maxsize=256)
def _find_font_path(font_name: str) -> tuple | None:
	"""
	根据字体名称在系统字体映射中查找对应的 (文件路径, ttc_index)。
	查找顺序：精确匹配 → 前缀匹配 → 包含匹配。
	返回 (path, index) 或 None。
	"""
	if not font_name:
		return None

	font_map = _scan_system_fonts()
	key = font_name.strip().lower()

	if key in font_map:
		return font_map[key]

	for k, v in font_map.items():
		if k.startswith(key) or key.startswith(k):
			return v

	for k, v in font_map.items():
		if key in k or k in key:
			return v

	return None


def _estimate_text_size(text: str, font_size_pt: float, is_bold: bool) -> tuple:
	"""
	使用估算公式计算文本尺寸（无法加载字体时的回退方案）。

	规则：
	  - CJK 字符宽度 = 1.0 × 字号
	  - 其他字符宽度  = 0.6 × 字号
	  - 粗体宽度额外增加 10%
	  - 高度 = 1.2 × 字号
	  - pt → cm: 1pt ≈ 0.0353cm
	"""
	PT_TO_CM = 0.0353
	est_width_pt = sum(
		font_size_pt * (1.0 if _is_cjk_char(ch) else 0.6)
		for ch in text
	)
	if is_bold:
		est_width_pt *= 1.1
	est_height_pt = font_size_pt * 1.2
	return round(est_width_pt * PT_TO_CM, 2), round(est_height_pt * PT_TO_CM, 2)


def calculate_text_size(text: str, font_name: str, font_size_pt: float, is_bold: bool = False) -> tuple:
	"""
	计算文本的实际渲染宽度和高度。

	优先使用 fontTools 扫描系统字体，通过 Pillow 精确测量；
	若字体无法加载，则按字符类型（CJK / 非CJK）使用估算公式。

	Args:
		text:         文本内容
		font_name:    字体名称（如 "微软雅黑"、"Arial"）
		font_size_pt: 字号（pt）
		is_bold:      是否粗体

	Returns:
		(width_cm, height_cm): 文本宽度和高度（厘米）
	"""
	if not text:
		return 0.0, 0.0

	font_size_px = int(font_size_pt * _get_system_dpi() / 72)

	try:
		font_info = _find_font_path(font_name)
		if font_info is not None:
			fpath, ttc_index = font_info
			pil_font = ImageFont.truetype(fpath, font_size_px, index=ttc_index)

			img = Image.new('RGB', (1, 1))
			width_px = pil_font.getlength(text)
			ascent, descent = pil_font.getmetrics()
			height_px = ascent + descent

			if is_bold:
				width_px *= 1.03

			DPI = _get_system_dpi()
			PPTWIDTHEXTEND = 1.05
			width_cm = round(width_px * 2.54 / DPI, 2) * PPTWIDTHEXTEND
			height_cm = round(height_px * 2.54 / DPI, 2)
			return width_cm, height_cm
	except Exception:
		pass

	return _estimate_text_size(text, font_size_pt, is_bold)


def calculate_text_frame_size(text_frame, text_frame_width_cm, slide=None):
	"""
	计算文本框内文本的实际占用尺寸(考虑换行、行距、段落间距)

	Args:
		text_frame: pptx TextFrame对象
		text_frame_width_cm: 文本框宽度(厘米)

	Returns:
		dict: {
			'total_width_cm': 文本内容总宽度,
			'total_height_cm': 文本内容总高度(含行距、段落间距),
			'word_wrap': 是否自动换行,
			'styles': 样式信息列表
		}
	"""
	word_wrap = text_frame.word_wrap
	styles = []
	total_height_cm = 0
	max_width_cm = 0

	para_count = len(text_frame.paragraphs)

	for para_idx, para in enumerate(text_frame.paragraphs):
		para_width_cm = 0
		para_max_height_cm = 0

		line_spacing = 1.0
		is_fixed_line_spacing = False
		fixed_line_spacing_cm = 0

		if para.line_spacing:
			if hasattr(para.line_spacing, 'pt'):
				is_fixed_line_spacing = True
				fixed_line_spacing_cm = para.line_spacing.pt * 0.0353
			else:
				line_spacing = float(para.line_spacing)

		space_before_cm = 0
		space_after_cm = 0
		if para.space_before:
			space_before_cm = para.space_before.pt * 0.0353 if hasattr(para.space_before, 'pt') else 0
		if para.space_after:
			space_after_cm = para.space_after.pt * 0.0353 if hasattr(para.space_after, 'pt') else 0

		if para_idx > 0:
			total_height_cm += space_before_cm

		for run in para.runs:
			run_text = run.text.strip()
			if run_text:
				style_info = []
				font_name = None
				font_size_pt = 18
				is_bold = False

				if run.font.name:
					font_name = run.font.name
					style_info.append(f"font={run.font.name}")
				if run.font.size:
					font_size_pt = run.font.size.pt
					style_info.append(f"size={font_size_pt}pt")
				if run.font.bold:
					is_bold = True
					style_info.append("bold")
				if run.font.italic:
					style_info.append("italic")
				try:
					ci = _resolve_text_color(run, slide)
					if ci:
						if "color" in ci:
							style_info.append(f"color={ci['color']}")
						elif "theme" in ci:
							style_info.append(f"color_theme={ci['theme']}")
				except Exception:
					pass

				width_cm, height_cm = calculate_text_size(
					run_text,
					font_name or 'Arial',
					font_size_pt,
					is_bold
				)

				para_width_cm += width_cm
				para_max_height_cm = max(para_max_height_cm, height_cm)

				style_info.append(f"render_size={width_cm}*{height_cm}cm")

				if style_info:
					styles.append(f"({', '.join(style_info)})")

		if word_wrap and para_width_cm > text_frame_width_cm and text_frame_width_cm > 0:
			lines = int(para_width_cm / text_frame_width_cm) + 1
			if is_fixed_line_spacing:
				total_height_cm += fixed_line_spacing_cm * lines
			else:
				total_height_cm += para_max_height_cm * lines * line_spacing
			max_width_cm = max(max_width_cm, text_frame_width_cm)
		else:
			if is_fixed_line_spacing:
				total_height_cm += fixed_line_spacing_cm
			else:
				total_height_cm += para_max_height_cm * line_spacing
			max_width_cm = max(max_width_cm, para_width_cm)

		if para_idx < para_count - 1:
			total_height_cm += space_after_cm

	return {
		'total_width_cm': round(max_width_cm, 2),
		'total_height_cm': round(total_height_cm, 2),
		'word_wrap': word_wrap,
		'styles': styles
	}
