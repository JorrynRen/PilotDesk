# 创建 DOCX 文档

编写 JS 脚本，通过 `python "<技能根目录>\scripts\docx_cli.py" create` 执行生成 `.docx`。

**开始前必读**：

- 阅读 [helpers-api.md](references/helpers-api.md) 了解 docx-helpers 封装层的完整 API

**JS文件写入方式**：

调用 `file(brief=..., action=write, path=..., content=...)` 一次性写入完整 JS 代码。长篇文档分章追加时用 `file(brief=..., action=append, path=..., content=...)`。

---

## 设计美学

创建文档前，先确定设计基调，再写代码。

### 先定基调

根据文档性质选择美学方向，贯穿全篇：

| 基调 | 典型场景 | 视觉特征 |
|------|----------|----------|
| 庄重克制 | 公文、合同、学术论文 | 黑白灰为主，标题仅靠字号加粗区分，无装饰色 |
| 专业商务 | 行业报告、方案、白皮书 | 一个主色贯穿标题和表头，正文深灰，配色不超过 3 色 |
| 活泼表达 | 营销方案、产品手册、盘点 | 主色+强调色，封面可用背景色，表格可交替行色 |
| 文学叙事 | 小说、散文、故事集 | 字体偏宋体/楷体，大行距，无色彩装饰，靠留白营造节奏 |

### 字体搭配

标题字体和正文字体应形成对比，而非全篇同一字体：

- **黑体标题 + 宋体正文**：学术、公文的经典搭配，严肃清晰
- **黑体标题 + 仿宋正文**：政府公文标准
- **雅黑标题 + 雅黑正文**：现代商务，统一感强
- **黑体标题 + 楷体正文**：文学、书信，柔和有温度

避免：全篇楷体（难以阅读长文）、全篇黑体（视觉疲劳）

### 色彩克制

- 定义 `COLORS` 常量后全局引用，禁止裸色值散落各处
- 一个主色（标题、表头、页眉）+ 一个辅助色（强调、链接）+ 正文深灰
- 越正式 → 越趋近黑白；越活泼 → 可引入强调色
- 表格交替行色用主色的极浅变体（如主色 `2B579A` → 交替行 `F2F6FC`）

避免：每章换一套颜色、彩虹表头、正文用彩色

### 空间节奏

文档的"呼吸感"靠间距层次实现：

- **封面**：大量留白 + 少量文字，制造仪式感（spacer 3000+ DXA）
- **标题前间距 > 标题后间距 > 段间距 > 行间距**，形成清晰的层级节奏
- 章节之间用 section 自动分页或 `h.divider()` 过渡，不要连续堆砌内容

### 视觉锚点

避免"文字墙"——每一屏（约一页 A4）至少有一个视觉变化点：

- 表格、列表、引用块、分割线都是天然的锚点
- 不是所有信息都需要表格化——只在对比、结构化数据时使用
- 不要每段都加粗关键词（等于没有重点）

### 封面设计

封面背景优先用 `coverColor` 补丁实现纯色或渐变填充（推荐）。仅当有明确的图片素材时才用 `h.coverBg(imagePath)`。不要用 `h.img()` 放置封面背景——图片定位不可控，极易遮盖文字。

```javascript
// coverSection() 只放文字，背景交给 coverColor 补丁
function coverSection() {
  return [
    h.spacer(3000),
    h.p(标题, { size: 52, bold: true, color: 封面文字色, align: 'center' }),
    h.spacer(400),
    h.p(副标题, { size: 24, color: 封面次要文字色, align: 'center' }),
    h.spacer(1200),
    h.p(作者与日期, { size: 20, color: 封面辅助文字色, align: 'center' }),
  ];
}

h.build({
  sections: [
    { children: coverSection() },
    { ...hf, children: [...chapters()] },
  ],
}, [
  // colors: 1色=纯色, 2色=渐变; direction 默认 vertical
  { type: 'coverColor', colors: [主色, 渐变终止色], direction: 'vertical' },
]);
```

**配色原则：**

- 封面主色从文档配色方案的 `primary` 衍生，保持与正文页视觉统一
- 深色背景（深蓝、深棕、墨绿、纯黑等）配白色/浅色文字，适合报告、小说、学术
- 浅色背景（米白、浅灰、淡蓝等）配深色文字，适合方案、简历、手册
- 封面色和文字色必须有足够明度差——标题与背景至少 5:1 对比度，辅助文字用降低明度的同色系

### 一致性

- 同级标题必须用相同 API（不要混用 `h.h2()` 和 `h.p()` 模拟标题）
- 全文一套配色方案，一套字体搭配
- 相同类型的内容块保持相同的间距和样式

---

## Phase 1：设计文档结构与视觉风格

根据内容性质确定：

- **页面设置**：纸张尺寸（代码默认 US Letter: 12240×15840 DXA；中文场景需显式指定 A4: 11906×16838 DXA）、边距、横竖向
- **样式**：标题层级、正文字体字号、间距。**禁止商业/付费字体**（如方正系列），只用系统自带字体：`SimSun`（宋体）、`SimHei`（黑体）、`KaiTi`（楷体）、`FangSong`（仿宋）、`Microsoft YaHei`（微软雅黑）等
- **段落缩进**：中文公文、报告、小说等正文需要首行缩进时，在初始化配置中设置首行缩进值（见 helpers-api 配置表），全局自动生效
- **色板**：顶部定义语义 `COLORS` 常量后全局引用，禁止到处写裸色值。正文色始终低饱和深色；越正式越趋近黑白灰；学术/正式文档标题与正文同色（仅靠字号加粗区分）；商务/营销可带主题色
- **内容组件**：段落、表格、列表、图片、脚注、目录、页眉页脚
- **封面背景**：纯色/渐变封面用 `coverColor` 补丁（无需图片）；需要实际图片时用 `h.coverBg(imagePath)`；全文背景用 `backgroundImage` 补丁
- **补丁功能**（可选，通过 `h.build()` 第二参数传入）：

| 类型 | 用途 | 关键参数 |
|------|------|----------|
| `watermark` | 文字水印 | `text, color, opacity, rotation` |
| `dropCap` | 首字下沉 | `lines, style, placeholderId?` |
| `stripe` | 表格条纹 | `evenFill, headerFill` |
| `backgroundImage` | 全文背景（每页重复） | `imagePath` |
| `coverColor` | 封面纯色/渐变背景 | `colors, direction?` |
| `pieChart` / `barChart` / `lineChart` | 图表 | `series/values, placeholderId` |

图表优先用 `placeholderId` 定位：正文中插入 `h.p('{{CHART:id}}')` 占位段落，补丁自动替换。

---

## Phase 2：写入 JS 文件

**写入方式**：使用 `file(brief=..., action=write, path=..., content=...)` 一次性写入完整 JS 代码。长篇文档（正文超过 2000 字）必须用 `file(brief=..., action=append, path=..., content=...)` 分章写入，详见下方「长篇创作」。

**编码注意**：中文文本直接写原始字符，不要写 `\uXXXX` 转义。避免 emoji 和不常见 Unicode 符号。

**空文件陷阱**：`file` 工具的 `content` 参数不能为空字符串，否则文件将为空，执行必然失败。

**必须使用 `docx-helpers`**，按功能拆分函数，便于出错时定点修复：

```javascript
const path = require('path');
const h = require(path.join('<技能根目录>', 'scripts', 'docx-helper'))({
  fonts: { heading: 'Microsoft YaHei', body: 'Microsoft YaHei' },
  colors: { primary: '2B579A', accent: '4A90D9', text: '333333', light: 'F2F6FC' },
});

const C = h.colors;

// ── 页面区块（每个函数返回 children 数组）──
function coverSection() { return [/* 封面段落 */]; }
function tocSection() { return [h.h1('目  录', { align: 'center' }), h.spacer(200), h.toc()]; }
function chapter1() { return [/* 第一章内容 */]; }
function chapter2() { return [/* 第二章内容 */]; }

// ── 页眉页脚（一行搞定，多个 section 共享）──
// 页码由 h.build() 自动补到没有 footers 的 section；封面等不需要页码的 section 写 noPageNumber: true。
const hf = h.headerFooter('文档标题');
// 自定义页脚时，pageNum() 是字段标记，需要用 h.text() 包裹做混排：
// const hf = h.headerFooter('文档标题', [
//   h.text('第 '), h.text(h.pageNum(), { bold: true }), h.text(' 页'),
// ]);

// ── 组装（封面、目录、正文各自独立 section → 自动分页，不要再加 h.pageBreak()）──
h.build({
  sections: [
    { noPageNumber: true, children: coverSection() },
    { ...hf, children: tocSection() },
    { ...hf, children: [...chapter1(), ...chapter2()] },
  ],
});
```

### 学术论文场景

学术论文通过自定义配置设置字体/字号/页边距等，并通过 `h.formula()`、`h.math()`、`h.threeLineTable()` 处理公式和三线表。详见 [helpers-api.md](references/helpers-api.md)。

### 补丁功能（可选）

需要图表、水印等补丁时，在 `h.build()` 第二个参数传入补丁数组：

```javascript
h.build({ sections: [...] }, [
  { type: 'watermark', text: '机密', color: 'FF0000', opacity: 0.3 },
  { type: 'barChart', series: [...], placeholderId: 'chart1' },
]);
```

### 长篇创作（正文超过 2000 字必须使用，包括学术论文场景）

**核心原则：所有 append 都是往同一个 JS 文件追加代码，不是创建独立模块。** `h`、`refs`、`hf`、`COLORS` 等变量在骨架中定义一次，后续 append 的代码处于同一文件作用域，可直接使用这些变量，**禁止在 append 中重复 require 或重新声明任何骨架变量（如 `const hf = ...`）**。

1. 第一次 `file(brief=..., action=write, path=..., content=...)`：写入 require、`h` 初始化、`refs`、常量、工具函数、封面、目录等**骨架代码**，不写 `h.build()` 和文件末尾
2. 每章用 `file(brief=..., action=append, path=..., content=...)`：续写该章节的函数定义，函数内直接使用骨架中已有的 `h`、`refs` 等变量
3. 最后一次 append：写入 `h.build({...})` 组装和收尾

```javascript
// ── 第 1 次 write（骨架）──
const path = require('path');
const h = require(path.join('<技能根目录>', 'scripts', 'docx-helper'))({...});
const refs = h.refTracker();
const C = h.colors;
const hf = h.headerFooter('论文标题');

function coverSection() { return [...]; }
function tocSection() { return [...]; }

// ── 第 2 次 append（第一章）── h, refs, C 已在骨架中定义，直接使用
function chapter1() {
  return [
    h.h1('第一章 绪论'),
    h.p('近年来，深度学习在计算机视觉领域取得了显著进展[@wang2024]。'),
  ];
}

// ── 第 N 次 append（仅 h.build，hf 已在骨架中声明，禁止重复 const hf = ...）──
h.build({
  sections: [
    { children: coverSection() },
    { ...hf, children: tocSection() },
    { ...hf, children: [...chapter1(), h.h1('参考文献'), ...refs.autoBibliography(...)] },
  ],
});
```

每次 append 衔接前一次的末尾，确保 JS 语法完整。全部写完后再进入 Phase 3 执行。

---

## Phase 3：执行

通过 **execute_command** 执行，内部自动修复常见问题并预检，执行成功即生成完成：

```bash
python "<技能根目录>\scripts\docx_cli.py" create /path/to/script.js -o /path/to/output.docx
```

## 错误恢复

报错信息会带行号和修复命令模板，按提示定点改即可。如需恢复 JS 脚本的上次成功版本：

```bash
python "<技能根目录>\scripts\docx_cli.py" restore /path/to/script.js
```

---

## 编写规则 Checklist

### 禁止事项

- 禁止写任何 TypeScript 语法（`as any`、类型标注、`interface`、`type` 等）
- 禁止 `new Table()` / `new Paragraph()` / `new Header()` / `new Footer()` / `require('docx')` — 全部通过 `h.xxx()` 封装 API 调用。需要原始类/枚举时从 `h.raw` 解构（如 `const { HorizontalPositionRelativeFrom } = h.raw`），不要 `require('docx')`
- 禁止用 `h.p()` + `indent` 模拟列表 — 用 `h.bullet()` / `h.numbered()`
- 禁止在独立 section 之间加 `h.pageBreak()` — section 自动分页
- 禁止在 `file` 工具写入 JS 文件的同一轮中调用 execute_command 执行——先写完文件，再执行
- 禁止先调用 `creative` 工具生成文本再包装为 JS
- 禁止封装段落辅助函数 — `h.p()` 已自动应用 `indent` 和 `spacing.body` 配置，直接使用即可

### 关键规则

- **有序列表编号重置**：`h.numbered()` 不传 `ref` 时全文共享编号链。需要从 1 重新开始时，必须传不同的 `{ ref: 'xxx' }`
- **标题层级一致性**：同级内容必须用相同 API，严禁混用 `h.h2()` 和 `h.p()` 表示同级标题

### 文体排版参考

| 文体 | 字体 | 缩进 | 特殊要求 |
|------|------|------|---------|
| **公文** | 标题 SimHei，正文 FangSong(32) | `firstLine: 480` | 标题居中加粗；签发人、日期右对齐，不要封面|
| **书信** | KaiTi 或 FangSong | `firstLine: 480` | "此致"缩进，"敬礼！"顶格，落款右对齐，不要封面|
| **合同** | SimSun 或 FangSong | `firstLine: 480` | 条款用 `h.numbered()`；签章区右对齐留空行 |
| **报告/论文** | 标题 SimHei，正文 SimSun | `firstLine: 480` | 摘要 KaiTi；参考文献 `hanging: 480` |
| **新闻/自媒体** | Microsoft YaHei | 不缩进 | 短段落+大行距 |
| **小说/散文** | SimSun 或 YaHei | `firstLine: 480` | 行距≥400；>2000 字用 append 分章写入 |
| **简历** | YaHei 或 SimHei | 不缩进 | 表格布局；`h.divider()` 分隔；不超两页，不要封面|

---

## Troubleshooting

| 问题 | 处理方式 |
|------|----------|
| JS 文件为空（0 字节） | `file` 的 `content` 为空。删除空文件，回到 Phase 2 重新写入。**禁止改用 Python `open()` 写入** |
| `FileNotFoundError` | 检查命令中的路径是否正确 |
| stdout 出现 `[docx-helpers] ... → 已自动兼容` warn | 参数顺序/类型写错但已被自动归一化跑通。下次按 warn 提示调用 |
| `npm 模块缺失` 错误 | `python "<技能根目录>\scripts\docx_cli.py" create` 会自动安装，通常无需干预。若自动安装失败，报错信息中已包含排查方向和手动修复命令，按提示操作即可 |
| 生成的文档无参考文献 | 若正文使用了 `[@key]` 引用但漏写 `refs.autoBibliography()`，`h.build()` 会自动在输出目录或工作目录查找 `references.json` 并追加参考文献 section。若仍无参考文献，检查 `references.json` 是否存在且格式正确 |
