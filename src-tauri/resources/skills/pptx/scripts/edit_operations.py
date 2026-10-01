"""
PPT 幻灯片页面批量组织操作模块

支持对 pptx 文件完成页面的增、删、复制、重排序，
可在一次调用中以操作列表的形式完成多页处理。
"""

import copy
import os
import tempfile
from lxml import etree
from pptx.util import Cm, Pt


EMU_PER_CM = 360000  # 1 cm = 360000 EMU
# 顶层 cNvPr 的 XPath 路径，用于精确定位 shape 的名称和 id 属性
_TOP_CNVPR_PATHS = [
	'{http://schemas.openxmlformats.org/presentationml/2006/main}nvSpPr/{http://schemas.openxmlformats.org/presentationml/2006/main}cNvPr',           # p:sp
	'{http://schemas.openxmlformats.org/presentationml/2006/main}nvGrpSpPr/{http://schemas.openxmlformats.org/presentationml/2006/main}cNvPr',        # p:grpSp
	'{http://schemas.openxmlformats.org/presentationml/2006/main}nvCxnSpPr/{http://schemas.openxmlformats.org/presentationml/2006/main}cNvPr',        # p:cxnSp
	'{http://schemas.openxmlformats.org/presentationml/2006/main}nvGraphicFramePr/{http://schemas.openxmlformats.org/presentationml/2006/main}cNvPr', # p:graphicFrame
	'{http://schemas.openxmlformats.org/presentationml/2006/main}nvPicPr/{http://schemas.openxmlformats.org/presentationml/2006/main}cNvPr',              # p:pic
]
# ─────────────────────────────────────────────────────────────────────────────
# 内部辅助函数
# ─────────────────────────────────────────────────────────────────────────────

def _qn(tag: str) -> str:
    """将 'p:xxx' / 'r:xxx' 转换为带命名空间的完整标签名"""
    nsmap = {
        'p': 'http://schemas.openxmlformats.org/presentationml/2006/main',
        'r': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships',
        'a': 'http://schemas.openxmlformats.org/drawingml/2006/main',
    }
    prefix, local = tag.split(':', 1)
    return f"{{{nsmap[prefix]}}}{local}"


def _get_sldIdLst(prs):
    """获取演示文稿 XML 中的 sldIdLst 节点"""
    return prs._element.find(_qn('p:sldIdLst'))


def _get_sldIds(prs):
    """返回当前所有 sldId 元素列表（顺序即页面顺序）"""
    return list(_get_sldIdLst(prs).findall(_qn('p:sldId')))


# ─────────────────────────────────────────────────────────────────────────────
# 单页原子操作
# ─────────────────────────────────────────────────────────────────────────────

def _delete_slide(prs, slide_index: int) -> None:
    """
    删除指定索引的幻灯片（索引从 0 开始）。

    Args:
        prs: Presentation 对象
        slide_index: 要删除的页面索引（0-based）
    """
    total = len(prs.slides)
    if slide_index < 0 or slide_index >= total:
        raise IndexError(f"slide_index={slide_index} 超出范围 [0, {total - 1}]")

    slide = prs.slides[slide_index]

    # 找到对应的 relationship id
    rId = None
    for rel_key, rel in prs.part.rels.items():
        if rel.target_part == slide.part:
            rId = rel_key
            break

    if rId is None:
        raise ValueError(f"未找到第 {slide_index} 页的 relationship")

    # 从 sldIdLst 中移除该 sldId 节点
    sldIdLst = _get_sldIdLst(prs)
    for sldId in sldIdLst.findall(_qn('p:sldId')):
        if sldId.get(_qn('r:id')) == rId:
            sldIdLst.remove(sldId)
            break

    # 移除关系
    prs.part.rels._rels.pop(rId, None)


def _move_slide(prs, old_index: int, new_index: int) -> None:
    """
    将幻灯片从 old_index 移动到 new_index（索引从 0 开始）。

    Args:
        prs: Presentation 对象
        old_index: 原始页面索引（0-based）
        new_index: 目标页面索引（0-based）
    """
    total = len(prs.slides)
    if old_index < 0 or old_index >= total:
        raise IndexError(f"old_index={old_index} 超出范围 [0, {total - 1}]")
    if new_index < 0 or new_index >= total:
        raise IndexError(f"new_index={new_index} 超出范围 [0, {total - 1}]")
    if old_index == new_index:
        return

    sldIdLst = _get_sldIdLst(prs)
    sldIds = _get_sldIds(prs)

    el = sldIds[old_index]
    sldIdLst.remove(el)

    # 重新获取移除后的列表
    remaining = list(sldIdLst.findall(_qn('p:sldId')))
    if new_index >= len(remaining):
        sldIdLst.append(el)
    else:
        remaining[new_index].addprevious(el)


def _duplicate_slide(prs, slide_index: int):
    """
    复制指定幻灯片并追加到末尾（含图片资源处理）。

    Args:
        prs: Presentation 对象
        slide_index: 源页面索引（0-based）

    Returns:
        新建的 Slide 对象
    """
    total = len(prs.slides)
    if slide_index < 0 or slide_index >= total:
        raise IndexError(f"slide_index={slide_index} 超出范围 [0, {total - 1}]")

    source_slide = prs.slides[slide_index]

    # 基于相同布局新建页面
    new_slide = prs.slides.add_slide(source_slide.slide_layout)

    # 替换 spTree 内容（复制所有 shape XML）
    new_spTree = new_slide._element.find(_qn('p:cSld')).find(_qn('p:spTree'))
    old_spTree = source_slide._element.find(_qn('p:cSld')).find(_qn('p:spTree'))

    # 删除新页的默认元素（保留结构节点）
    for child in list(new_spTree):
        tag = etree.QName(child).localname
        if tag not in ('nvGrpSpPr', 'grpSpPr'):
            new_spTree.remove(child)

    # 深拷贝源页面的所有元素
    for child in old_spTree:
        tag = etree.QName(child).localname
        if tag not in ('nvGrpSpPr', 'grpSpPr'):
            new_spTree.append(copy.deepcopy(child))

    # 处理图片资源：重新写入 relationship 并替换图片 shape
    _copy_image_rels(source_slide, new_slide)

    return new_slide


def _copy_image_rels(source_slide, target_slide) -> None:
    """
    将源幻灯片中的图片 relationship 完整复制到目标幻灯片，
    并修正目标幻灯片 spTree 中图片 shape 的 rEmbed 引用。

    Args:
        source_slide: 源 Slide 对象
        target_slide: 目标 Slide 对象
    """
    from pptx.opc.constants import RELATIONSHIP_TYPE as RT

    # 收集源页面中所有图片 rel（image rId → image part）
    src_image_rels = {}
    for rId, rel in source_slide.part.rels.items():
        if 'image' in rel.reltype:
            src_image_rels[rId] = rel

    if not src_image_rels:
        return

    # rId 映射表：源 rId → 目标 rId
    rId_map = {}
    for src_rId, rel in src_image_rels.items():
        new_rId = target_slide.part.relate_to(rel.target_part, rel.reltype)
        rId_map[src_rId] = new_rId

    # 修正目标 slide XML 中 <p:blipFill> 下 <a:blip r:embed="..."> 的引用
    spTree = target_slide._element.find(_qn('p:cSld')).find(_qn('p:spTree'))
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

    for blip in spTree.iter(f'{{{a_ns}}}blip'):
        old_embed = blip.get(f'{{{r_ns}}}embed')
        if old_embed and old_embed in rId_map:
            blip.set(f'{{{r_ns}}}embed', rId_map[old_embed])


# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：批量页面组织
# ─────────────────────────────────────────────────────────────────────────────

def _build_orig_index_map(prs) -> dict:
    """
    构建"原始索引 → sldId 唯一标识"的快照映射表。

    key: 初始位置（int），value: sldId 元素的 'id' 属性字符串。
    用于在多操作场景中，将调用者传入的原始索引转换为当前实际索引。
    """
    sldIds = _get_sldIds(prs)
    return {i: sldId.get("id") for i, sldId in enumerate(sldIds)}


def _resolve_index(prs, orig_index: int, orig_map: dict) -> int:
    """
    将原始索引（基于初始状态）转换为当前实际索引。

    通过 sldId 的唯一 'id' 属性在当前 sldIdLst 中定位页面，
    从而屏蔽中间操作导致的索引偏移。

    Args:
        prs: Presentation 对象
        orig_index: 调用者传入的原始索引（0-based，基于函数调用前的状态）
        orig_map: 由 _build_orig_index_map 生成的快照映射表

    Returns:
        当前实际索引（0-based）

    Raises:
        IndexError: 原始索引不存在于映射表，或对应页面已被删除
    """
    total_orig = len(orig_map)
    if orig_index < 0 or orig_index >= total_orig:
        raise IndexError(
            f"原始索引 {orig_index} 超出初始范围 [0, {total_orig - 1}]"
        )

    slide_id = orig_map.get(orig_index)
    if slide_id is None:
        raise IndexError(f"原始索引 {orig_index} 不存在于映射表")

    sldIds = _get_sldIds(prs)
    for current_pos, sldId in enumerate(sldIds):
        if sldId.get("id") == slide_id:
            return current_pos

    raise IndexError(
        f"原始索引 {orig_index}（sldId.id={slide_id}）对应的页面已被删除，无法操作"
    )


def slide_organize(prs, operations: list) -> dict:
    """
    对 pptx 的幻灯片页面执行批量组织操作（增、删、复制、重排序）。

    支持在一次调用中按顺序执行多个操作，每个操作为一个 dict。

    **索引语义（重要）**：所有操作中的索引均基于函数调用时的初始状态，
    即"原始索引"。函数内部会自动追踪每次操作后的页面位移，
    无需调用者手动计算偏移量。

    Args:
        prs: python-pptx 的 Presentation 对象
        operations: 操作列表，每个元素为一个 dict，格式如下：

            删除:
                {
                    "action": "delete",
                    "index": int            # 要删除的页面原始索引（0-based，基于初始状态）
                }

            复制:
                {
                    "action": "copy",
                    "index": int,           # 源页面原始索引（0-based，基于初始状态）
                    "position": int         # 可选，复制后插入的位置（基于初始状态），默认追加到末尾
                }

            移动（重排序）:
                {
                    "action": "move",
                    "from_index": int,      # 源原始索引（0-based，基于初始状态）
                    "to_index": int         # 目标原始索引（0-based，基于初始状态）
                }

    Returns:
        执行结果 dict::

            {
                "success": bool,
                "total_slides": int,        # 操作完成后的总页数
                "operations_count": int,    # 成功执行的操作数
                "errors": [                 # 失败的操作信息列表
                    {"operation_index": int, "operation": dict, "error": str},
                    ...
                ]
            }

    Examples:
        >>> from pptx import Presentation
        >>> prs = Presentation("demo.pptx")

        # 1. 删除原第3页，然后将原第1页移动到原第4页位置
        #    两个索引均基于调用前的初始状态，无需关心删除后的偏移
        >>> result = slide_organize(prs, [
        ...     {"action": "delete", "index": 2},
        ...     {"action": "move", "from_index": 0, "to_index": 3},
        ... ])

        # 2. 在原第2页位置复制原第0页，再复制原第0页到末尾
        >>> result = slide_organize(prs, [
        ...     {"action": "copy", "index": 0, "position": 1},
        ...     {"action": "copy", "index": 0},
        ... ])

        # 3. 对5页 PPT 重新排序为 [3, 0, 4, 1, 2]（直接指定每页的目标位置）
        #    target_order[new_pos] = orig_idx 表示：原第 orig_idx 页移动到第 new_pos 位
        >>> target_order = [3, 0, 4, 1, 2]
        >>> ops = [
        ...     {"action": "move", "from_index": orig_idx, "to_index": new_pos}
        ...     for new_pos, orig_idx in enumerate(target_order)
        ... ]
        >>> result = slide_organize(prs, ops)

        >>> prs.save("demo_edited.pptx")
    """
    if not isinstance(operations, list) or len(operations) == 0:
        return {
            "success": False,
            "total_slides": len(prs.slides),
            "operations_count": 0,
            "errors": [{"operation_index": -1, "operation": None,
                        "error": "operations 参数必须是非空列表"}],
        }

    # ── 快照：记录初始状态下 原始索引 → sldId 唯一标识 ──────────────────────
    orig_map = _build_orig_index_map(prs)
    # insert/copy 新增页面的 sldId 映射（key 为虚拟原始索引，从 len(orig_map) 起增长）
    # 新增页面不能被后续操作通过"原始索引"引用，但 position 参数会通过 orig_map 转换
    orig_count = len(orig_map)

    errors = []
    success_count = 0

    for i, op in enumerate(operations):
        if not isinstance(op, dict):
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": "操作必须是 dict 类型",
            })
            continue

        action = op.get("action", "").lower()

        try:
            if action == "delete":
                # ── 删除 ──────────────────────────────────────────────────
                orig_index = int(op["index"])
                real_index = _resolve_index(prs, orig_index, orig_map)
                _delete_slide(prs, real_index)
                # 从映射表中移除已删除的页面，避免后续操作误引用
                orig_map.pop(orig_index, None)

            elif action == "copy":
                # ── 复制页面 ──────────────────────────────────────────────
                orig_index = int(op["index"])
                real_index = _resolve_index(prs, orig_index, orig_map)
                new_slide = _duplicate_slide(prs, real_index)
                # 如果指定了目标位置，则移动过去
                if "position" in op:
                    orig_position = int(op["position"])
                    if orig_position >= orig_count:
                        real_position = len(prs.slides) - 1
                    else:
                        real_position = _resolve_index(prs, orig_position, orig_map)
                    current_index = len(prs.slides) - 1
                    if real_position != current_index:
                        _move_slide(prs, current_index, real_position)
                # 将新页面登记到映射表
                final_pos = len(prs.slides) - 1 if "position" not in op else real_position
                new_sldId = _get_sldIds(prs)[final_pos]
                new_orig_key = orig_count
                orig_map[new_orig_key] = new_sldId.get("id")
                orig_count += 1

            elif action == "move":
                # ── 移动（重排序） ────────────────────────────────────────
                orig_from = int(op["from_index"])
                orig_to = int(op["to_index"])
                real_from = _resolve_index(prs, orig_from, orig_map)
                real_to = _resolve_index(prs, orig_to, orig_map)
                _move_slide(prs, real_from, real_to)

            else:
                raise ValueError(
                    f"不支持的 action='{action}'，"
                    "可选值：delete / copy / move"
                )

            success_count += 1

        except (KeyError, TypeError) as exc:
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": f"参数错误：{exc}",
            })
        except (IndexError, ValueError) as exc:
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": str(exc),
            })

    return {
        "success": len(errors) == 0,
        "total_slides": len(prs.slides),
        "operations_count": success_count,
        "errors": errors,
    }

def get_shape(slide, shape_id: int):
	"""
	按 shape_id（cNvPr id 属性）获取 shape，支持递归检索 group 内部的 shape。

	Args:
		slide: 幻灯片对象
		shape_id: shape 的数字 id（即 cNvPr 的 id 属性）

	Returns:
		找到的 shape 对象

	Raises:
		ValueError: 未找到对应 shape_id 的 shape
	"""
	p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'
	pic_ns = 'http://schemas.openxmlformats.org/drawingml/2006/picture'
	target_id_str = str(shape_id)

	def _read_id_from_xml(elm):
		"""直接从 XML 读取 cNvPr id，避免 python-pptx 代理属性问题。"""
		# pic 使用 p:nvPicPr/p:cNvPr（注意：pic 的 nv/cNvPr 也在 p 命名空间下）
		cNvPr = elm.find(f'{{{p_ns}}}nvPicPr/{{{p_ns}}}cNvPr')
		if cNvPr is not None:
			return cNvPr.get('id', '')
		# 普通形状
		for nv_tag in ('nvSpPr', 'nvGrpSpPr', 'nvCxnSpPr', 'nvGraphicFramePr'):
			cNvPr = elm.find(f'{{{p_ns}}}{nv_tag}/{{{p_ns}}}cNvPr')
			if cNvPr is not None:
				return cNvPr.get('id', '')
		return ''

	def search_in_shapes(shapes, target_id):
		for shape in shapes:
			if _read_id_from_xml(shape._element) == target_id:
				return shape
			if hasattr(shape, 'shape_type') and shape.shape_type == 6:
				if hasattr(shape, 'shapes'):
					result = search_in_shapes(shape.shapes, target_id)
					if result:
						return result
		return None

	result = search_in_shapes(slide.shapes, target_id_str)
	if result is None:
		raise ValueError(f"未找到 shape_id={shape_id} 的 shape")
	return result

# ─────────────────────────────────────────────────────────────────────────────
# 新增辅助函数：处理组内元素坐标变换
# ─────────────────────────────────────────────────────────────────────────────

def _get_parent_groups(shape):
    """获取所有祖先 group（从外到内排序）"""
    groups = []
    parent = shape._element.getparent()

    while parent is not None:
        if etree.QName(parent).localname == 'grpSp':
            groups.append(parent)
        parent = parent.getparent()

    groups.reverse()
    return groups

def _parse_group_transform(grp_elm):
    """解析 group 的 off / chOff / scale"""
    p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

    grpSpPr = grp_elm.find(f'{{{p_ns}}}grpSpPr')
    if grpSpPr is None:
        return None

    xfrm = grpSpPr.find(f'{{{a_ns}}}xfrm')
    if xfrm is None:
        return None

    off = xfrm.find(f'{{{a_ns}}}off')
    ext = xfrm.find(f'{{{a_ns}}}ext')
    chOff = xfrm.find(f'{{{a_ns}}}chOff')
    chExt = xfrm.find(f'{{{a_ns}}}chExt')

    if None in (off, ext, chOff, chExt):
        return None

    off_x = int(off.get('x', '0'))
    off_y = int(off.get('y', '0'))

    ch_x = int(chOff.get('x', '0'))
    ch_y = int(chOff.get('y', '0'))

    ext_cx = int(ext.get('cx', '0'))
    ext_cy = int(ext.get('cy', '0'))
    ch_cx = int(chExt.get('cx', '0'))
    ch_cy = int(chExt.get('cy', '0'))

    if ch_cx == 0 or ch_cy == 0:
        scale_x = scale_y = 1.0
    else:
        scale_x = ext_cx / ch_cx
        scale_y = ext_cy / ch_cy

    return {
        "off_x": off_x,
        "off_y": off_y,
        "ch_x": ch_x,
        "ch_y": ch_y,
        "scale_x": scale_x,
        "scale_y": scale_y,
    }

def _abs_to_local(shape, abs_x, abs_y):
    """
    将绝对坐标 → shape 局部坐标（逆变换）

    对于多层group嵌套，需要从外到内逐层进行逆变换。
    正向变换：child_x = parent_off_x + (child_local_x - parent_chOff_x) * parent_scale_x
    逆变换：child_local_x = (child_x - parent_off_x) / parent_scale_x + parent_chOff_x

    注意：正向变换是从内到外（从shape到幻灯片），逆变换是从外到内（从幻灯片到shape）。
    """
    groups = _get_parent_groups(shape)

    # 如果没有父group，直接返回绝对坐标
    if not groups:
        return abs_x, abs_y

    x, y = abs_x, abs_y

    # 从外到内逐层进行逆变换（groups是从外到内排序的）
    for grp in groups:
        tf = _parse_group_transform(grp)
        if not tf:
            continue

        # 逆变换：从父group的坐标系转换为当前group的内部坐标系
        # 正向变换：x = tf["off_x"] + (child_x - tf["ch_x"]) * tf["scale_x"]
        # 逆变换：child_x = (x - tf["off_x"]) / tf["scale_x"] + tf["ch_x"]
        x = (x - tf["off_x"]) / tf["scale_x"] + tf["ch_x"]
        y = (y - tf["off_y"]) / tf["scale_y"] + tf["ch_y"]

    return x, y

def _get_cumulative_scale(shape):
    """获取累计缩放（用于尺寸）"""
    groups = _get_parent_groups(shape)

    sx = sy = 1.0
    for grp in groups:
        tf = _parse_group_transform(grp)
        if not tf:
            continue
        sx *= tf["scale_x"]
        sy *= tf["scale_y"]

    return sx, sy

def _get_shape_absolute_pos_size(shape) -> dict:
	"""
	获取 shape 在幻灯片级别的绝对位置和尺寸（EMU）。

	对于顶层 shape（非 group 子元素），直接返回其 left/top/width/height。
	对于 group 内部的子 shape，python-pptx 的 .left/.top/.width/.height
	返回的是相对于 group 坐标系（chOff/chExt）的**内部坐标**，需要通过
	group 的缩放因子转换为幻灯片级别的绝对坐标。

	OOXML 中 group 坐标系的关系：
	  - group 外框: off(x, y), ext(cx, cy)     -- 幻灯片坐标系
	  - group 内框: chOff(x, y), chExt(cx, cy)  -- 内部坐标系
	  - 缩放因子: scaleX = ext.cx / chExt.cx,  scaleY = ext.cy / chExt.cy
	  - 子 shape 的绝对位置 = off + (child_off - chOff) * scale

	Args:
		shape: python-pptx 的 shape 对象

	Returns:
		dict 包含 left, top, width, height（均为 EMU 整数）
	"""
	a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
	p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'

	shape_elm = shape._element

	# 获取 shape 自身的 off/ext
	xfrm = None
	spPr = shape_elm.find(f'{{{p_ns}}}spPr')
	if spPr is not None:
		xfrm = spPr.find(f'{{{a_ns}}}xfrm')
	if xfrm is None:
		grpSpPr = shape_elm.find(f'{{{p_ns}}}grpSpPr')
		if grpSpPr is not None:
			xfrm = grpSpPr.find(f'{{{a_ns}}}xfrm')
	if xfrm is None:
		xfrm = shape_elm.find(f'.//{{{a_ns}}}xfrm')

	if xfrm is None:
		return {
			'left': int(shape.left),
			'top': int(shape.top),
			'width': int(shape.width),
			'height': int(shape.height),
		}

	off_el = xfrm.find(f'{{{a_ns}}}off')
	ext_el = xfrm.find(f'{{{a_ns}}}ext')
	child_x = int(off_el.get('x', '0')) if off_el is not None else 0
	child_y = int(off_el.get('y', '0')) if off_el is not None else 0
	child_cx = int(ext_el.get('cx', '0')) if ext_el is not None else 0
	child_cy = int(ext_el.get('cy', '0')) if ext_el is not None else 0

	# 向上查找所有祖先 group，逐级累加坐标变换
	parent_elm = shape_elm.getparent()
	while parent_elm is not None:
		parent_tag = etree.QName(parent_elm).localname
		if parent_tag == 'grpSp':
			parent_grpSpPr = parent_elm.find(f'{{{p_ns}}}grpSpPr')
			if parent_grpSpPr is None:
				break
			parent_xfrm = parent_grpSpPr.find(f'{{{a_ns}}}xfrm')
			if parent_xfrm is None:
				break

			p_off = parent_xfrm.find(f'{{{a_ns}}}off')
			p_ext = parent_xfrm.find(f'{{{a_ns}}}ext')
			p_chOff = parent_xfrm.find(f'{{{a_ns}}}chOff')
			p_chExt = parent_xfrm.find(f'{{{a_ns}}}chExt')

			if p_off is None or p_ext is None or p_chOff is None or p_chExt is None:
				break

			grp_x = int(p_off.get('x', '0'))
			grp_y = int(p_off.get('y', '0'))
			grp_cx = int(p_ext.get('cx', '0'))
			grp_cy = int(p_ext.get('cy', '0'))
			ch_x = int(p_chOff.get('x', '0'))
			ch_y = int(p_chOff.get('y', '0'))
			ch_cx = int(p_chExt.get('cx', '0'))
			ch_cy = int(p_chExt.get('cy', '0'))

			if ch_cx <= 0 or ch_cy <= 0:
				break

			scale_x = grp_cx / ch_cx
			scale_y = grp_cy / ch_cy

			child_x = grp_x + (child_x - ch_x) * scale_x
			child_y = grp_y + (child_y - ch_y) * scale_y
			child_cx = child_cx * scale_x
			child_cy = child_cy * scale_y

		parent_elm = parent_elm.getparent()

	return {
		'left': int(child_x),
		'top': int(child_y),
		'width': int(child_cx),
		'height': int(child_cy),
	}

def _set_xfrm(new_elm, left_emu: int, top_emu: int, width_emu: int, height_emu: int) -> bool:
	"""
	在 new_elm 的直接子结构中定位 xfrm 节点并更新 off/ext。

	支持以下结构：
	  - 普通 shape / 文本框：<p:sp><p:spPr><a:xfrm>
	  - 图片：              <p:pic><p:spPr><a:xfrm>
	  - group shape：       <p:grpSp><p:grpSpPr><a:xfrm>

	对于 group shape（当用户确实要复制 group 整体时），只修改 off（绝对位置），
	保留原始 chOff/chExt 和子 shape 坐标不变，兼容 WPS 私有行为。

	Args:
		new_elm:    shape 的 XML 元素
		left_emu:   左边距（EMU）
		top_emu:    上边距（EMU）
		width_emu:  宽度（EMU）
		height_emu: 高度（EMU）

	Returns:
		True 表示成功找到并修改，False 表示未找到 xfrm。
	"""
	a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
	p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'

	xfrm = None

	# 1. <p:spPr><a:xfrm>（普通 shape / 文本框 / 图片）
	spPr = new_elm.find(f'{{{p_ns}}}spPr')
	if spPr is not None:
		xfrm = spPr.find(f'{{{a_ns}}}xfrm')

	# 2. <p:grpSpPr><a:xfrm>（group shape）
	if xfrm is None:
		grpSpPr = new_elm.find(f'{{{p_ns}}}grpSpPr')
		if grpSpPr is not None:
			xfrm = grpSpPr.find(f'{{{a_ns}}}xfrm')

	# 3. 兜底：全局查找第一个 xfrm（connector / freeform 等）
	if xfrm is None:
		xfrm = new_elm.find(f'.//{{{a_ns}}}xfrm')

	if xfrm is None:
		return False

	tag_local = etree.QName(new_elm).localname
	if tag_local == 'grpSp':
		# group shape: 只修改 off，不碰 chOff/chExt
		off = xfrm.find(f'{{{a_ns}}}off')
		if off is not None:
			off.set('x', str(left_emu))
			off.set('y', str(top_emu))
	else:
		off = xfrm.find(f'{{{a_ns}}}off')
		ext = xfrm.find(f'{{{a_ns}}}ext')
		if off is not None:
			off.set('x', str(left_emu))
			off.set('y', str(top_emu))
		if ext is not None:
			ext.set('cx', str(width_emu))
			ext.set('cy', str(height_emu))

	return True




# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：文本修改
# ─────────────────────────────────────────────────────────────────────────────

def _apply_para_format(para, fmt: dict) -> None:
    """
    将段落格式 fmt 应用到 python-pptx 的 Paragraph 对象上。

    支持的格式键：
        alignment       : 对齐方式，字符串 "left"/"center"/"right"/"justify"
        space_before_pt : 段前间距（磅）
        space_after_pt  : 段后间距（磅）
        line_spacing_pt : 行间距（磅）；传 None 保持不变
        level           : 列表缩进级别（0-8）
    """
    from pptx.util import Pt
    from pptx.enum.text import PP_ALIGN

    _ALIGN_MAP = {
        "left":    PP_ALIGN.LEFT,
        "center":  PP_ALIGN.CENTER,
        "right":   PP_ALIGN.RIGHT,
        "justify": PP_ALIGN.JUSTIFY,
    }

    pf = para._pPr  # 可能为 None，需通过属性赋值触发自动创建

    if "alignment" in fmt:
        align_str = str(fmt["alignment"]).lower()
        if align_str in _ALIGN_MAP:
            para.alignment = _ALIGN_MAP[align_str]

    if "level" in fmt:
        para.level = int(fmt["level"])

    if "space_before_pt" in fmt and fmt["space_before_pt"] is not None:
        para.space_before = Pt(float(fmt["space_before_pt"]))

    if "space_after_pt" in fmt and fmt["space_after_pt"] is not None:
        para.space_after = Pt(float(fmt["space_after_pt"]))

    if "line_spacing_pt" in fmt and fmt["line_spacing_pt"] is not None:
        para.line_spacing = Pt(float(fmt["line_spacing_pt"]))


def set_run_font(run, latin: str | None = None, east_asia: str | None = None) -> None:
    """
    在 run 级别分别设置西文字体（latin）和中文/东亚字体（east_asia）。

    python-pptx 的 ``font.name`` 只能修改 ``<a:latin>``，无法直接修改
    ``<a:ea>``（东亚字体），因此本函数通过直接操作底层 XML 实现。

    Args:
        run:       python-pptx 的 Run 对象
        latin:     西文字体名称（如 "Arial"、"Calibri"），None 表示不修改
        east_asia: 东亚/中文字体名称（如 "微软雅黑"、"宋体"），None 表示不修改

    Examples:
        >>> _set_run_font(run, latin="Calibri", east_asia="微软雅黑")
        >>> _set_run_font(run, east_asia="宋体")   # 只改中文字体
    """
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    rPr = run._r.get_or_add_rPr()

    if latin is not None:
        latin_elm = rPr.find(f'{{{a_ns}}}latin')
        if latin_elm is None:
            latin_elm = etree.SubElement(rPr, f'{{{a_ns}}}latin')
        latin_elm.set('typeface', latin)

    if east_asia is not None:
        ea_elm = rPr.find(f'{{{a_ns}}}ea')
        if ea_elm is None:
            ea_elm = etree.SubElement(rPr, f'{{{a_ns}}}ea')
        ea_elm.set('typeface', east_asia)


def _extract_run_format(run) -> dict:
    """
    从已有 run 中提取格式信息为 dict，用于格式继承。
    只提取显式设置过的属性（非 None 的值）。
    """
    fmt = {}
    font = run.font

    if font.bold is not None:
        fmt["bold"] = font.bold
    if font.italic is not None:
        fmt["italic"] = font.italic
    if font.underline is not None:
        fmt["underline"] = font.underline
    if font.size is not None:
        # Pt 是 pptx.util.Length，转成 float 磅值
        fmt["size_pt"] = round(float(font.size) / 12700, 1)

    # 字体名称
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    rPr = run._r.find(f'{{{a_ns}}}rPr')
    if rPr is not None:
        latin_elm = rPr.find(f'{{{a_ns}}}latin')
        ea_elm = rPr.find(f'{{{a_ns}}}ea')
        latin_name = latin_elm.get("typeface") if latin_elm is not None else None
        ea_name = ea_elm.get("typeface") if ea_elm is not None else None
        if latin_name:
            fmt["latin"] = latin_name
        if ea_name:
            fmt["east_asia"] = ea_name

    # 颜色
    try:
        color = font.color.rgb
        if color is not None:
            fmt["color_rgb"] = str(color)
    except (AttributeError, TypeError):
        pass

    # 删除线
    if rPr is not None and rPr.get("strike"):
        fmt["strike"] = rPr.get("strike") == "sngStrike"

    return fmt


def _apply_run_format(run, fmt: dict) -> None:
    """
    将文字格式 fmt 应用到 python-pptx 的 Run 对象上。

    支持的格式键：
        bold            : 是否加粗（bool）
        italic          : 是否斜体（bool）
        underline       : 是否下划线（bool）
        size_pt         : 字号（磅，float）
        color_rgb       : 字体颜色，十六进制字符串，如 "FF0000" 或 "#FF0000"
        font_name       : 字体名称（str），等价于同时设置西文和东亚字体
        latin           : 西文字体名称（str），精确设置 <a:latin> 节点
        east_asia       : 东亚/中文字体名称（str），精确设置 <a:ea> 节点
        strike          : 是否删除线（bool）
    """
    from pptx.dml.color import RGBColor

    font = run.font

    if "bold" in fmt and fmt["bold"] is not None:
        font.bold = bool(fmt["bold"])

    if "italic" in fmt and fmt["italic"] is not None:
        font.italic = bool(fmt["italic"])

    if "underline" in fmt and fmt["underline"] is not None:
        font.underline = bool(fmt["underline"])

    if "strike" in fmt and fmt["strike"] is not None:
        # python-pptx 通过直接操作 XML 设置删除线
        a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
        rPr = run._r.get_or_add_rPr()
        rPr.set("strike", "sngStrike" if fmt["strike"] else "noStrike")

    if "size_pt" in fmt and fmt["size_pt"] is not None:
        font.size = Pt(float(fmt["size_pt"]))

    if "font_name" in fmt and fmt["font_name"] is not None:
        font.name = str(fmt["font_name"])

    if "latin" in fmt and fmt["latin"] is not None:
        set_run_font(run, latin=str(fmt["latin"]))

    if "east_asia" in fmt and fmt["east_asia"] is not None:
        set_run_font(run, east_asia=str(fmt["east_asia"]))

    if "color_rgb" in fmt and fmt["color_rgb"] is not None:
        color_str = str(fmt["color_rgb"]).lstrip("#")
        font.color.rgb = RGBColor.from_string(color_str)


def edit_text(
    prs,
    slide_index: int,
    shape_id: int,
    operations: list | None = None,
) -> dict:
    """
    修改指定幻灯片中的文字内容及格式。

    可通过 ``operations`` 列表对文本框内任意段（paragraph）和
    段内任意 run 进行文字替换、格式变更，支持在一次调用中完成多条修改。

    Args:
        prs:         python-pptx 的 Presentation 对象
        slide_index: 目标幻灯片索引（0-based）
        shape_id:    目标 shape 的数字 id（cNvPr id 属性）
        operations:  操作列表，每个元素为一个 dict，格式如下：

            **修改段落中某个 run 的文字**::

                {
                    "action": "set_run_text",
                    "para_index": int,   # 段落索引（0-based）
                    "run_index":  int,   # run 索引（0-based）
                    "text":       str    # 新文字内容
                }

            **修改段落中某个 run 的格式**::

                {
                    "action": "set_run_format",
                    "para_index": int,   # 段落索引（0-based）
                    "run_index":  int,   # run 索引（0-based）
                    "format": {
                        "bold":       bool,   # 加粗
                        "italic":     bool,   # 斜体
                        "underline":  bool,   # 下划线
                        "strike":     bool,   # 删除线
                        "size_pt":    float,  # 字号（磅）
                        "font_name":  str,    # 字体名称（同时设置西文和东亚）
                        "latin":      str,    # 西文字体（精确设置 <a:latin>）
                        "east_asia":  str,    # 东亚/中文字体（精确设置 <a:ea>）
                        "color_rgb":  str,    # 颜色，如 "FF0000" 或 "#FF0000"
                    }
                }

            **同时修改某 run 的文字和格式**::

                {
                    "action": "set_run",
                    "para_index": int,
                    "run_index":  int,
                    "text":       str,   # 可选，省略则只改格式
                    "format": { ... }    # 可选，省略则只改文字
                }

            **在指定段落末尾追加新 run（段落级操作）**::

                {
                    "action":     "add_run",
                    "para_index": int,   # 目标段落索引（0-based）
                    "text":       str,   # 新 run 的文字内容（可为空字符串）
                    "format": { ... }    # 可选，同 set_run 的 format
                }

                在 para_index 指定段落的末尾追加一个新 run。
                若省略 format，自动继承同段落最后一个已有 run 的样式。
                若传入 format，以 format 为准（覆盖继承的样式）。

            **在文本框末尾追加新段落（文本框级操作）**::

                {
                    "action":     "add_para",
                    "text":       str,   # 新段落的文字内容
                    "format": { ... }    # 可选，同 set_run 的 format，应用于段内唯一的 run
                }

                在文本框所有段落之后追加一个新段落（包含一个 run）。
                若省略 format，自动继承文本框中最后一个已有 run 的样式。
                若传入 format，以 format 为准（覆盖继承的样式）。

            **删除指定段落中的指定 run（段落级操作）**::

                {
                    "action":     "delete_run",
                    "para_index": int,   # 目标段落索引（0-based）
                    "run_index":  int,   # 要删除的 run 索引（0-based）
                }

                删除 para_index 段落中 run_index 指定的 run。
                若段落仅剩一个 run，删除后段落变为空段落。

            **删除指定段落（文本框级操作）**::

                {
                    "action":     "delete_para",
                    "para_index": int,   # 要删除的段落索引（0-based）
                }

                删除文本框中 para_index 指定的段落。

            **替换段落的完整文字（保留第一个 run 的格式，删除多余 run）**::

                {
                    "action": "set_para_text",
                    "para_index": int,
                    "text": str
                }

            **修改段落格式**::

                {
                    "action": "set_para_format",
                    "para_index": int,
                    "format": {
                        "alignment":       str,   # "left"/"center"/"right"/"justify"
                        "space_before_pt": float, # 段前间距（磅）
                        "space_after_pt":  float, # 段后间距（磅）
                        "line_spacing_pt": float, # 行间距（磅）
                        "level":           int,   # 列表缩进级别（0-8）
                    }
                }

    Returns:
        执行结果 dict::

            {
                "success":          bool,
                "slide_index":      int,
                "shape_name":       str,
                "operations_count": int,   # 成功执行的操作数
                "errors": [
                    {"operation_index": int, "operation": dict, "error": str},
                    ...
                ]
            }

    Examples:
        >>> from pptx import Presentation
        >>> prs = Presentation("demo.pptx")

        # 1. 将第 0 页 shape_id=5 的文本框第 0 段第 0 个 run 改为红色加粗
        >>> edit_text(prs, 0, shape_id=5, operations=[
        ...     {
        ...         "action": "set_run",
        ...         "para_index": 0, "run_index": 0,
        ...         "text": "新标题",
        ...         "format": {"bold": True, "color_rgb": "FF0000", "size_pt": 36}
        ...     }
        ... ])

        # 2. 统一设置第 1 段的对齐方式和行间距
        >>> edit_text(prs, 0, shape_id=7, operations=[
        ...     {
        ...         "action": "set_para_format",
        ...         "para_index": 1,
        ...         "format": {"alignment": "center", "line_spacing_pt": 24}
        ...     }
        ... ])

        >>> prs.save("demo_edited.pptx")
    """
    # ── 参数校验 ─────────────────────────────────────────────────────────────
    total = len(prs.slides)
    if slide_index < 0 or slide_index >= total:
        return {
            "success": False,
            "slide_index": slide_index,
            "shape_id": shape_id,
            "operations_count": 0,
            "errors": [{
                "operation_index": -1,
                "operation": None,
                "error": f"slide_index={slide_index} 超出范围 [0, {total - 1}]",
            }],
        }

    if not operations:
        return {
            "success": False,
            "slide_index": slide_index,
            "shape_id": shape_id,
            "operations_count": 0,
            "errors": [{
                "operation_index": -1,
                "operation": None,
                "error": "operations 参数必须是非空列表",
            }],
        }

    # ── 定位 shape ───────────────────────────────────────────────────────────
    slide = prs.slides[slide_index]
    try:
        shape = get_shape(slide, shape_id)
    except (ValueError, IndexError) as exc:
        return {
            "success": False,
            "slide_index": slide_index,
            "shape_id": shape_id,
            "operations_count": 0,
            "errors": [{
                "operation_index": -1,
                "operation": None,
                "error": f"未找到目标 shape：{exc}",
            }],
        }

    # ── 检查 shape 是否含文本框 ───────────────────────────────────────────────
    if not shape.has_text_frame:
        return {
            "success": False,
            "slide_index": slide_index,
            "shape_id": shape_id,
            "operations_count": 0,
            "errors": [{
                "operation_index": -1,
                "operation": None,
                "error": f"shape id={shape_id} 不含文本框（has_text_frame=False）",
            }],
        }

    tf = shape.text_frame
    errors = []
    success_count = 0

    for i, op in enumerate(operations):
        if not isinstance(op, dict):
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": "操作必须是 dict 类型",
            })
            continue

        action = op.get("action", "").lower()

        try:
            # add_para 不需要定位已有段落，其余 action 需要
            if action == "add_para":
                para = None
                para_index = None
            else:
                para_index = int(op.get("para_index", 0))
                if para_index < 0 or para_index >= len(tf.paragraphs):
                    raise IndexError(
                        f"para_index={para_index} 超出范围 [0, {len(tf.paragraphs) - 1}]"
                    )
                para = tf.paragraphs[para_index]

            if action in ("set_run_text", "set_run_format", "set_run"):
                run_index = int(op.get("run_index", 0))
                if run_index < 0 or run_index >= len(para.runs):
                    raise IndexError(
                        f"run_index={run_index} 超出范围 [0, {len(para.runs) - 1}]"
                    )
                run = para.runs[run_index]

                if action == "set_run_text":
                    run.text = str(op["text"])

                elif action == "set_run_format":
                    _apply_run_format(run, op.get("format", {}))

                else:  # set_run
                    if "text" in op:
                        run.text = str(op["text"])
                    if "format" in op:
                        _apply_run_format(run, op["format"])

            elif action == "add_run":
                new_text = str(op.get("text", ""))
                from pptx.oxml.ns import qn as pptx_qn
                a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
                r_elem = etree.SubElement(para._p, f'{{{a_ns}}}r')
                t_elem = etree.SubElement(r_elem, f'{{{a_ns}}}t')
                t_elem.text = new_text
                # 新添加的 run 会自动出现在 para.runs 末尾
                new_run = para.runs[-1]
                if "format" in op and op["format"]:
                    _apply_run_format(new_run, op["format"])
                elif len(para.runs) >= 2:
                    # 无 format 时，自动继承同段落最后一个已有 run 的格式
                    inherited = _extract_run_format(para.runs[-2])
                    if inherited:
                        _apply_run_format(new_run, inherited)

            elif action == "add_para":
                new_text = str(op.get("text", ""))
                from pptx.oxml.ns import qn as pptx_qn
                a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
                p_elem = etree.SubElement(tf._txBody, f'{{{a_ns}}}p')
                r_elem = etree.SubElement(p_elem, f'{{{a_ns}}}r')
                t_elem = etree.SubElement(r_elem, f'{{{a_ns}}}t')
                t_elem.text = new_text
                # 新段落会自动出现在 tf.paragraphs 末尾
                new_run = tf.paragraphs[-1].runs[0]
                if "format" in op and op["format"]:
                    _apply_run_format(new_run, op["format"])
                else:
                    # 无 format 时，自动继承文本框中最后一个已有 run 的格式
                    last_run = None
                    for p in reversed(tf.paragraphs[:-1]):
                        if p.runs:
                            last_run = p.runs[-1]
                            break
                    if last_run is not None:
                        inherited = _extract_run_format(last_run)
                        if inherited:
                            _apply_run_format(new_run, inherited)

            elif action == "delete_run":
                run_index = int(op.get("run_index", 0))
                if run_index < 0 or run_index >= len(para.runs):
                    raise IndexError(
                        f"run_index={run_index} 超出范围 [0, {len(para.runs) - 1}]"
                    )
                r_elem = para.runs[run_index]._r
                r_elem.getparent().remove(r_elem)

            elif action == "delete_para":
                p_elem = para._p
                p_elem.getparent().remove(p_elem)

            elif action == "set_para_text":
                # 保留第一个 run 的格式，删除多余 run，整段文字替换
                new_text = str(op["text"])
                runs = para.runs
                if runs:
                    # 保留第一个 run，设置文字，删除其余 run
                    runs[0].text = new_text
                    for extra_run in runs[1:]:
                        r_elem = extra_run._r
                        r_elem.getparent().remove(r_elem)
                else:
                    # 段落中没有 run，直接通过 XML 添加
                    from pptx.oxml.ns import qn as pptx_qn
                    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
                    r_elem = etree.SubElement(para._p, f'{{{a_ns}}}r')
                    t_elem = etree.SubElement(r_elem, f'{{{a_ns}}}t')
                    t_elem.text = new_text

            elif action == "set_para_format":
                _apply_para_format(para, op.get("format", {}))

            else:
                raise ValueError(
                    f"不支持的 action='{action}'，"
                    "可选值：set_run_text / set_run_format / set_run / "
                    "add_run / delete_run / add_para / delete_para / "
                    "set_para_text / set_para_format"
                )

            success_count += 1

        except (KeyError, TypeError) as exc:
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": f"参数错误：{exc}",
            })
        except (IndexError, ValueError) as exc:
            errors.append({
                "operation_index": i,
                "operation": op,
                "error": str(exc),
            })

    return {
        "success": len(errors) == 0,
        "slide_index": slide_index,
        "shape_id": shape_id,
        "operations_count": success_count,
        "errors": errors,
    }


# ─────────────────────────────────────────────────────────────────────────────
# 内部辅助：复制 shape 内所有图片 relationship 到目标幻灯片
# ─────────────────────────────────────────────────────────────────────────────

def _collect_image_rids(elm) -> set:
    """
    递归收集 XML 元素（及其所有子孙）中所有 <a:blip r:embed="..."> 的 rId 集合。
    用于 group shape 或 pic shape 的图片 relationship 检测。
    """
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    rids = set()
    for blip in elm.iter(f'{{{a_ns}}}blip'):
        rid = blip.get(f'{{{r_ns}}}embed')
        if rid:
            rids.add(rid)
    return rids


def _copy_shape_image_rels(src_slide, dst_slide, src_elm) -> dict:
    """
    将源幻灯片中被 src_elm（及其子孙）引用的图片 relationship 复制到目标幻灯片，
    返回 rId 映射表 {src_rId: dst_rId}，供后续修正 XML 中的 r:embed 属性。

    与 _copy_image_rels 的区别：只复制 src_elm 实际引用的图片 rel，
    适用于单个 shape（包括 group、pic）的精确复制场景。
    """
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

    needed_rids = _collect_image_rids(src_elm)
    if not needed_rids:
        return {}

    rId_map = {}
    for rId, rel in src_slide.part.rels.items():
        if rId in needed_rids and 'image' in rel.reltype:
            new_rId = dst_slide.part.relate_to(rel.target_part, rel.reltype)
            rId_map[rId] = new_rId

    return rId_map


def _fix_image_rids_in_elm(elm, rId_map: dict) -> None:
    """
    将 elm（及其子孙）中所有 <a:blip r:embed="..."> 的 rId 按 rId_map 替换。
    """
    if not rId_map:
        return
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    for blip in elm.iter(f'{{{a_ns}}}blip'):
        old = blip.get(f'{{{r_ns}}}embed')
        if old and old in rId_map:
            blip.set(f'{{{r_ns}}}embed', rId_map[old])


# ─────────────────────────────────────────────────────────────────────────────
# 内部辅助：为副本 XML 分配不冲突的 cNvPr id
# ─────────────────────────────────────────────────────────────────────────────

def _assign_new_shape_ids(prs, new_elm) -> int:
    """
    为 new_elm 中所有 cNvPr 元素分配全局唯一的 id（整个演示文稿范围内不冲突）。
    支持普通 shape（p:cNvPr）、图片（p:cNvPr in pic）以及 group（p:cNvGrpSpPr 下的 p:cNvPr）。

    Returns:
        顶层 cNvPr 被分配的新 id
    """
    p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'
    pic_ns = 'http://schemas.openxmlformats.org/drawingml/2006/picture'

    def _collect_existing_ids():
        existing = set()
        for slide in prs.slides:
            for ns in (p_ns, pic_ns):
                for el in slide._element.iter(f'{{{ns}}}cNvPr'):
                    val = el.get('id', '')
                    if val.isdigit():
                        existing.add(int(val))
        return existing

    existing_ids = _collect_existing_ids()
    next_id = max(existing_ids, default=0) + 1

    top_id = None
    for ns in (p_ns, pic_ns):
        for cNvPr in new_elm.iter(f'{{{ns}}}cNvPr'):
            cNvPr.set('id', str(next_id))
            if top_id is None:
                top_id = next_id
            next_id += 1

    return top_id


# ─────────────────────────────────────────────────────────────────────────────
# 内部辅助：修改副本 XML 中顶层 xfrm 的位置与尺寸
# ─────────────────────────────────────────────────────────────────────────────

def _get_shape_absolute_pos_size(shape) -> dict:
    """
    获取 shape 在幻灯片级别的绝对位置和尺寸（EMU）。

    对于顶层 shape（非 group 子元素），直接返回其 left/top/width/height。
    对于 group 内部的子 shape，python-pptx 的 .left/.top/.width/.height
    返回的是相对于 group 坐标系（chOff/chExt）的**内部坐标**，需要通过
    group 的缩放因子转换为幻灯片级别的绝对坐标。

    OOXML 中 group 坐标系的关系：
      - group 外框: off(x, y), ext(cx, cy)     -- 幻灯片坐标系
      - group 内框: chOff(x, y), chExt(cx, cy)  -- 内部坐标系
      - 缩放因子: scaleX = ext.cx / chExt.cx,  scaleY = ext.cy / chExt.cy
      - 子 shape 的绝对位置 = off + (child_off - chOff) * scale

    Args:
        shape: python-pptx 的 shape 对象

    Returns:
        dict 包含 left, top, width, height（均为 EMU 整数）
    """
    from lxml import etree

    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'

    shape_elm = shape._element

    # 获取 shape 自身的 off/ext
    xfrm = None
    spPr = shape_elm.find(f'{{{p_ns}}}spPr')
    if spPr is not None:
        xfrm = spPr.find(f'{{{a_ns}}}xfrm')
    if xfrm is None:
        grpSpPr = shape_elm.find(f'{{{p_ns}}}grpSpPr')
        if grpSpPr is not None:
            xfrm = grpSpPr.find(f'{{{a_ns}}}xfrm')
    if xfrm is None:
        xfrm = shape_elm.find(f'.//{{{a_ns}}}xfrm')

    if xfrm is None:
        # 无坐标信息的 shape，回退到 python-pptx 属性
        return {
            'left': int(shape.left),
            'top': int(shape.top),
            'width': int(shape.width),
            'height': int(shape.height),
        }

    off_el = xfrm.find(f'{{{a_ns}}}off')
    ext_el = xfrm.find(f'{{{a_ns}}}ext')
    child_x = int(off_el.get('x', '0')) if off_el is not None else 0
    child_y = int(off_el.get('y', '0')) if off_el is not None else 0
    child_cx = int(ext_el.get('cx', '0')) if ext_el is not None else 0
    child_cy = int(ext_el.get('cy', '0')) if ext_el is not None else 0

    # 向上查找所有祖先 group，逐级累加坐标变换
    parent_elm = shape_elm.getparent()
    while parent_elm is not None:
        parent_tag = etree.QName(parent_elm).localname
        if parent_tag == 'grpSp':
            parent_grpSpPr = parent_elm.find(f'{{{p_ns}}}grpSpPr')
            if parent_grpSpPr is None:
                break
            parent_xfrm = parent_grpSpPr.find(f'{{{a_ns}}}xfrm')
            if parent_xfrm is None:
                break

            p_off = parent_xfrm.find(f'{{{a_ns}}}off')
            p_ext = parent_xfrm.find(f'{{{a_ns}}}ext')
            p_chOff = parent_xfrm.find(f'{{{a_ns}}}chOff')
            p_chExt = parent_xfrm.find(f'{{{a_ns}}}chExt')

            if p_off is None or p_ext is None or p_chOff is None or p_chExt is None:
                break

            grp_x = int(p_off.get('x', '0'))
            grp_y = int(p_off.get('y', '0'))
            grp_cx = int(p_ext.get('cx', '0'))
            grp_cy = int(p_ext.get('cy', '0'))
            ch_x = int(p_chOff.get('x', '0'))
            ch_y = int(p_chOff.get('y', '0'))
            ch_cx = int(p_chExt.get('cx', '0'))
            ch_cy = int(p_chExt.get('cy', '0'))

            if ch_cx <= 0 or ch_cy <= 0:
                break

            # 缩放因子
            scale_x = grp_cx / ch_cx
            scale_y = grp_cy / ch_cy

            # 内部坐标 → 绝对坐标
            child_x = grp_x + (child_x - ch_x) * scale_x
            child_y = grp_y + (child_y - ch_y) * scale_y
            child_cx = child_cx * scale_x
            child_cy = child_cy * scale_y

        # 继续向上查找（支持嵌套 group）
        parent_elm = parent_elm.getparent()

    return {
        'left': int(child_x),
        'top': int(child_y),
        'width': int(child_cx),
        'height': int(child_cy),
    }


def _set_xfrm(new_elm, left_emu: int, top_emu: int, width_emu: int, height_emu: int) -> bool:
    """
    在 new_elm 的**直接**子结构中定位 xfrm 节点并更新 off/ext。

    支持以下三种 shape 类型的 XML 结构：
      - 普通 shape / 文本框：<p:sp><p:spPr><a:xfrm>
      - 图片：              <p:pic><p:spPr><a:xfrm>
      - group shape：       <p:grpSp><p:grpSpPr><a:xfrm>（仅更新外框，不改内部子 shape）

    Returns:
        True 表示成功找到并修改，False 表示未找到 xfrm。
    """
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    p_ns = 'http://schemas.openxmlformats.org/presentationml/2006/main'

    xfrm = None

    # 1. 尝试 <p:spPr><a:xfrm>（普通 shape / 文本框 / 图片）
    for spPr_tag in (f'{{{p_ns}}}spPr',):
        spPr = new_elm.find(spPr_tag)
        if spPr is not None:
            xfrm = spPr.find(f'{{{a_ns}}}xfrm')
            if xfrm is not None:
                break

    # 2. 尝试 <p:grpSpPr><a:xfrm>（group shape）
    if xfrm is None:
        grpSpPr = new_elm.find(f'{{{p_ns}}}grpSpPr')
        if grpSpPr is not None:
            xfrm = grpSpPr.find(f'{{{a_ns}}}xfrm')

    # 3. 兜底：全局查找第一个 xfrm（connector / freeform 等特殊 shape）
    if xfrm is None:
        xfrm = new_elm.find(f'.//{{{a_ns}}}xfrm')

    if xfrm is None:
        return False

    tag_local = etree.QName(new_elm).localname
    if tag_local == 'grpSp':
        # group shape 的外框用 <a:off> / <a:ext>，内部子 shape 的基准用 <a:chOff> / <a:chExt>
        off   = xfrm.find(f'{{{a_ns}}}off')
        ext   = xfrm.find(f'{{{a_ns}}}ext')
        chOff = xfrm.find(f'{{{a_ns}}}chOff')
        chExt = xfrm.find(f'{{{a_ns}}}chExt')
        if off is not None:
            off.set('x', str(left_emu))
            off.set('y', str(top_emu))
        if ext is not None:
            ext.set('cx', str(width_emu))
            ext.set('cy', str(height_emu))
        # chOff / chExt 保持与外框一致（不缩放子 shape）
        if chOff is not None:
            chOff.set('x', str(left_emu))
            chOff.set('y', str(top_emu))
        if chExt is not None:
            chExt.set('cx', str(width_emu))
            chExt.set('cy', str(height_emu))
    else:
        off = xfrm.find(f'{{{a_ns}}}off')
        ext = xfrm.find(f'{{{a_ns}}}ext')
        if off is not None:
            off.set('x', str(left_emu))
            off.set('y', str(top_emu))
        if ext is not None:
            ext.set('cx', str(width_emu))
            ext.set('cy', str(height_emu))

    return True


# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：复制 shape（支持文本框、图片、group 及其他任意 shape 类型）
# ─────────────────────────────────────────────────────────────────────────────

def copy_shape(
	slide,
	src_shape_id: int,
	left_cm: float | None = None,
	top_cm: float | None = None,
	width_cm: float | None = None,
	height_cm: float | None = None,
	new_name: str | None = None,
	z_order: int | None = None,
) -> int:
	"""
	复制幻灯片中 src_shape_id 指向的**单个元素**，插入到幻灯片顶层 spTree。

	**核心行为**：只复制 src_shape_id 指向的那个单独元素（文本框、图片等），
	**不会连带复制其所在的 group**。即使目标元素嵌套在多层 group 内部，
	也只会提取该元素本身，副本被放置在幻灯片顶层。

	坐标与尺寸单位均为**厘米（cm）**。若省略位置/尺寸参数，则使用源 shape
	的绝对坐标（python-pptx 已将 group 偏移计算在内）。

	Args:
		slide:           python-pptx 的 slide 对象
		src_shape_id:    源 shape 的数字 id（cNvPr id 属性；支持递归查找 group 内部）
		left:            副本左边距（cm），None 时使用源 shape 的绝对坐标
		top:             副本上边距（cm），None 时使用源 shape 的绝对坐标
		width:           副本宽度（cm），None 时保留源 shape 的值
		height:          副本高度（cm），None 时保留源 shape 的值
		new_name:        副本的 shape 名称，None 时自动命名为 "{原名称} Copy"
		z_order:         图层顺序（0-based），None 时追加到最顶层

	Returns:
		int: 新 shape 的 id（cNvPr id 属性）

	Raises:
		ValueError: 未找到源 shape 或 shape 类型不支持复制
	"""


	if src_shape_id is None:
		raise ValueError("必须提供 src_shape_id")

	# ── 定位源 shape ─────────────────────────────────────────────────────────
	src_shape = get_shape(slide, src_shape_id)
	src_elm = src_shape._element

	shape_tag = etree.QName(src_elm).localname  # sp / pic / grpSp / ...

	# ── 防御：如果目标恰好是 grpSp，直接深拷贝整个 group ────────────────────
	# 这种情况通常不应该发生（AI 应该用子 shape 的 id），但保留兜底支持。
	if shape_tag == 'grpSp':
		new_elm = copy.deepcopy(src_elm)
	else:
		# ── 深拷贝目标元素自身的 XML（只拷这个元素，不拷 group）─────────────
		new_elm = copy.deepcopy(src_elm)

	# ── 确定最终位置与尺寸（EMU） ─────────────────────────────────────────────
	# 对于 group 内部的子 shape，python-pptx 的 .left/.top/.width/.height
	# 返回的是 group 坐标系下的内部坐标（未缩放），不能直接使用。
	# 通过 _get_shape_absolute_pos_size 转换为幻灯片级别的绝对坐标和尺寸。
	# 副本总是插入到顶层 spTree（无父 group 偏移），因此未指定时使用绝对坐标。
	abs_pos = _get_shape_absolute_pos_size(src_shape)
	final_left_emu   = int(left_cm   * EMU_PER_CM) if left_cm   is not None else abs_pos['left']
	final_top_emu    = int(top_cm    * EMU_PER_CM) if top_cm    is not None else abs_pos['top']
	final_width_emu  = int(width_cm  * EMU_PER_CM) if width_cm  is not None else abs_pos['width']
	final_height_emu = int(height_cm * EMU_PER_CM) if height_cm is not None else abs_pos['height']

	# ── 修改副本 XML 中的位置与尺寸 ──────────────────────────────────────────
	_set_xfrm(new_elm, final_left_emu, final_top_emu, final_width_emu, final_height_emu)

	# ── 设置副本名称 ─────────────────────────────────────────────────────────

	resolved_name = new_name if new_name else f"{src_shape.name} Copy"
	for path in _TOP_CNVPR_PATHS:
		top_cNvPr = new_elm.find(path)
		if top_cNvPr is not None:
			top_cNvPr.set('name', resolved_name)
			break

	# ── 为顶层 cNvPr 分配全局唯一 id ─────────────────────────────────────────
	prs = slide.part.package.presentation_part.presentation
	new_id = _assign_new_shape_ids(prs, new_elm)

	# ── 将副本插入幻灯片的 spTree ────────────────────────────────────────────
	spTree = slide._element.find(_qn('p:cSld')).find(_qn('p:spTree'))
	spTree.append(new_elm)

	# ── 刷新 python-pptx shape 缓存 ──────────────────────────────────────────
	if hasattr(slide.shapes, '_spTree'):
		slide.shapes._spTree = spTree

	# ── 调整 z_order ─────────────────────────────────────────────────────────
	shape_children = [
		child for child in list(spTree)
		if etree.QName(child).localname not in ('nvGrpSpPr', 'grpSpPr')
	]

	if z_order is not None:
		spTree.remove(new_elm)
		clamped = max(0, min(z_order, len(shape_children) - 1))
		if clamped == 0:
			shape_children[0].addprevious(new_elm)
		else:
			shape_children[clamped - 1].addnext(new_elm)

	return new_id


# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：删除 shape（支持文本框、图片、group 及其他任意 shape 类型）
# ─────────────────────────────────────────────────────────────────────────────

def delete_shape(
    prs,
    slide_index: int,
    shape_id: int,
) -> dict:
    """
    删除指定幻灯片中的任意 shape（文本框、图片、group 等）。

    通过 shape_id（cNvPr id 属性）定位，支持递归搜索 group 内部的子 shape。
    当删除的是 group 内部的子 shape 时，仅移除该子 shape，其余子 shape 保留。
    当删除顶层 group shape 时，整个 group（含全部子 shape）一并删除。

    Args:
        prs:         python-pptx 的 Presentation 对象
        slide_index: 目标幻灯片索引（0-based）
        shape_id:    目标 shape 的数字 id（cNvPr id 属性）

    Returns:
        执行结果 dict::

            {
                "success":      bool,
                "slide_index":  int,
                "shape_id":     int,   # 被删除的 shape id
                "shape_type":   str,   # shape 类型标签（如 "sp" / "pic" / "grpSp"）
                "error":        str | None,
            }

    Examples:
        >>> from pptx import Presentation
        >>> prs = Presentation("demo.pptx")

        # 1. 按 shape_id 删除文本框
        >>> delete_shape(prs, 0, shape_id=3)

        # 2. 删除 group 内部的子 shape
        >>> delete_shape(prs, 1, shape_id=12)

        # 3. 删除整个 group shape
        >>> delete_shape(prs, 1, shape_id=7)

        >>> prs.save("demo_deleted.pptx")
    """
    _err = lambda msg: {
        "success": False,
        "slide_index": slide_index,
        "shape_id": shape_id,
        "shape_type": None,
        "error": msg,
    }

    # ── 参数校验 ─────────────────────────────────────────────────────────────
    total = len(prs.slides)
    if slide_index < 0 or slide_index >= total:
        return _err(f"slide_index={slide_index} 超出范围 [0, {total - 1}]")

    # ── 定位目标 shape ───────────────────────────────────────────────────────
    slide = prs.slides[slide_index]
    try:
        target_shape = get_shape(slide, shape_id)
    except (ValueError, IndexError) as exc:
        return _err(f"未找到目标 shape：{exc}")

    shape_type = etree.QName(target_shape._element).localname
    target_elm = target_shape._element
    parent_elm = target_elm.getparent()

    if parent_elm is None:
        return _err(f"shape id={shape_id} 没有父节点，无法删除")

    parent_elm.remove(target_elm)

    return {
        "success": True,
        "slide_index": slide_index,
        "shape_id": shape_id,
        "shape_type": shape_type,
        "error": None,
    }

# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：移动和调整大小 shape（优化版本）
# ─────────────────────────────────────────────────────────────────────────────

def move_shape(slide, shape_id, left_cm=None, top_cm=None,
               width_cm=None, height_cm=None):

    try:
        shape = get_shape(slide, shape_id)
    except Exception as e:
        return {"success": False, "shape_id": shape_id, "error": str(e)}

    # 当前绝对位置
    abs_pos = _get_shape_absolute_pos_size(shape)

    target_abs_x = Cm(left_cm) if left_cm is not None else abs_pos["left"]
    target_abs_y = Cm(top_cm) if top_cm is not None else abs_pos["top"]
    target_abs_w = Cm(width_cm) if width_cm is not None else abs_pos["width"]
    target_abs_h = Cm(height_cm) if height_cm is not None else abs_pos["height"]

    # ===== 关键：绝对 → 局部（正确逆变换）=====
    local_x, local_y = _abs_to_local(shape, target_abs_x, target_abs_y)

    shape.left = int(local_x)
    shape.top = int(local_y)

    # ===== 尺寸处理 =====
    sx, sy = _get_cumulative_scale(shape)

    if width_cm is not None:
        shape.width = int(target_abs_w / sx)

    if height_cm is not None:
        shape.height = int(target_abs_h / sy)

    return {"success": True, "shape_id": shape_id}

def resize_shape(slide, shape_id, width_cm=None, height_cm=None):
    try:
        shape = get_shape(slide, shape_id)
    except Exception as e:
        return {"success": False, "shape_id": shape_id, "error": str(e)}

    sx, sy = _get_cumulative_scale(shape)

    if width_cm is not None:
        abs_w = Cm(width_cm)
        local_w = abs_w / sx
        shape.width = int(local_w)

    if height_cm is not None:
        abs_h = Cm(height_cm)
        local_h = abs_h / sy
        shape.height = int(local_h)

    return {"success": True, "shape_id": shape_id}




# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：获取系统可用字体列表
# ─────────────────────────────────────────────────────────────────────────────

def get_available_fonts(
    category: str | None = None,
    keyword: str | None = None,
) -> dict:
    """
    获取当前操作系统中可用的字体名称列表。

    委托到 font_scanner 模块，保持向后兼容。

    Args:
        category: 可选过滤分类，支持以下值：
            - ``"chinese"``  / ``"zh"``  ：仅返回含中文（CJK）字形的字体
            - ``"latin"``    / ``"en"``  ：仅返回拉丁/英文字体（名称中无 CJK 字符）
            - ``None``（默认）：返回全部字体
        keyword:  可选关键词过滤（大小写不敏感），仅返回名称中包含该关键词的字体，
                  如 ``"微软"``、``"Arial"``。

    Returns:
        结果 dict::

            {
                "success":     bool,
                "fonts":       list[str],   # 排序后的字体名列表
                "total":       int,          # 字体总数
                "category":    str | None,   # 传入的 category 参数
                "keyword":     str | None,   # 传入的 keyword 参数
                "error":       str | None,
            }

    Examples:
        >>> # 获取所有可用字体
        >>> get_available_fonts()

        >>> # 仅获取中文字体
        >>> get_available_fonts(category="chinese")

        >>> # 搜索名称含 "Hei" 的字体
        >>> get_available_fonts(keyword="Hei")
    """
    from font_scanner import scan_system_font_names

    try:
        all_fonts = scan_system_font_names(category=category)
    except Exception as e:
        return {
            "success": False,
            "fonts": [],
            "total": 0,
            "category": category,
            "keyword": keyword,
            "error": str(e),
        }

    # keyword filter
    if keyword:
        kw_lower = keyword.lower()
        all_fonts = [f for f in all_fonts if kw_lower in f.lower()]

    return {
        "success": True,
        "fonts": all_fonts,
        "total": len(all_fonts),
        "category": category,
        "keyword": keyword,
        "error": None,
    }


def _get_fonts_fallback() -> list[str]:
    """matplotlib 不可用时的字体枚举回退实现。委托到 font_scanner。"""
    from font_scanner import scan_system_font_names
    return scan_system_font_names()


def _system_font_dirs() -> list[str]:
    """返回当前操作系统的常见字体目录列表。委托到 font_scanner。"""
    from font_scanner import _system_font_dirs as _scanner_dirs
    return _scanner_dirs()

def export_image(
    slide,
    shape_id: int,
    output_path: str,
) -> dict:
    """
    导出 shape 中的图片为独立文件。

    支持两种图片存在形式：
    - 独立图片元素 (<p:pic>, shape_type=PICTURE)
    - 作为 fill 填充在 shape 中的图片 (fill_type=6)

    Args:
        slide:       python-pptx 的 slide 对象
        shape_id:    目标 shape 的数字 id（cNvPr id 属性；支持递归查找 group 内部）
        output_path: 导出文件的保存路径（扩展名决定格式，如 .png / .jpg）

    Returns:
        执行结果 dict::

            {
                "success":      bool,
                "shape_id":     int,
                "image_type":   str,   # "picture" 或 "fill"
                "content_type": str,   # MIME 类型
                "size_bytes":   int,   # 图片字节数
                "output_path":  str,   # 实际保存路径
                "error":        str | None,
            }

    Raises:
        ValueError: 未找到 shape 或 shape 中不包含图片
    """
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

    # 定位 shape
    src_shape = get_shape(slide, shape_id)
    elm = src_shape._element
    from pptx.enum.shapes import MSO_SHAPE_TYPE

    image_blob = None
    image_type = None
    content_type = None

    if src_shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
        # 独立图片 (<p:pic>)
        image_type = "picture"
        try:
            image_blob = src_shape.image.blob
            content_type = src_shape.image.content_type
        except Exception as e:
            return {
                "success": False,
                "shape_id": shape_id,
                "image_type": image_type,
                "content_type": None,
                "size_bytes": 0,
                "output_path": output_path,
                "error": f"读取独立图片失败: {e}",
            }

    else:
        # 检查是否有图片填充
        try:
            fill_type = src_shape.fill.type
            if fill_type == 6:  # PICTURE
                image_type = "fill"
                blips = elm.findall(f'.//{{{a_ns}}}blip')
                if not blips:
                    return {
                        "success": False,
                        "shape_id": shape_id,
                        "image_type": None,
                        "content_type": None,
                        "size_bytes": 0,
                        "output_path": output_path,
                        "error": "shape 的 fill 中未找到 blip 引用",
                    }
                rId = blips[0].get(f'{{{r_ns}}}embed')
                if not rId or rId not in src_shape.part.rels:
                    return {
                        "success": False,
                        "shape_id": shape_id,
                        "image_type": None,
                        "content_type": None,
                        "size_bytes": 0,
                        "output_path": output_path,
                        "error": f"图片引用 rId '{rId}' 无效",
                    }
                image_part = src_shape.part.rels[rId].target_part
                image_blob = image_part.blob
                content_type = image_part.content_type
            else:
                return {
                    "success": False,
                    "shape_id": shape_id,
                    "image_type": None,
                    "content_type": None,
                    "size_bytes": 0,
                    "output_path": output_path,
                    "error": f"shape 不含图片（fill_type={fill_type}）",
                }
        except Exception as e:
            return {
                "success": False,
                "shape_id": shape_id,
                "image_type": None,
                "content_type": None,
                "size_bytes": 0,
                "output_path": output_path,
                "error": f"读取 fill 图片失败: {e}",
            }

    # 写入文件
    try:
        os.makedirs(os.path.dirname(output_path) if os.path.dirname(output_path) else '.', exist_ok=True)
        with open(output_path, 'wb') as f:
            f.write(image_blob)
    except Exception as e:
        return {
            "success": False,
            "shape_id": shape_id,
            "image_type": image_type,
            "content_type": content_type,
            "size_bytes": len(image_blob),
            "output_path": output_path,
            "error": f"写入文件失败: {e}",
        }

    return {
        "success": True,
        "shape_id": shape_id,
        "image_type": image_type,
        "content_type": content_type,
        "size_bytes": len(image_blob),
        "output_path": output_path,
        "error": None,
    }


# ─────────────────────────────────────────────────────────────────────────────
# 公开接口：替换 shape 中的图片
# ─────────────────────────────────────────────────────────────────────────────

def replace_shape_image(
    slide,
    shape_id: int,
    image_path: str,
) -> dict:
    """
    替换 shape 中的图片。

    支持两种图片存在形式：
    - 独立图片元素 (<p:pic>, shape_type=PICTURE)：替换 blipFill 中的图片
    - 作为 fill 填充在 shape 中的图片 (fill_type=6)：替换 spPr/blipFill 中的图片

    替换操作会：创建新的 image part，将 XML 中 <a:blip> 的 r:embed 指向新 part，
    原始图片 part 保留在文件中（未被任何 shape 引用，但不影响使用）。

    Args:
        slide:      python-pptx 的 slide 对象
        shape_id:   目标 shape 的数字 id（cNvPr id 属性；支持递归查找 group 内部）
        image_path: 新图片的文件路径

    Returns:
        执行结果 dict::

            {
                "success":     bool,
                "shape_id":    int,
                "image_type":  str,   # "picture" 或 "fill"
                "new_rId":     str,   # 新图片的 relationship ID
                "error":       str | None,
            }

    Raises:
        ValueError: 未找到 shape、shape 中不包含图片、或新图片文件不存在
    """
    r_ns = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
    a_ns = 'http://schemas.openxmlformats.org/drawingml/2006/main'

    if not os.path.isfile(image_path):
        raise ValueError(f"图片文件不存在: {image_path}")

    # 定位 shape
    src_shape = get_shape(slide, shape_id)
    elm = src_shape._element
    from pptx.enum.shapes import MSO_SHAPE_TYPE

    image_type = None

    # 确认图片类型
    if src_shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
        image_type = "picture"
    else:
        try:
            if src_shape.fill.type == 6:
                image_type = "fill"
        except Exception:
            pass

    if image_type is None:
        raise ValueError(f"shape_id={shape_id} 不包含图片，无法替换")

    # 添加新图片到 slide part
    with open(image_path, 'rb') as f:
        img_blob = f.read()
    image_part, new_rId = slide.part.get_or_add_image_part(image_path)

    # 替换 XML 中所有 <a:blip r:embed="..."> 为新 rId
    blips = elm.findall(f'.//{{{a_ns}}}blip')
    if not blips:
        raise ValueError(f"shape_id={shape_id} 的 XML 中未找到 <a:blip> 元素")

    for blip in blips:
        blip.set(f'{{{r_ns}}}embed', new_rId)

    return {
        "success": True,
        "shape_id": shape_id,
        "image_type": image_type,
        "new_rId": new_rId,
        "error": None,
    }
