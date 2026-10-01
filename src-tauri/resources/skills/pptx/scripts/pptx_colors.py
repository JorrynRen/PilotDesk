from __future__ import annotations

import os
from functools import lru_cache
from pptx.enum.shapes import MSO_SHAPE_TYPE

EMU_PER_CM = 360000

_NS_A = 'http://schemas.openxmlformats.org/drawingml/2006/main'
_NS_P = 'http://schemas.openxmlformats.org/presentationml/2006/main'

_PRESET_COLOR_MAP = {
    "white": "#FFFFFF", "black": "#000000", "red": "#FF0000",
    "green": "#008000", "blue": "#0000FF", "yellow": "#FFFF00",
    "cyan": "#00FFFF", "magenta": "#FF00FF", "silver": "#C0C0C0",
    "gray": "#808080", "grey": "#808080", "maroon": "#800000",
    "olive": "#808000", "purple": "#800080", "teal": "#008080",
    "navy": "#000080", "orange": "#FFA500", "pink": "#FFC0CB",
    "aqua": "#00FFFF", "fuchsia": "#FF00FF", "lime": "#00FF00",
    "dkBlue": "#00008B", "dkCyan": "#008B8B", "dkGoldenrod": "#B8860B",
    "dkGray": "#A9A9A9", "dkGrey": "#A9A9A9", "dkGreen": "#006400",
    "dkMagenta": "#8B008B", "dkOliveGreen": "#556B2F", "dkOrange": "#FF8C00",
    "dkRed": "#8B0000", "dkViolet": "#9400D3",
    "ltBlue": "#ADD8E6", "ltCoral": "#F08080", "ltCyan": "#E0FFFF",
    "ltGoldenrodYellow": "#FAFAD2", "ltGray": "#D3D3D3", "ltGrey": "#D3D3D3",
    "ltGreen": "#90EE90", "ltPink": "#FFB6C1", "ltSalmon": "#FFA07A",
    "ltSkyBlue": "#87CEFA", "ltYellow": "#FFFFE0",
    "aliceBlue": "#F0F8FF", "antiqueWhite": "#FAEBD7", "beige": "#F5F5DC",
    "bisque": "#FFE4C4", "blueViolet": "#8A2BE2", "brown": "#A52A2A",
    "cadetBlue": "#5F9EA0", "chocolate": "#D2691E", "coral": "#FF7F50",
    "cornflowerBlue": "#6495ED", "cornsilk": "#FFF8DC", "crimson": "#DC143C",
    "darkBlue": "#00008B", "darkCyan": "#008B8B", "darkGray": "#A9A9A9",
    "darkGreen": "#006400", "darkMagenta": "#8B008B", "darkOrange": "#FF8C00",
    "darkRed": "#8B0000", "darkViolet": "#9400D3",
    "deepPink": "#FF1493", "deepSkyBlue": "#00BFFF", "dimGray": "#696969",
    "dodgerBlue": "#1E90FF", "firebrick": "#B22222", "forestGreen": "#228B22",
    "gold": "#FFD700", "goldenrod": "#DAA520", "greenYellow": "#ADFF2F",
    "hotPink": "#FF69B4", "indianRed": "#CD5C5C", "indigo": "#4B0082",
    "ivory": "#FFFFF0", "khaki": "#F0E68C", "lavender": "#E6E6FA",
    "lawnGreen": "#7CFC00", "lemonChiffon": "#FFFACD",
    "lightBlue": "#ADD8E6", "lightCoral": "#F08080", "lightCyan": "#E0FFFF",
    "lightGray": "#D3D3D3", "lightGreen": "#90EE90", "lightPink": "#FFB6C1",
    "lightSkyBlue": "#87CEFA", "lightYellow": "#FFFFE0",
    "limeGreen": "#32CD32", "mediumBlue": "#0000CD",
    "mediumOrchid": "#BA55D3", "mediumPurple": "#9370DB",
    "mediumSeaGreen": "#3CB371", "mediumSpringGreen": "#00FA9A",
    "mediumTurquoise": "#48D1CC", "mediumVioletRed": "#C71585",
    "midnightBlue": "#191970", "mintCream": "#F5FFFA",
    "mistyRose": "#FFE4E1", "moccasin": "#FFE4B5",
    "oldLace": "#FDF5E6", "oliveDrab": "#6B8E23", "orangeRed": "#FF4500",
    "orchid": "#DA70D6", "paleGreen": "#98FB98", "paleTurquoise": "#AFEEEE",
    "paleVioletRed": "#DB7093", "peachPuff": "#FFDAB9", "peru": "#CD853F",
    "plum": "#DDA0DD", "powderBlue": "#B0E0E6", "rosyBrown": "#BC8F8F",
    "royalBlue": "#4169E1", "salmon": "#FA8072", "sandyBrown": "#F4A460",
    "seaGreen": "#2E8B57", "sienna": "#A0522D", "skyBlue": "#87CEEB",
    "slateBlue": "#6A5ACD", "slateGray": "#708090", "snow": "#FFFAFA",
    "springGreen": "#00FF7F", "steelBlue": "#4682B4", "tan": "#D2B48C",
    "thistle": "#D8BFD8", "tomato": "#FF6347", "turquoise": "#40E0D0",
    "violet": "#EE82EE", "wheat": "#F5DEB3", "whiteSmoke": "#F5F5F5",
    "yellowGreen": "#9ACD32",
}


def emu_to_cm(v):
	return round(v / EMU_PER_CM, 2)

def cm_to_emu(v):
	return round(v * EMU_PER_CM)


@lru_cache(maxsize=16)
def _get_theme_colors_for_master(master_part):
    """
    解析母版对应的主题颜色映射，返回 dict: {'tx1': '#000000', 'bg1': '#FFFFFF', ...}
    """
    try:
        from lxml import etree
        root_master = etree.fromstring(master_part.blob)
        ns_p = {"p": "http://schemas.openxmlformats.org/presentationml/2006/main"}
        clr_map_elem = root_master.find(".//p:clrMap", namespaces=ns_p)
        
        clr_map = {}
        if clr_map_elem is not None:
            clr_map = dict(clr_map_elem.attrib)
        else:
            clr_map = {
                "bg1": "lt1", "tx1": "dk1", "bg2": "lt2", "tx2": "dk2",
                "accent1": "accent1", "accent2": "accent2", "accent3": "accent3",
                "accent4": "accent4", "accent5": "accent5", "accent6": "accent6",
                "hlink": "hlink", "folHlink": "folHlink"
            }
            
        theme_part = None
        for rel in master_part.rels.values():
            if "theme" in rel.reltype:
                theme_part = rel.target_part
                break
                
        if theme_part:
            root_theme = etree.fromstring(theme_part.blob)
            ns_a = {"a": "http://schemas.openxmlformats.org/drawingml/2006/main"}
            clr_scheme = root_theme.find(".//a:themeElements/a:clrScheme", namespaces=ns_a)
            if clr_scheme is None:
                clr_scheme = root_theme.find(".//a:clrScheme", namespaces=ns_a)
            
            theme_colors = {}
            if clr_scheme is not None:
                for child in clr_scheme:
                    tag_name = child.tag.split("}")[-1]
                    clr_elem = child[0] if len(child) > 0 else None
                    if clr_elem is not None:
                        clr_tag = clr_elem.tag.split("}")[-1]
                        if clr_tag == 'srgbClr':
                            val = clr_elem.get('val')
                            if val: theme_colors[tag_name] = "#" + val.upper()
                        elif clr_tag == 'sysClr':
                            val = clr_elem.get('lastClr')
                            if val:
                                theme_colors[tag_name] = "#" + val.upper()
                            else:
                                theme_colors[tag_name] = clr_elem.get('val')
                                
            final_map = {}
            for scheme_name, theme_key in clr_map.items():
                if theme_key in theme_colors:
                    final_map[scheme_name] = theme_colors[theme_key]
            final_map.update(theme_colors)
            return final_map
    except Exception:
        pass
    return {}


def _extract_color_info(color_choice_element):
    """从 OOXML 颜色选择元素（如 solidFill, gradFill.stop 下的颜色子元素）中提取颜色信息。

    支持 srgbClr / schemeClr / sysClr 三种颜色定义方式，
    统一提取为标准化的 dict。

    Args:
        color_choice_element: lxml Element，颜色定义的直接父元素，
            例如 <a:solidFill> 或 <a:gs>。

    Returns:
        dict | None: 颜色信息，例如:
            {"color": "#FF0000", "color_type": "srgb"}
            {"color": "#C41E3A", "color_type": "scheme", "theme": "accent1", "lumMod": "75000"}
            {"color": "#000000", "color_type": "sys", "sys_val": "windowText"}
            None 表示未找到任何颜色定义
    """
    if color_choice_element is None:
        return None

    result = {}

    srgb_clr = color_choice_element.find(f'{{{_NS_A}}}srgbClr')
    if srgb_clr is not None:
        val = srgb_clr.get('val')
        if val:
            result["color"] = f"#{val.upper()}"
            result["color_type"] = "srgb"
        return result or None

    scheme_clr = color_choice_element.find(f'{{{_NS_A}}}schemeClr')
    if scheme_clr is not None:
        theme_val = scheme_clr.get('val')
        if theme_val:
            result["theme"] = theme_val
        result["color_type"] = "scheme"
        lum_mod = scheme_clr.find(f'{{{_NS_A}}}lumMod')
        if lum_mod is not None:
            result["lumMod"] = lum_mod.get('val')
        lum_off = scheme_clr.find(f'{{{_NS_A}}}lumOff')
        if lum_off is not None:
            result["lumOff"] = lum_off.get('val')
        return result or None

    sys_clr = color_choice_element.find(f'{{{_NS_A}}}sysClr')
    if sys_clr is not None:
        last_clr = sys_clr.get('lastClr')
        sys_val = sys_clr.get('val')
        if last_clr:
            result["color"] = f"#{last_clr.upper()}"
        result["color_type"] = "sys"
        if sys_val:
            result["sys_val"] = sys_val
        return result or None

    prst_clr = color_choice_element.find(f'{{{_NS_A}}}prstClr')
    if prst_clr is not None:
        val = (prst_clr.get('val') or '').lower()
        if val:
            hex_color = _PRESET_COLOR_MAP.get(val)
            if hex_color:
                result["color"] = hex_color
                result["color_type"] = "preset"
                return result
        return None

    return None


def _find_solid_fill_element(parent_element):
    """在父元素（如 spPr）下查找 <a:solidFill> 元素。"""
    if parent_element is None:
        return None
    return parent_element.find(f'{{{_NS_A}}}solidFill')

def _extract_run_color(rPr_elem):
    """从 rPr 或 defRPr 中提取颜色（支持 solidFill 和 gradFill）。

    渐变填充时返回所有色标，并根据面积占比加权选取主色。
    """
    if rPr_elem is None:
        return None
    solid_fill = _find_solid_fill_element(rPr_elem)
    if solid_fill is not None:
        return _extract_color_info(solid_fill)
    grad_fill = rPr_elem.find(f'{{{_NS_A}}}gradFill')
    if grad_fill is not None:
        gs_lst = grad_fill.find(f'{{{_NS_A}}}gsLst')
        if gs_lst is not None:
            stops = []
            for gs in gs_lst.findall(f'{{{_NS_A}}}gs'):
                ci = _extract_color_info(gs)
                if ci:
                    pos = gs.get('pos', '0')
                    ci["pos"] = pos
                    stops.append(ci)
            if stops:
                dominant = max(stops, key=lambda s: int(s.get("pos", "0")))
                result = {k: v for k, v in dominant.items() if k != "pos"}
                result["gradient"] = True
                result["gradient_stops"] = [
                    {k: v for k, v in s.items()} for s in stops
                ]
                lin = grad_fill.find(f'{{{_NS_A}}}lin')
                if lin is not None:
                    result["gradient_angle"] = lin.get('ang')
                return result
    return None

def _apply_lum_modifiers(hex_color, lum_mod=None, lum_off=None):
    """对 hex 颜色应用 lumMod / lumOff 亮度修饰，返回修改后的 hex 颜色。

    OOXML 亮度修饰公式（基于 HSL 色彩空间）：
        L' = L × (lumMod / 100000) + (lumOff / 100000)
    """
    if lum_mod is None and lum_off is None:
        return hex_color
    try:
        import colorsys
        h = hex_color.lstrip("#")
        r, g, b = int(h[0:2], 16) / 255, int(h[2:4], 16) / 255, int(h[4:6], 16) / 255
        hue, lig, sat = colorsys.rgb_to_hls(r, g, b)
        if lum_mod is not None:
            lig = lig * int(lum_mod) / 100000
        if lum_off is not None:
            lig = lig + int(lum_off) / 100000
        lig = max(0.0, min(1.0, lig))
        r2, g2, b2 = colorsys.hls_to_rgb(hue, lig, sat)
        return f"#{int(r2*255+0.5):02X}{int(g2*255+0.5):02X}{int(b2*255+0.5):02X}"
    except Exception:
        return hex_color


def _resolve_theme_color(ci, slide):
    """尝试将主题颜色解析为实际的 RGB 颜色，并应用 lumMod / lumOff 修饰。"""
    if ci and "theme" in ci and slide is not None:
        try:
            master_part = slide.slide_layout.slide_master.part
            theme_colors = _get_theme_colors_for_master(master_part)
            theme_val = ci["theme"]
            if theme_val in theme_colors:
                color = theme_colors[theme_val]
                color = _apply_lum_modifiers(color, ci.get("lumMod"), ci.get("lumOff"))
                resolved = {"color": color, "color_type": "srgb", "from_theme": theme_val}
                for k, v in ci.items():
                    if k not in ("theme", "color_type", "color", "lumMod", "lumOff"):
                        resolved[k] = v
                return resolved
        except Exception:
            pass
    if ci and "color" in ci:
        color = ci["color"]
        color = _apply_lum_modifiers(color, ci.get("lumMod"), ci.get("lumOff"))
        ci["color"] = color
    return ci

_PH_XML_TYPE_TO_TXSTYLE = {
    "body": "bodyStyle", "subTitle": "bodyStyle", "tbl": "bodyStyle",
    "obj": "bodyStyle",
    "title": "titleStyle", "ctrTitle": "titleStyle",
}


def _read_rpr_props(rpr_elem, slide):
    """从 <a:rPr> 或 <a:defRPr> 元素提取 size_pt / color / font_name，仅返回非空字段。"""
    if rpr_elem is None:
        return {}
    out = {}
    sz = rpr_elem.get("sz")
    if sz:
        try:
            out["size_pt"] = round(int(sz) / 100, 1)
        except (TypeError, ValueError):
            pass
    for tag in ("ea", "latin", "cs"):
        font_node = rpr_elem.find(f"{{{_NS_A}}}{tag}")
        if font_node is not None:
            typeface = font_node.get("typeface")
            if typeface and not typeface.startswith("+"):
                out["font_name"] = typeface
                break
    try:
        ci = _extract_run_color(rpr_elem)
        if ci:
            ci = _resolve_theme_color(ci, slide)
            c = ci.get("color")
            if c:
                out["color"] = c if c.startswith("#") else f"#{c}"
    except Exception:
        pass
    return out


def _resolve_placeholder_inherited_props(shape_elem, slide):
    """沿占位符继承链（layout ph → master ph → master txStyles）解析文字样式。

    仅在 shape 是占位符时有效；非占位符返回空 dict。

    Returns:
        dict: 含 size_pt / color / font_name 等字段（setdefault 先到先得），可能为空。
    """
    if slide is None or shape_elem is None:
        return {}

    try:
        nv_sp_pr = shape_elem.find(f'{{{_NS_P}}}nvSpPr')
        if nv_sp_pr is None:
            return {}
        nv_pr = nv_sp_pr.find(f'{{{_NS_P}}}nvPr')
        if nv_pr is None:
            return {}
        ph = nv_pr.find(f'{{{_NS_P}}}ph')
        if ph is None:
            return {}
    except Exception:
        return {}

    idx = ph.get('idx', '0')
    ph_type = ph.get('type', '')

    result = {}

    def _merge(props):
        for k, v in props.items():
            result.setdefault(k, v)

    def _scan_text_body(tx_body):
        if tx_body is None:
            return
        for p in tx_body.findall(f'{{{_NS_A}}}p'):
            for r in p.findall(f'{{{_NS_A}}}r'):
                _merge(_read_rpr_props(r.find(f'{{{_NS_A}}}rPr'), slide))
            ppr = p.find(f'{{{_NS_A}}}pPr')
            if ppr is not None:
                _merge(_read_rpr_props(ppr.find(f'{{{_NS_A}}}defRPr'), slide))
        lst_style = tx_body.find(f'{{{_NS_A}}}lstStyle')
        if lst_style is not None:
            lvl1 = lst_style.find(f'{{{_NS_A}}}lvl1pPr')
            if lvl1 is not None:
                _merge(_read_rpr_props(lvl1.find(f'{{{_NS_A}}}defRPr'), slide))

    try:
        layout = slide.slide_layout
    except Exception:
        return result

    try:
        for lph in layout.placeholders:
            if str(lph.placeholder_format.idx) == str(idx):
                _scan_text_body(lph._element.find(f'{{{_NS_P}}}txBody'))
                break
    except Exception:
        pass

    try:
        master = layout.slide_master
    except Exception:
        return result

    if master is not None:
        try:
            for mph in master.placeholders:
                if str(mph.placeholder_format.idx) == str(idx):
                    _scan_text_body(mph._element.find(f'{{{_NS_P}}}txBody'))
                    break
        except Exception:
            pass

        try:
            branch_name = _PH_XML_TYPE_TO_TXSTYLE.get(ph_type, "otherStyle")
            tx_styles = master._element.find(f'{{{_NS_P}}}txStyles')
            if tx_styles is not None:
                branch = tx_styles.find(f'{{{_NS_P}}}{branch_name}')
                if branch is not None:
                    lvl1 = branch.find(f'{{{_NS_A}}}lvl1pPr')
                    if lvl1 is not None:
                        _merge(_read_rpr_props(lvl1.find(f'{{{_NS_A}}}defRPr'), slide))
        except Exception:
            pass

    return result


def _resolve_text_color(run, slide=None):
    """
    解析文本的最终颜色。
    优先级：run rPr → paragraph defRPr → 占位符继承链 → shape style fontRef → tx1 兜底。
    """
    try:
        run_rPr = run._r.find(f'{{{_NS_A}}}rPr')
        ci = _extract_run_color(run_rPr)
        if ci: return _resolve_theme_color(ci, slide)

        pPr = run._parent._p.find(f'{{{_NS_A}}}pPr')
        if pPr is not None:
            defRPr = pPr.find(f'{{{_NS_A}}}defRPr')
            ci = _extract_run_color(defRPr)
            if ci: return _resolve_theme_color(ci, slide)

        # 占位符文本样式继承（layout → master → txStyles）
        try:
            shape_elem = run._r.getparent().getparent().getparent()
            props = _resolve_placeholder_inherited_props(shape_elem, slide)
            c = props.get("color")
            if c: return {"color": c, "color_type": "srgb"}
        except Exception:
            pass

        shape = run._parent._parent._parent
        if shape is not None and hasattr(shape, "_element"):
            style = shape._element.find(f'.//{{{_NS_A}}}style')
            if style is not None:
                fontRef = style.find(f'{{{_NS_A}}}fontRef')
                if fontRef is not None:
                    ci = _extract_color_info(fontRef)
                    if ci: return _resolve_theme_color(ci, slide)
    except Exception:
        pass

    return _resolve_theme_color({"theme": "tx1", "color_type": "scheme"}, slide)

def get_fill_info(fill, shape=None, slide=None):
	"""解析填充信息，返回 dict"""
	try:
		fill_type = fill.type
		result = {"type": str(fill_type)}

		if fill_type == 1:  # SOLID
			try:
				fill_parent = getattr(fill, '_xPr', None)
				if fill_parent is None:
					fill_parent = getattr(fill, '_element', None)
				solid_fill = _find_solid_fill_element(fill_parent)
				if solid_fill is not None:
					color_info = _extract_color_info(solid_fill)
					if color_info:
						color_info = _resolve_theme_color(color_info, slide)
						result.update(color_info)
			except Exception:
				pass

		elif fill_type == 6:  # PICTURE
			if shape is not None:
				try:
					blip = shape.element.xpath(".//a:blip")
					if blip:
						rId = blip[0].get("{http://schemas.openxmlformats.org/officeDocument/2006/relationships}embed")
						if rId:
							rel = shape.part.rels[rId]
							image_part = rel.target_part
							result["fill_image"] = str(image_part.partname)
				except Exception as xpath_error:
					result["xpath_error"] = str(xpath_error)

		elif fill_type == 3:  # GRADIENT
			result["gradient"] = True
			try:
				fill_parent = getattr(fill, '_xPr', None)
				if fill_parent is None:
					fill_parent = getattr(fill, '_element', None)
				if fill_parent is not None:
					grad_fill = fill_parent.find(f'{{{_NS_A}}}gradFill')
					if grad_fill is not None:
						lin = grad_fill.find(f'{{{_NS_A}}}lin')
						if lin is not None:
							result["gradient_angle"] = lin.get('ang')
							result["gradient_scaled"] = lin.get('scaled')
						gs_list = grad_fill.findall(f'{{{_NS_A}}}gsLst/{{{_NS_A}}}gs')
						if gs_list:
							stops = []
							for gs in gs_list:
								pos = gs.get('pos', '')
								color_info = _extract_color_info(gs)
								stop = {"pos": pos}
								if color_info:
									color_info = _resolve_theme_color(color_info, slide)
									stop.update(color_info)
								stops.append(stop)
							result["gradient_stops"] = stops
			except Exception:
				pass

		elif fill_type == 4:  # PATTERN
			result["pattern"] = True
			try:
				fill_parent = getattr(fill, '_xPr', None)
				if fill_parent is None:
					fill_parent = getattr(fill, '_element', None)
				if fill_parent is not None:
					patt_fill = fill_parent.find(f'{{{_NS_A}}}pattFill')
					if patt_fill is not None:
						prst = patt_fill.get('prst')
						if prst:
							result["pattern_preset"] = prst
						fg_color = _extract_color_info(patt_fill)
						if fg_color:
							fg_color = _resolve_theme_color(fg_color, slide)
							result["pattern_fg"] = fg_color
						bg_clr = patt_fill.find(f'{{{_NS_A}}}bgClr')
						if bg_clr is not None:
							bg_color = _extract_color_info(bg_clr)
							if bg_color:
								bg_color = _resolve_theme_color(bg_color, slide)
								result["pattern_bg"] = bg_color
			except Exception:
				pass

		return result
	except Exception as e:
		return {"error": str(e)}


def get_line_info(line, slide=None):
	"""解析线条信息，返回 dict"""
	try:
		result = {}

		if line.width:
			result["width_pt"] = round(line.width.pt, 2)

		try:
			_parent = getattr(line, '_parent', None)
			if _parent is not None:
				_sp = getattr(_parent, '_sp', None)
				if _sp is not None:
					spPr = _sp.spPr
					ln_elem = spPr.find(f'{{{_NS_A}}}ln')
					if ln_elem is not None:
						solid_fill = _find_solid_fill_element(ln_elem)
						if solid_fill is not None:
							color_info = _extract_color_info(solid_fill)
							if color_info:
								color_info = _resolve_theme_color(color_info, slide)
								result.update(color_info)
		except Exception:
			pass

		if hasattr(line, 'dash_style') and line.dash_style:
			result["dash_style"] = str(line.dash_style)

		return result if result else None
	except Exception as e:
		return {"error": str(e)}


def get_slide_background(slide):
    """
    提取幻灯片的背景填充信息。

    按优先级尝试：slide 自身 → slide layout → slide master。

    Args:
        slide: pptx Slide 对象

    Returns:
        dict | None: 背景填充信息（与 get_fill_info 格式一致），
            额外包含 "source" 字段标识来源（"slide" / "layout" / "master"）。
            无显式背景时返回 None。
    """
    sources = [
        ("slide", slide),
        ("layout", slide.slide_layout),
        ("master", slide.slide_layout.slide_master),
    ]
    for source_name, obj in sources:
        try:
            fill = obj.background.fill
            fill_type = fill.type
            if fill_type is not None and fill_type != 5:
                info = get_fill_info(fill)
                if info:
                    info["source"] = source_name
                    return info
        except Exception:
            continue
    return None
