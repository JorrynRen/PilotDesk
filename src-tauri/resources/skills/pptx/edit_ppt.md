# PPT 编辑指南

## 核心规则（必须遵守）

### 文件隔离

在工作区为当前 PPT 任务创建独立工作目录，所有产物写入该目录。编辑现有 PPT 前，先将源文件复制为工作副本，**所有操作只在副本上进行，源文件全程只读**。

### 绝对禁止

以下操作是最高频的错误来源，**任何情况下都不允许**：

- 禁止使用 `add_textbox` / `add_shape` / `add_picture` 等 python-pptx 原生 API 新增元素 — **必须用 `copy-shape`**
- 禁止直接操作 python-pptx 底层 API 修改文字（`font.color.rgb = ...`、`run.font.size = ...`、`tf.paragraphs` 等） — **必须用 `edit-text`**
- 禁止凭空指定坐标 — **必须先 `query` 获取现有布局再推算**
- `copy-shape` 已完整复制源元素的样式，添加元素时无需通过 XML 查看源 shape 的样式信息

### 工具选择

| 场景 | 命令 |
|------|-----------|
| 查询页面信息 | `query` |
| 查询元素信息 | `query-shape` |
| 修改文字内容/格式 | `edit-text` |
| 新增元素 | `copy-shape` |
| 移动/调整尺寸 | `move-shape` / `resize-shape` |
| 删除元素 | `delete-shape` |
| 页面增删/复制/排序 | `slide-organize` |
| 图片导出/替换 | `export-image` / `replace-image` |
| 查询可用字体 | `get-fonts` |

### 默认行为

- **保留原始样式**：编辑时保持模板的字体、配色、布局风格，不做无根据的风格变更。
- **单位**：所有布局参数均使用**厘米（cm）**。
- **shape 定位**：所有操作通过 `shape_id`（即 OOXML `cNvPr` 的 `id` 属性）定位元素，支持递归查找 Group 内部。
- **图表/图标**：添加图表参考 `reference/chart.md`，添加图标参考 `reference/get_icons.md`。
- **自动保存**：每个编辑命令执行后自动保存到原文件，无需手动保存。
- **新增页面位置**：用户说"在第 i 页新增一页"时，新页面插入到第 i 页之前（即新页面成为新的第 i 页，原第 i 页及之后顺延）。

---

## 查询命令

### query — 查询页面信息

按需查询页面信息，白名单模式。每次调用重新遍历，无状态。

```bash
# 查看第 1 页概览
python "<技能根目录>\scripts\pptx_cli.py" query demo.pptx --slide 1

# 只查布局
python "<技能根目录>\scripts\pptx_cli.py" query demo.pptx --slide 1 --include layouts

# 包含图片和表格信息
python "<技能根目录>\scripts\pptx_cli.py" query demo.pptx --slide 1 \
  --include summary,texts,layouts,images,tables
```

**include 可选值：**

| key | 返回内容 |
|-----|---------|
| `summary` | `{total: int, by_type: {SHAPE_TYPE: count, ...}}` |
| `texts` | `[{shape_id, text_preview}]`，preview 截断 30 字 |
| `images` | `[{shape_id, ext, fill_shape}]`，`fill_shape=false` 为独立图片，`true` 为 fill 填充图片 |
| `tables` | `[{shape_id, rows, cols}]` |
| `charts` | `[{shape_id, chart_type}]` |
| `layouts` | `[{shape_id, x, y, w, h, z_order, text_render?}]`，绝对坐标（cm），Group 内子元素已转换；文本类 shape 自动附加 `text_render`（含 `text_width_cm`、`text_height_cm`、`is_overflow`） |

### query-shape — 查询元素详情

**查询元素信息的首选命令。** 白名单模式，按需获取。`style` 返回中已包含完整的填充颜色信息，无需通过 XML 手动解析。

```bash
# 查看文字内容和格式
python "<技能根目录>\scripts\pptx_cli.py" query-shape demo.pptx \
  --slide 1 --shape-id 5 --include text

# 查看布局和样式
python "<技能根目录>\scripts\pptx_cli.py" query-shape demo.pptx \
  --slide 1 --shape-id 3 --include layout,style

# 查看填充颜色
python "<技能根目录>\scripts\pptx_cli.py" query-shape demo.pptx \
  --slide 1 --shape-id 5 --include style
```

**include 可选值：**

| key | 返回内容 |
|-----|---------|
| `text` | run 级别的文本结构（段落列表 + 每段内的 runs，含文字与 format 字段） |
| `style` | 形状视觉样式（fill 类型/颜色, line 颜色/宽度），详见下方 `style` 返回结构 |
| `layout` | 绝对坐标（cm），Group 内子元素自动转换 |

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

**返回值中的 `None`（重要）：**

`include` 中指定的 key 对应的值**可能是 `None`**，而非始终为 dict：

| key | 值为 `None` 的场景 |
|-----|-------------------|
| `style` | shape 类型为 PICTURE / LINKED_PICTURE（填充即图片本身）、GROUP（样式定义在子元素上）、LINE（无 fill）、TABLE / CHART / MEDIA 等嵌入对象；或 AUTO_SHAPE / FREEFORM / PLACEHOLDER / TEXT_BOX 四种适用类型但无有效 fill 和 line 信息时 |
| `text` | shape 无文本框（`has_text_frame=False`）或文本内容为空（空白 / 纯空格） |

使用时**必须做空值防御**，避免对 `None` 做 dict 方法调用（如 `.get("fill")`）导致 `AttributeError`。

**异常：**

| 异常类型 | 触发条件 |
|---------|---------|
| `LookupError` | `shape_id` 在当前 slide 中不存在 |
| `ValueError` | `include` 中包含不支持的 key |

**Group 处理：** 当 `shape_id` 指向 Group 时，返回该 Group 下所有子元素的信息，每个 key 对应一个列表。

---

## 编辑命令

### edit-text — 修改文字内容和格式

修改文字内容和格式。支持在一次调用中执行多条操作。

```bash
# 修改文字和格式
python "<技能根目录>\scripts\pptx_cli.py" edit-text output.pptx \
  --slide 1 --shape-id 5 \
  --operations '[
    {"action":"set_run","para_index":0,"run_index":0,"text":"新标题","format":{"bold":true,"color_rgb":"FF0000","size_pt":36}},
    {"action":"set_para_format","para_index":1,"format":{"alignment":"center","line_spacing_pt":24}},
    {"action":"add_run","para_index":0,"text":"追加的文字","format":{"size_pt":14}},
    {"action":"add_para","text":"新段落内容"},
    {"action":"delete_run","para_index":1,"run_index":2},
    {"action":"delete_para","para_index":2}
  ]'
```

**operations — run 级操作：**

| action | 参数 | 说明 |
|--------|------|------|
| `set_run` | `para_index`, `run_index`, `text?`, `format?` | 同时修改文字和格式，text/format 均可选 |
| `set_run_text` | `para_index`, `run_index`, `text` | 只修改文字 |
| `set_run_format` | `para_index`, `run_index`, `format` | 只修改格式 |
| `add_run` | `para_index`, `text`, `format?` | 在段落末尾追加新 run；省略 format 自动继承同段末尾 run 样式 |
| `delete_run` | `para_index`, `run_index` | 删除指定 run；若段落仅剩一个 run，删除后变为空段落 |

**operations — 段落级操作：**

| action | 参数 | 说明 |
|--------|------|------|
| `set_para_text` | `para_index`, `text` | 替换整段文字（保留首 run 格式，删除多余 run） |
| `set_para_format` | `para_index`, `format` | 修改段落格式 |
| `add_para` | `text`, `format?` | 在文本框末尾追加新段落；省略 format 自动继承文本框末尾 run 样式 |
| `delete_para` | `para_index` | 删除指定段落 |

**format 字段（run 级）：**

`bold`, `italic`, `underline`, `strike`, `size_pt`, `font_name`, `latin`（西文字体名）, `east_asia`（中文字体名）, `color_rgb`

**format 字段（段落级）：**

`alignment`（left/center/right/justify）, `space_before_pt`, `space_after_pt`, `line_spacing_pt`, `level`（0-8）

### slide-organize — 页面增删/复制/排序

批量执行页面级操作。**所有索引均基于调用时的初始状态**，函数内部自动追踪位移。

```bash
python "<技能根目录>\scripts\pptx_cli.py" slide-organize output.pptx \
  --operations '[
    {"action":"delete","index":2},
    {"action":"copy","index":0,"position":1}
  ]'
```

**operations 格式：**

| action | 必需参数 | 可选参数 | 说明 |
|--------|---------|---------|------|
| `delete` | `index` | | 删除页面（0-based） |
| `copy` | `index` | `position` | 复制页面，position 默认追加到末尾 |
| `move` | `from_index`, `to_index` | | 重排序页面 |

> 所有索引均基于调用时的初始状态，函数内部自动追踪位移。

### move-shape — 移动元素

移动元素或同时调整尺寸。参数为绝对坐标（cm），Group 内元素自动处理逆变换。未指定的参数保持原值。

```bash
# 仅移动位置
python "<技能根目录>\scripts\pptx_cli.py" move-shape output.pptx \
  --slide 1 --shape-id 3 --left-cm 5.0 --top-cm 10.0

# 同时移动和调整尺寸
python "<技能根目录>\scripts\pptx_cli.py" move-shape output.pptx \
  --slide 1 --shape-id 3 --left-cm 5.0 --top-cm 10.0 --width-cm 8.0 --height-cm 4.0
```

### resize-shape — 调整尺寸

仅调整尺寸，不改变位置。

```bash
python "<技能根目录>\scripts\pptx_cli.py" resize-shape output.pptx \
  --slide 1 --shape-id 3 --width-cm 10.0 --height-cm 5.0
```

### copy-shape — 复制元素

复制元素到幻灯片顶层。只复制 `src-shape-id` 指向的**单个元素**，不连带其所在 Group。未指定的位置/尺寸参数使用源元素的绝对坐标。返回新 shape 的 shape_id。

```bash
# 指定位置复制
python "<技能根目录>\scripts\pptx_cli.py" copy-shape output.pptx \
  --slide 1 --src-shape-id 5 --left-cm 10.0 --top-cm 8.0

# 原位置复制
python "<技能根目录>\scripts\pptx_cli.py" copy-shape output.pptx \
  --slide 1 --src-shape-id 3
```

### delete-shape — 删除元素

支持删除 Group 内子 shape（仅移除该子 shape，不影响其余）。

```bash
python "<技能根目录>\scripts\pptx_cli.py" delete-shape output.pptx \
  --slide 1 --shape-id 5
```

### export-image — 导出图片

导出 shape 中的图片为独立文件。支持独立图片（`<p:pic>`）和 fill 填充图片（`fill_type=6`）。

```bash
python "<技能根目录>\scripts\pptx_cli.py" export-image output.pptx \
  --slide 1 --shape-id 2 -o logo.png
```

**返回值：**

```json
{
  "success": true,
  "shape_id": 3,
  "image_type": "fill",
  "content_type": "image/png",
  "size_bytes": 45230,
  "output_path": "exported.png",
  "error": null
}
```

### replace-image — 替换图片

替换 shape 中的图片。支持独立图片（`<p:pic>`）和 fill 填充图片（`fill_type=6`）。

```bash
python "<技能根目录>\scripts\pptx_cli.py" replace-image output.pptx \
  --slide 1 --shape-id 2 --image new_logo.png
```

**返回值：**

```json
{
  "success": true,
  "shape_id": 3,
  "image_type": "fill",
  "new_rId": "rId5",
  "error": null
}
```

### get-fonts — 查询可用字体

```bash
# 查询中文字体
python "<技能根目录>\scripts\pptx_cli.py" get-fonts --category chinese

# 按关键字搜索
python "<技能根目录>\scripts\pptx_cli.py" get-fonts --keyword Hei
```

---

## 布局规则

### 布局参考值（非强制，根据实际内容灵活调整）

- 页面边距一般留 1~2 cm
- 相邻元素间距一般 0.5~1 cm
- 标题区通常位于页面上部，正文内容区在其下方

### 编辑心法：全局先行

编辑布局时，**先完整理解目标页面的空间结构和元素语义关系，再动手修改**。禁止只盯着用户要求操作的区域而忽略页面其他部分。

每次布局编辑前，必须在思维中回答三个问题：

1. **这个页面整体的空间划分是什么样的？** — 哪些区域被占用，哪些空闲，页面的视觉节奏是什么？
2. **我要操作的区域和周围元素的关系是什么？** — 哪些元素属于同一逻辑组？哪些是装饰？哪些与内容有对应关系（如列表符号与段落的一一对应）？
3. **我的修改会对页面其他部分产生什么影响？** — 空间够不够？会不会挤压已有内容？布局形态是否合理？

只有三个问题都有了清晰答案后，才进入具体坐标计算。这三个问题不需要向用户输出，但**必须在代码执行前完成思考**。

### 核心原则：基于 CRAP 设计原理

添加或移动元素时，**必须基于 `query` 获取的实际布局来推算坐标**，禁止凭空指定。布局推算遵循以下原则：

1. **先算后动**：在修改任何元素之前，先基于 `query` 结果完成空间计算（可用高度、可用宽度、已有元素占用情况），确认方案可行再执行，避免先动手再反复检测修正。文本框的布局信息中已自动包含 `text_render` 字段（文本实际渲染宽高及是否溢出），直接据此推算所需空间即可，禁止凭直觉分配高度导致文字截断或溢出。空间计算时，必须使用 `query` 返回的 `text_render`（文本实际渲染高度）作为内容占用空间的唯一依据。如果修改会改变已有元素的尺寸，应先验证 `text_render` 确认内容不会溢出。
2. **尺寸由空间和内容决定**：新元素的尺寸应根据可用空间和内容量独立计算，不要从被复制的元素继承尺寸。`copy-shape` 只复用视觉样式（填充、边框等），尺寸参数应在复制后重新设定。
3. **接近性（Proximity）**：相关的元素在空间上靠近，不相关的保持距离。添加新元素时，先判断它与哪些已有元素属于同一逻辑组，放入该组附近。不要将属于不同组的元素强行合并排列。
4. **对齐（Alignment）**：元素边缘或中心应对齐到页面上的视觉参考线。对齐是美化手段，不是默认操作——只在必要时使用，不要为追求对齐而大幅改变用户未要求修改的元素位置。
5. **重复（Repetition）**：同类型元素保持一致的样式（尺寸、间距、配色）。新增元素应复用已有同类元素的样式，保持页面视觉一致性。
6. **对比（Contrast）**：不同层级的信息通过大小、颜色、粗细区分（如标题 > 副标题 > 正文），增强信息层级清晰度。

### 布局意图规则（必须遵守）

用户对布局位置的指令（如"下方""右侧""上方"等）是**刚性约束**，必须严格遵从，不得擅自改变整体布局结构：

- 用户说"在...下方添加" -> 新元素放在指定元素的下方，不是改成多列并排
- 用户说"在...右侧添加" -> 新元素放在指定元素的右侧，不是改成上下排列
- 如果用户指定的空间不足以容纳新元素，应先通过 `query` 计算可用空间，然后向用户说明情况并提出至少两种替代方案供选择，**禁止在未告知用户的情况下缩小或移动已有元素以腾出空间**

### 页面尺寸参考

标准 16:9 宽屏页面约 33.87 x 19.05 cm，计算时以此为准。

---

## 图片

PPT 中图片有两种存在形式：

| 形式 | XML 结构 | shape_type | 识别方式 |
|------|----------|------------|---------|
| **独立图片** | `<p:pic>` | `PICTURE` | `shape.shape_type == PICTURE` |
| **Fill 填充图片** | `<p:sp>` 内的 `<a:blipFill>` | `AUTO_SHAPE` / `PLACEHOLDER` / `TEXT_BOX` / `FREEFORM` | `shape.fill.type == 6` |

### 查询

```bash
# 查询所有图片
python "<技能根目录>\scripts\pptx_cli.py" query demo.pptx --slide 1 --include images

# 查看 fill 图片详情（style 中包含 fill_image 字段）
python "<技能根目录>\scripts\pptx_cli.py" query-shape demo.pptx \
  --slide 1 --shape-id 3 --include style
```

### 编辑

| 操作 | 命令 | 说明 |
|------|------|------|
| 导出图片 | `export-image` | 将图片保存为独立文件 |
| 替换图片 | `replace-image` | 用新图片替换原图 |
| 复制图片 | `copy-shape` | 两种图片形式均支持（同页复制，rId 天然兼容） |
| 删除图片 | `delete-shape` | 删除整个 shape（含图片） |

> **注意**：fill 图片的显示效果受填充模式（stretch、tile、clip）影响。如需修改填充模式，需直接操作 XML。

---

## 常见陷阱

- **python-pptx 的 Group 坐标陷阱**：`shape.left/top` 对于 Group 内子元素返回的是相对于 Group 的局部坐标，而非页面绝对坐标。本工具链的所有分析/编辑函数已内置自动转换，直接使用 cm 值即可。
- **color_rgb 格式**：`edit-text` 的 `color_rgb` 接受 `"FF0000"` 或 `"#FF0000"` 格式，不带 `#` 前缀也可以。
- **段落索引与视觉行不一致**：python-pptx 的 paragraph 按硬回车分段，一个文本框内可能只有 1 个 paragraph 但视觉上多行显示。
- **`RGBColor.from_string` 不接受带 `#` 的字符串**：传参时去掉 `#` 前缀，正确写法为 `RGBColor.from_string("2C4A6E")` 而非 `RGBColor.from_string("#2C4A6E")`。
