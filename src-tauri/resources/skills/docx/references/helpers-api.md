# docx-helpers API 参考

```javascript
const path = require('path');
const h = require(path.join('<技能根目录>', 'scripts', 'docx-helper'))({
  fonts: { heading: 'Microsoft YaHei', body: 'Microsoft YaHei' },
  colors: { primary: '2B579A', text: '333333', light: 'F2F6FC' },
});
```

**配置项**（全部可选，有默认值）：

| 配置 | 默认值 | 说明 |
|------|--------|------|
| `fonts.heading` / `body` | `Microsoft YaHei` | 标题/正文字体 |
| `fonts.english` | — | 英文字体。设置后 body 和 heading 自动变为复合字体：ASCII/hAnsi 用 english 字体，eastAsia 用原中文字体 |
| `sizes.h1` / `h2` / `h3` / `body` | 36 / 30 / 26 / 22 | 字号（半磅，22 = 11pt） |
| `colors.primary` / `text` / `border` / `light` | `2B579A` / `333333` / `D0D0D0` / `F2F6FC` | 各色值 |
| `page.width` / `height` | 12240 / 15840 | 页面尺寸（DXA，US Letter） |
| `page.margins` | 各 1440 | 页边距（DXA） |
| `indent` | — | 正文首行缩进（DXA）。设置后 `h.p()` 自动对所有正文段落应用 `firstLine` 缩进（标题和列表除外），无需手动传 `indent` 或封装辅助函数 |
| `columns.count` / `space` | 1 / 708 | 分栏数和栏间距（DXA）。设置 `columns: { count: 2, space: 468 }` 启用双栏；表格和图片自动缩放到栏宽而非页面宽度 |

初始化后通过 `h.colors`、`h.fonts`、`h.sizes`、`h.contentWidth`、`h.fullContentWidth`、`h.cfg` 访问计算后的配置。`h.contentWidth` 为单栏可用宽度（双栏时自动减半），`h.fullContentWidth` 始终为页面全宽。

---

## API 速查

### 段落与文字

| API | 说明 |
|-----|------|
| `h.p(content, opts?)` | 段落。content 为字符串或 `[str, TextRun, ...]` 混排数组 |
| `h.h1/h2/h3(text, opts?)` | 标题，自动 `keepNext` + `keepLines` 防止与下文分页。`opts.bookmark` 添加书签 |
| `h.bullet(content, opts?)` | 无序列表。`{ level: 1 }` 为二级 |
| `h.numbered(content, opts?)` | 有序列表。传不同 `{ ref: 'xxx' }` 重置编号 |
| `h.text(content, props?)` | 自定义样式文字，用于混排 |
| `h.bold/italic(text, extra?)` | 快捷粗体/斜体 |
| `h.link(text, url)` | 超链接，放在 `h.p()` 混排数组中 |

### 布局与媒体

| API | 说明 |
|-----|------|
| `h.pageBreak()` | 分页符（仅同一 section 内部，section 间自动分页） |
| `h.spacer(height?)` | 空白间距（DXA，默认 400） |
| `h.divider(color?, size?)` | 水平分割线 |
| `h.fullWidth(...children)` | 跨栏容器：双栏文档中让表格/图片横跨整个页面宽度，文字自然环绕（不打断双栏文字流）。表格自动变为浮动定位并缩放到全页宽；图片自动包裹在无边框浮动表格中并按比例放大。相邻的 Paragraph 自动合并为表格内的标题行。单栏文档中为透传。**注意：浮动元素不能跨页，行数多的大表格（≥7行）不宜使用，应放栏内** |
| `h.img(path, opts?)` | 图片，返回 Paragraph。`{ width, height, align, floating, altText, spacing }`，宽高像素@96DPI。**直接放入 sections children，不要嵌套在 `h.p()` 内** |
| `h.coverBg(imagePath)` | 封面全屏背景图，自动计算尺寸和浮动定位 |

### 页眉页脚

| API | 说明 |
|-----|------|
| `h.headerFooter(headerText, footerContent?)` | 一站式页眉+页脚，展开到 section：`{ ...h.headerFooter('标题'), children }` |
| `h.header(content, opts?)` | 页眉（默认 18 号灰色居中） |
| `h.footer(content?)` | 页脚（不传参 → 自动 `— 1 —` 页码） |
| `h.pageNum()` | 页码占位符，用于混排：`h.text(h.pageNum())` |

默认行为：`h.build({ sections })` 会给没有 `footers` 的 section 自动补默认页码页脚，避免漏写 `h.headerFooter()` 时整篇没有页码。已有 `footers` 不会被覆盖。封面或特殊 section 不要页码时写 `{ noPageNumber: true, children: [...] }`；整篇关闭自动页码时写 `h.build({ pageNumbers: false, sections: [...] })`。

### 目录与书签

| API | 说明 |
|-----|------|
| `h.toc(opts?)` | 目录。默认 `hyperlink: true, headingStyleRange: '1-3'`。支持 `cachedEntries` 预填充 |
| `h.bookmark(id, content)` | 书签锚点。也可在 `h.h1/h2/h3` 中传 `{ bookmark: '_id' }` |

### 表格

| API | 说明 |
|-----|------|
| `h.table(spec)` | 表格。列宽溢出自动缩放到页面宽度。支持 `rowHeight` 行高；单元格对象支持 `align`、`verticalAlign`、`textDirection` |
| `h.threeLineTable(spec)` | 学术三线表（上下粗线、表头下细线、无竖线）。`{ widths, header, rows, caption?, captionPosition? }` |
| `h.MERGE.START / CONTINUE` | 纵向合并常量 |

### 公式（LaTeX → OMML）

| API | 说明 |
|-----|------|
| `h.math(latex)` | 行内公式，返回 `OfficeMath` 对象。放在 `h.p()` 混排数组中 |
| `h.formula(latex, opts?)` | 独立公式段落（居中）。支持 `opts.eqNumber` 公式编号（编号自动右对齐） |
| `$...$` 自动转换 | `h.p()` 等接收的字符串中 `$x^2$` 会自动转为行内公式，无需手动调用 `h.math()` |

### 参考文献（自动编号）

| API | 说明 |
|-----|------|
| `h.refTracker(opts?)` | 创建引用追踪器，按首次引用顺序自动分配 `[N]` 编号。创建后 `h.p()` 中可用 `[@key]` 内联引用语法 |
| `[@key1,key2]` | 内联引用语法，在字符串中直接标记引用位置，自动替换为上标编号。多 key 逗号分隔 |
| `refs.bibliography(entries, opts?)` | 生成参考文献列表段落数组，按首次出现顺序排列。`entries: [{key, text}]` |
| `refs.autoBibliography(jsonPath, opts?)` | 读取 JSON 文件自动生成参考文献。若漏调此方法，`h.build()` 会在输出目录/工作目录查找 `references.json` 自动追加 |

### 文档组装

| API | 说明 |
|-----|------|
| `h.build(spec, patches?)` | 生成并写文件 |
| `h.createDoc(spec)` | 仅创建 Document 对象 |
| `h.raw` | docx-js 原始类（极少数未封装场景） |

#### h.build() 推荐写法

```javascript
// 无补丁
h.build({ sections: [{ children: [...] }] });

// 带补丁
h.build({ sections: [...] }, [
  { type: 'watermark', text: '机密' },
]);
```
---

## h.p() opts 详细

**段落级属性：**

| 属性 | 类型 | 说明 |
|------|------|------|
| `align` | `'left'` / `'center'` / `'right'` / `'justify'` | 对齐 |
| `spacing` | `{ before, after, line }` | 段落间距（DXA） |
| `indent` | `{ left, right, hanging, firstLine }` | 缩进 |
| `pageBreakBefore` | `boolean` | 段前分页 |
| `keepNext` | `boolean` | 与下一段保持同页（标题默认开启） |
| `keepLines` | `boolean` | 段落不跨页拆分（标题默认开启） |
| `widowControl` | `boolean` | 防止孤行（文档默认开启） |
| `shading` | `{ fill, type }` | 段落底色 |
| `border` | `{ top, bottom, left, right }` | 段落边框 |
| `bookmark` | `string` | 仅 `h.h1/h2/h3` — 添加书签锚点 |

**文字级属性**（对字符串元素生效）：

| 属性 | 类型 | 说明 |
|------|------|------|
| `bold` / `italic` / `strike` | `boolean` | 粗体 / 斜体 / 删除线 |
| `underline` | `boolean` / `{ type }` | 下划线 |
| `color` | `string` | 颜色（6 位 hex） |
| `size` | `number` | 字号（半磅，24 = 12pt） |
| `font` | `string` | 字体 |
| `highlight` | `string` | 高亮（`"yellow"` 等） |

---

## h.table() spec 详细

列宽溢出时自动按比例缩放到页面宽度。支持 `columnSpan` 横向合并，校验每行列数。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `widths` | `number[]` | 是 | 列宽数组（DXA），溢出自动缩放 |
| `header` | `string[]` | 否 | 表头文字（自动加粗居中） |
| `rows` | `any[][]` | 是 | 数据行 |
| `headerColor` | `string` | 否 | 表头背景色（hex），文字自动白色 |
| `headerTextColor` | `string` | 否 | 覆盖表头文字色 |
| `altColor` | `string` | 否 | 斑马纹底色（奇数数据行） |
| `borders` | `boolean` / `object` | 否 | `true`（默认）= 标准边框；`false` = 无边框 |
| `noBorders` | `boolean` | 否 | 同 `borders: false` |
| `margins` | `{ top, bottom, left, right }` | 否 | 单元格内边距（默认 60/60/100/100） |
| `rowHeight` | `number` / `'page'` / `{ value, rule }` | 否 | 所有行的行高。桌签建议用 `'page'` 自动撑满正文页高；数字单位为 DXA/twips |
| `rowHeights` | `number[]` | 否 | 逐行行高；有表头时第 0 项对应表头 |
| `heightRule` | `'exact'` / `'atLeast'` / `'auto'` | 否 | 行高规则，默认 `'exact'` |

**单元格内容**：

| 写法 | 示例 |
|------|------|
| 字符串 | `'hello'` |
| TextRun | `h.bold('重要')` |
| Paragraph | `h.p('居中', { align: 'center', bold: true })` |
| 对象（单元格属性） | `{ text: '高亮', bold: true, fill: 'FFEB3B' }` |
| 对象 + 合并 | `{ text: '跨列', columnSpan: 3, bold: true, align: 'center' }` |
| 纵向合并 | `{ text: '跨行', verticalMerge: h.MERGE.START }` |
| 单元格垂直居中 | `{ text: '姓名', align: 'center', verticalAlign: 'center' }` |
| 单元格文字方向 | `{ text: '姓名', textDirection: 'btLr' }` 或 `{ text: '姓名', textDirection: 'tbRl' }` |

**单元格对象常用属性**：

| 属性 | 说明 |
|------|------|
| `align` | 单元格内段落水平对齐：`'left'` / `'center'` / `'right'` / `'justify'` |
| `verticalAlign` | 单元格垂直对齐：`'top'` / `'center'` / `'middle'` / `'bottom'`，也可传 `h.raw.VerticalAlign.*` |
| `textDirection` | 单元格文字方向：`'lrTb'` 正常横排、`'tbRl'` 竖排（桌签左半边常用）、`'btLr'` 竖排反向（桌签右半边常用）；也可传 `h.raw.TextDirection.*` |

桌签常用示例：

```javascript
h.table({
  widths: [5000, 5000],
  rowHeight: 'page',
  rows: [[
    { text: '洪富梅', textDirection: 'tbRl', align: 'center', verticalAlign: 'center', size: 130, bold: true },
    { text: '洪富梅', textDirection: 'btLr', align: 'center', verticalAlign: 'center', size: 130, bold: true },
  ]],
  borders: false,
});
```

---

## h.build() spec 详细

| 属性 | 类型 | 说明 |
|------|------|------|
| `sections` | `array` | section 数组，每个含 `children`（和可选 `properties`、`headers`、`footers`） |
| `styles` | `object` | 自定义样式（不传则自动生成，含 Heading1-3 定义） |
| `footnotes` | `object` | 脚注定义 |
| `numbering` | `array` | 额外 numbering config（内置 bullet/number 自动包含） |

未设置 `section.properties` 时自动使用配置中的页面尺寸和边距。`patches` 执行失败时 `h.build()` 会直接 reject，不会写出残缺文档。

> **⚠ 每个 section 自动分页。不要在 section 开头加 `h.pageBreak()`，否则会产生空白页。**
>
> ```javascript
> // ✅ 正确：3 个 section 自动分页，无空白页
> sections: [
>   { children: coverSection() },
>   { children: tocSection() },
>   { children: [...chapter1(), ...chapter2()] },
> ]
>
> // ❌ 错误：section 开头加了 pageBreak → 每个 section 前多一页空白
> sections: [
>   { children: coverSection() },
>   { children: [h.pageBreak(), ...tocSection()] },
>   { children: [h.pageBreak(), ...chapter1(), ...chapter2()] },
> ]
> ```

---

## 封面背景

### 纯色/渐变封面（推荐）

用 `coverColor` 补丁，无需图片文件，支持纯色和渐变：

```javascript
h.build({ sections: [...] }, [
  { type: 'coverColor', colors: ['1A1A2E', '3D1F0B'] },
]);
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `colors` | `string[]` | 必填。1 色 = 纯色，2+ 色 = 渐变（颜色位置均匀分布） |
| `direction` | `'vertical'` / `'horizontal'` | 可选，默认 `'vertical'`（从上到下） |

### 图片封面

用 `h.coverBg()` ，自动从 `h.cfg.page` 计算尺寸并设置浮动定位（置于文字下方）：

```javascript
sections: [
  { children: [h.coverBg(`${OUTPUT}/cover-bg.jpeg`), ...coverText()] },
  { children: [...chapters()] },
]
```

### 全文背景图

如需**每一页都有**背景图（非仅封面），改用 `backgroundImage` 补丁：

```javascript
h.build({...}, [
  { type: 'backgroundImage', imagePath: `${OUTPUT}/bg.jpeg` },
]);
```

---

## 完整示例

```javascript
const path = require('path');
const h = require(path.join('<技能根目录>', 'scripts', 'docx-helper'))({
  fonts: { heading: 'SimHei', body: 'SimSun' },
  colors: { primary: '1A5276', text: '2C3E50', light: 'EBF5FB' },
});
const C = h.colors;

function cover() {
  return [
    h.spacer(2400),
    h.p('年度报告', { size: 52, bold: true, color: C.primary, align: 'center' }),
    h.divider(C.primary),
  ];
}

function tocSection() {
  return [h.h1('目  录', { align: 'center' }), h.spacer(200), h.toc()];
}

function chapter1() {
  return [
    h.h1('第一章 概述', { bookmark: '_ch1' }),
    h.p('本年度完成以下工作：'),
    h.bullet('系统架构升级'),
    h.bullet('上线 3 个新模块'),
    h.table({
      widths: [3120, 3120, 3120],
      header: ['指标', '目标', '实际'],
      rows: [['营收', '1000万', '1200万'], ['用户', '50万', '62万']],
      headerColor: C.primary, altColor: C.light,
    }),
  ];
}

const hf = h.headerFooter('年度报告');

h.build({
  sections: [
    { children: cover() },
    { ...hf, children: tocSection() },
    { ...hf, children: [...chapter1()] },
  ],
});
```

---

## h.threeLineTable() 用法

```javascript
h.threeLineTable({
  widths: [3000, 3000, 3000],
  header: ['变量', '均值', 'p值'],
  rows: [
    ['温度', '23.5', '<0.01'],
    ['湿度', '65.2', '0.03'],
  ],
  caption: '表 1  实验数据汇总',
  captionPosition: 'top',  // 'top'（默认）| 'bottom'
  cellFont: '宋体',
  cellSize: 21,
})
```

返回值：有 caption 时返回 `[captionPara, table]` 数组（可直接展开到 children），无 caption 时返回 `Table` 对象。

---

## h.math() / h.formula() 用法

```javascript
// 行内公式 —— 放到 h.p() 混排数组中
h.p(['由欧拉公式 ', h.math('e^{i\\pi} + 1 = 0'), '，可知...'])

// 自动行内公式 —— 字符串中的 $...$ 自动渲染
h.p('其中 $\\alpha$ 为显著性水平，$p < 0.05$ 时拒绝原假设。')

// 独立公式（居中，无编号）
h.formula('E = mc^2')

// 独立公式 + 编号（公式居中，编号靠右）
h.formula('E = mc^2', { eqNumber: '1' })

// LaTeX 中带 \tag{} 会自动提取为编号
h.formula('F = ma \\tag{2.1}')
```

支持的 LaTeX 语法：分数 `\frac{}{}`、上下标 `^{}`/`_{}`、根号 `\sqrt{}`、求和 `\sum`、积分 `\int`、希腊字母 `\alpha`、矩阵 `\begin{pmatrix}...\end{pmatrix}`、绝对值 `|x|` 等。

---

## 学术论文示例

```javascript
const h = require(...)({
  fonts: { heading: '黑体', body: '宋体', english: 'Times New Roman' },
  sizes: { h1: 44, h2: 32, h3: 30, h4: 28, body: 24, small: 21, caption: 18, ref: 21 },
  page: { width: 11906, height: 16838, margins: { top: 1701, bottom: 1418, left: 1701, right: 1418 } },
  spacing: { heading: { before: 156, after: 156 }, body: { after: 0, line: 360, lineRule: 'auto' } },
  indent: 480,
});

h.build({
  sections: [{
    children: [
      h.h1('基于深度学习的图像分类研究'),
      h.h2('一、引言'),
      h.p('近年来，深度学习在计算机视觉领域取得了显著进展。本研究采用 $\\alpha=0.05$ 的显著性水平。'),

      h.h2('二、方法'),
      h.p('损失函数定义如下：'),
      h.formula('L = -\\sum_{i=1}^{n} y_i \\log(\\hat{y}_i)', { eqNumber: '1' }),

      h.h2('三、实验结果'),
      ...h.threeLineTable({
        widths: [2500, 2500, 2500, 2500],
        header: ['模型', '准确率', '召回率', 'F1值'],
        rows: [
          ['ResNet-50', '0.952', '0.941', '0.946'],
          ['VGG-16', '0.923', '0.918', '0.920'],
        ],
        caption: '表 1  不同模型性能对比',
      }),
    ],
  }],
});
```

---

## h.refTracker() 用法

```javascript
const refs = h.refTracker();

// 内联引用 —— 在字符串中用 [@key] 标记，自动替换为上标编号
h.p('深度学习取得显著进展[@wang2024]。多项研究[@li2023,zhang2024]证实了有效性。'),
// 渲染为: 深度学习取得显著进展[1]。多项研究[2,3]证实了有效性。

// 同一 key 复用编号；连续编号自动压缩为范围
h.p('大量研究[@wang2024,li2023,zhang2024,chen2024]支持该结论。'),
// 渲染为: 大量研究[1-4]支持该结论。

// 同一文献再次引用 —— 复用首次编号
h.p('正如王等人[@wang2024]所述...'),
// 渲染为: 正如王等人[1]所述...

// 文末生成参考文献列表（自动按首次出现顺序排列）
h.h2('参考文献'),
...refs.bibliography([
  { key: 'wang2024', text: '王某某. 深度学习方法研究[J]. 计算机学报, 2024, 47(3): 1-15.' },
  { key: 'li2023', text: 'Li X. Neural Networks[J]. Nature, 2023.' },
  { key: 'zhang2024', text: '张某某. CNN优化[J]. 自动化学报, 2024.' },
])
// 输出: [1] 王某某...  [2] Li X...  [3] 张某某...
```

`refTracker(opts?)` 可选参数：`size`（上标字号）、`font`（编号字体）、`bodyFont`（文献正文字体）、`bodySize`（文献正文字号）。

> **自动兜底**：若正文中使用了 `[@key]` 引用但未调用 `refs.autoBibliography()` 或 `refs.bibliography()`，`h.build()` 会自动在输出目录和工作目录查找 `references.json` 并追加「参考文献」section，无需手动干预。
