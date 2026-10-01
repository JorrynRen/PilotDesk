# 读取 PPT 指南

## Overview

本指南用于**读取/解析/提取**现有 `.pptx` 文件中的结构化内容（文字、布局、图片、表格、图表等），不涉及任何编辑操作。

## 适用场景

- 用户提供现有 PPT 文件，需要提取其中文字内容
- 需要分析 PPT 的页面结构、元素布局、样式信息
- 需要统计 PPT 中的图片、表格、图表等元素
- 需要回答关于 PPT 内容的提问

---

## 查询幻灯片概览

```bash
# 查询全部幻灯片的摘要 + 文本 + 布局
python "<技能根目录>\scripts\pptx_cli.py" query /path/to/file.pptx

# 查询第 2 页，包含图片和表格信息
python "<技能根目录>\scripts\pptx_cli.py" query /path/to/file.pptx --slide 2 \
  --include summary,texts,layouts,images,tables

# 只查布局
python "<技能根目录>\scripts\pptx_cli.py" query /path/to/file.pptx --slide 1 \
  --include layouts
```

### include 可选值

| key | 返回内容 |
|-----|---------|
| `summary` | `{total: int, by_type: {SHAPE_TYPE: count, ...}}` |
| `texts` | `[{shape_id, text_preview}]`，preview 截断 30 字 |
| `images` | `[{shape_id, ext, fill_shape}]`，`fill_shape=false` 为独立图片，`true` 为 fill 填充图片 |
| `tables` | `[{shape_id, rows, cols}]` |
| `charts` | `[{shape_id, chart_type}]` |
| `layouts` | `[{shape_id, x, y, w, h, z_order, text_render?}]`，绝对坐标（cm），Group 内子元素已转换 |

---

## 查询指定元素详情

```bash
# 查看 shape_id=5 的文字内容和格式
python "<技能根目录>\scripts\pptx_cli.py" query-shape /path/to/file.pptx \
  --slide 1 --shape-id 5 --include text

# 查看布局和样式
python "<技能根目录>\scripts\pptx_cli.py" query-shape /path/to/file.pptx \
  --slide 1 --shape-id 3 --include layout,style

# 查看填充颜色
python "<技能根目录>\scripts\pptx_cli.py" query-shape /path/to/file.pptx \
  --slide 1 --shape-id 5 --include style
```

### include 可选值

| key | 返回内容 |
|-----|---------|
| `text` | run 级别的文本结构（段落列表 + 每段内的 runs，含文字与 format 字段） |
| `style` | 形状视觉样式（fill 类型/颜色, line 颜色/宽度），详见下方 `style` 返回结构 |
| `layout` | 绝对坐标（cm），Group 内子元素自动转换 |

### 返回结构示例

**`text` 返回结构：**

```json
{
  "text": {
    "paragraphs": [
      {
        "index": 0,
        "runs": [
          {
            "index": 0,
            "text": "加粗标题",
            "format": {"font_name": "微软雅黑", "size_pt": 28.0, "bold": true, "color_rgb": "#1A1A1A"}
          },
          {
            "index": 1,
            "text": "正文内容"
          }
        ]
      }
    ]
  }
}
```

> `format` 只在 run 显式设置了该属性时才出现。不带 `format` 的 run 表示没有显式格式，渲染时回退到主题默认样式。

**`layout` 返回结构：**

```json
{
  "layout": {
    "shape_id": 5,
    "x": 2.5,
    "y": 3.0,
    "w": 12.0,
    "h": 5.0,
    "z_order": 1,
    "text_render": {
      "text_width_cm": 10.5,
      "text_height_cm": 3.2,
      "is_overflow": false
    }
  }
}
```

> `text_render` 仅在 shape 包含文本时出现。`is_overflow` 为 `true` 表示文本渲染高度超过文本框高度，内容可能被截断。

**`style` 返回结构：**

```json
{
  "style": {
    "fill": {
      "type": "SOLID (1)",
      "color": "#FFFFFF"
    },
    "line": {
      "width_pt": 1.5,
      "color": "#8A9A94",
      "dash_style": "SOLID"
    }
  }
}
```

> 各 fill 类型的返回字段：

| fill type | 额外字段 | 说明 |
|-----------|---------|------|
| `SOLID (1)` | `color`：填充颜色（`#RRGGBB` 格式）；`color_source`：颜色来源（`"sys"` 表示系统颜色，仅 sysClr 时出现） | |
| `GRADIENT (3)` | `gradient: true` | 渐变填充（暂不返回具体梯度信息） |
| `PICTURE (6)` | `fill_image`：图片在包内的路径（如 `/ppt/media/image1.jpg`） | 图片填充 |
| `PATTERN (4)` | `pattern: true` | 图案填充 |
| `NONE (0)` | 无 | 无填充 |

### 返回值中的 `None`（重要）

`include` 中指定的 key 对应的值**可能是 `None`**，而非始终为 dict：

| key | 值为 `None` 的场景 |
|-----|-------------------|
| `style` | shape 类型为 PICTURE / LINKED_PICTURE（填充即图片本身）、GROUP（样式定义在子元素上）、LINE（无 fill）、TABLE / CHART / MEDIA 等嵌入对象；或 AUTO_SHAPE / FREEFORM / PLACEHOLDER / TEXT_BOX 四种适用类型但无有效 fill 和 line 信息时 |
| `text` | shape 无文本框（`has_text_frame=False`）或文本内容为空（空白 / 纯空格） |

**Group 处理：** 当 `shape_id` 指向 Group 时，返回该 Group 下所有子元素的信息，每个 key 对应一个列表。

---

## 图片

PPT 中图片有两种存在形式：

| 形式 | XML 结构 | shape_type | 识别方式 |
|------|----------|------------|---------|
| **独立图片** | `<p:pic>` | `PICTURE` | `shape.shape_type == PICTURE` |
| **Fill 填充图片** | `<p:sp>` 内的 `<a:blipFill>` | `AUTO_SHAPE` / `PLACEHOLDER` / `TEXT_BOX` / `FREEFORM` | `shape.fill.type == 6` |

### 查询所有图片

```bash
python "<技能根目录>\scripts\pptx_cli.py" query /path/to/file.pptx --slide 1 --include images
```

### 查看图片填充详情

```bash
python "<技能根目录>\scripts\pptx_cli.py" query-shape /path/to/file.pptx \
  --slide 1 --shape-id 3 --include style
```

---

## 常见陷阱

- **python-pptx 的 Group 坐标陷阱**：`shape.left/top` 对于 Group 内子元素返回的是相对于 Group 的局部坐标，而非页面绝对坐标。本工具链的所有分析函数已内置自动转换，直接使用 cm 值即可。
- **段落索引与视觉行不一致**：python-pptx 的 paragraph 按硬回车分段，一个文本框内可能只有 1 个 paragraph 但视觉上多行显示。
- **`RGBColor.from_string` 不接受带 `#` 的字符串**：传参时去掉 `#` 前缀，正确写法为 `RGBColor.from_string("2C4A6E")` 而非 `RGBColor.from_string("#2C4A6E")`。
