---
name: pdf
description: 当用户需要对 PDF 文件进行任何操作时，使用本技能。包括：读取或提取 PDF 中的文字/表格、合并多个 PDF、拆分 PDF、旋转页面、删除页面、压缩 PDF、加密/解密 PDF，以及对扫描版 PDF 进行 OCR 识别使其可搜索。只要用户提到 .pdf 文件或希望生成 PDF，即使用本技能。
---

> ## PilotDesk 环境适配
> 本技能适配 PilotDesk 桌面客户端，无需任何预设环境变量（不使用 bash、不使用 $SKILL_PATH_BASH、不依赖任何硬编码安装路径）。
> - 技能根目录：由 load_skill 返回内容顶部给出（形如 `> 技能根目录：C:\...\skills\pdf`）；
> - 所有脚本调用统一为：execute_command 运行 `python "<技能根目录>\scripts\pdf_cli.py" <参数>`；
> - 本文档中所有命令均按此方式理解与执行。


# PDF 处理指南

所有 PDF 操作均通过 execute_command 运行 `python "<技能根目录>\scripts\pdf_cli.py"` 完成，无需手写 Python 代码。

## 命令速查

| 任务 | 命令 | 底层 python 库 |
|------|------|--------|
| 查看基本信息 | `pdf_cli.py info` | pypdf |
| 获取总页数 | `pdf_cli.py page-count` | pypdf |
| 提取文本（精准） | `pdf_cli.py extract-text` | pdfplumber |
| 提取文本（快速/损坏文件） | `pdf_cli.py extract-text --fast` | pypdf |
| 提取表格（JSON/CSV） | `pdf_cli.py extract-table` | pdfplumber |
| 拆分 PDF | `pdf_cli.py split` | pypdf |
| 合并多个 PDF | `pdf_cli.py merge` | pypdf |
| 删除指定页面 | `pdf_cli.py delete-pages` | pypdf |
| 旋转页面 | `pdf_cli.py rotate` | pypdf |
| 裁剪页面 | `pdf_cli.py crop` | pypdf |
| 添加整页水印 | `pdf_cli.py watermark` | pypdf |
| 压缩 PDF 体积 | `pdf_cli.py compress` | pypdf |
| 加密 | `pdf_cli.py encrypt` | pypdf |
| 解密 | `pdf_cli.py decrypt` | pypdf |
| 渲染为图片（PNG） | `pdf_cli.py render` | pypdfium2 |
| 提取内嵌图片 | `pdf_cli.py extract-images` | pypdf + Pillow |
| 批量处理 | `pdf_cli.py batch` | — |

---

## 一、信息查询

### 查看 PDF 基本信息

```bash
python "<技能根目录>\scripts\pdf_cli.py" info /path/to/doc.pdf
```

输出页数、首页尺寸、加密状态、标题、作者等元数据。

### 获取总页数

```bash
python "<技能根目录>\scripts\pdf_cli.py" page-count /path/to/doc.pdf
```

仅输出数字，适合脚本管道使用。

---

## 二、文本提取

### 精准提取（pdfplumber）

```bash
python "<技能根目录>\scripts\pdf_cli.py" extract-text /path/to/doc.pdf
```

### 指定页码提取

```bash
python "<技能根目录>\scripts\pdf_cli.py" extract-text /path/to/doc.pdf --pages 1,3,5-7
```

### 快速模式（pypdf，适用于损坏文件）

```bash
python "<技能根目录>\scripts\pdf_cli.py" extract-text /path/to/doc.pdf --fast
```

### 提取表格

```bash
# 提取第 1 页表格（JSON 格式）
python "<技能根目录>\scripts\pdf_cli.py" extract-table /path/to/doc.pdf

# 指定页码，输出 CSV 格式
python "<技能根目录>\scripts\pdf_cli.py" extract-table /path/to/doc.pdf --page 3 --format csv
```

---

## 三、页面操作

### 拆分为单页文件

```bash
# 全部页面拆分
python "<技能根目录>\scripts\pdf_cli.py" split /path/to/doc.pdf -o /output/dir

# 只拆分指定页码（支持逗号、范围）
python "<技能根目录>\scripts\pdf_cli.py" split /path/to/doc.pdf -o /output/dir --pages 1,3,5-7
```

### 合并多个 PDF

```bash
python "<技能根目录>\scripts\pdf_cli.py" merge file1.pdf file2.pdf file3.pdf -o merged.pdf
```

### 删除指定页面

```bash
python "<技能根目录>\scripts\pdf_cli.py" delete-pages /path/to/doc.pdf -o output.pdf --pages 1,3,5-7
```

### 旋转页面

```bash
# 全部旋转 90°
python "<技能根目录>\scripts\pdf_cli.py" rotate /path/to/doc.pdf -o output.pdf --angle 90

# 只旋转第 1、3 页
python "<技能根目录>\scripts\pdf_cli.py" rotate /path/to/doc.pdf -o output.pdf --pages 1,3 --angle 180
```

### 裁剪页面

```bash
# 裁剪所有页面（PDF 坐标系：左下角为原点）
python "<技能根目录>\scripts\pdf_cli.py" crop /path/to/doc.pdf -o cropped.pdf --bbox 50,50,550,750

# 只裁剪指定页
python "<技能根目录>\scripts\pdf_cli.py" crop /path/to/doc.pdf -o cropped.pdf --bbox 0,0,300,400 --pages 1,3
```

> 注意：`--bbox` 使用 PDF 原生坐标系（左下角为原点，y 向上），与屏幕坐标（左上角原点，y 向下）方向相反。

### 添加水印

```bash
python "<技能根目录>\scripts\pdf_cli.py" watermark /path/to/doc.pdf -o output.pdf --watermark watermark.pdf
```

水印 PDF 的第一页将叠加到每页上方。可用 reportlab 生成透明文字水印 PDF 后再合入。

---

## 四、压缩

```bash
python "<技能根目录>\scripts\pdf_cli.py" compress /path/to/doc.pdf -o compressed.pdf
```

压缩内容流并移除冗余对象，输出原始大小与压缩后大小。

---

## 五、安全

### 加密

```bash
python "<技能根目录>\scripts\pdf_cli.py" encrypt /path/to/doc.pdf -o encrypted.pdf --password mypass123
```

### 解密

```bash
python "<技能根目录>\scripts\pdf_cli.py" decrypt /path/to/encrypted.pdf -o decrypted.pdf --password mypass123
```

---

## 六、转换与导出

### 渲染为图片

```bash
# 全部页面渲染为 PNG（默认 150 DPI）
python "<技能根目录>\scripts\pdf_cli.py" render /path/to/doc.pdf -o /output/dir

# 指定页码和 DPI
python "<技能根目录>\scripts\pdf_cli.py" render /path/to/doc.pdf -o /output/dir --pages 1,3 --dpi 300
```

### 提取内嵌图片

```bash
python "<技能根目录>\scripts\pdf_cli.py" extract-images /path/to/doc.pdf -o /output/dir
```

提取 PDF 中嵌入的所有图片资源，按页码命名保存。

---

## 七、批量处理

对目录下所有 PDF 批量执行某个子命令：

```bash
# 批量压缩
python "<技能根目录>\scripts\pdf_cli.py" batch /path/to/pdf_dir compress -o /output/dir

# 批量旋转 90°
python "<技能根目录>\scripts\pdf_cli.py" batch /path/to/pdf_dir rotate -o /output/dir -- --angle 90

# 指定文件模式
python "<技能根目录>\scripts\pdf_cli.py" batch /path/to/pdf_dir compress -o /output/dir --pattern "report_*.pdf"
```

---

## 注意事项

- **加密 PDF**：`extract-text`、`info` 等操作前需先用 `decrypt` 解密。
- **损坏/结构异常 PDF**：加 `--fast` 参数，底层用 pypdf 的宽容模式读取。

---

# Python 扩展参考

> 当上述 CLI 命令无法满足需求时（如复杂多页报告排版、自定义字体样式、特殊坐标计算等），可直接用 Python 扩展。以下代码均可在 Python 工具中直接运行。

## pdfplumber — 精确坐标与复杂表格

底层库：**pdfplumber**

```python
import pdfplumber

with pdfplumber.open("doc.pdf") as pdf:
    page = pdf.pages[0]

    # 逐字符提取含坐标信息
    for char in page.chars[:10]:
        print(f"'{char['text']}' x:{char['x0']:.1f} y:{char['y0']:.1f} font:{char['fontname']}")

    # 复杂布局自定义策略
    tables = page.extract_tables({
        "vertical_strategy": "lines",
        "horizontal_strategy": "lines",
        "snap_tolerance": 3,
        "intersection_tolerance": 15,
    })

    # 可视化调试表格检测结果
    page.to_image(resolution=150).debug_tablefinder().save("debug.png")
```

---

## reportlab — 复杂表格与多页报告

底层库：**reportlab**

```python
from reportlab.platypus import SimpleDocTemplate, Table, TableStyle, Paragraph, PageBreak
from reportlab.lib.styles import getSampleStyleSheet
from reportlab.lib import colors
from reportlab.lib.pagesizes import letter

doc = SimpleDocTemplate("report.pdf", pagesize=letter)
styles = getSampleStyleSheet()

data = [
    ["产品", "Q1", "Q2", "Q3", "Q4"],
    ["Widget", "120", "135", "142", "158"],
    ["Gadget", "85",  "92",  "98",  "105"],
]
table = Table(data, colWidths=[120, 60, 60, 60, 60])
table.setStyle(TableStyle([
    ("BACKGROUND",     (0, 0), (-1,  0), colors.HexColor("#4472C4")),
    ("TEXTCOLOR",      (0, 0), (-1,  0), colors.white),
    ("FONTNAME",       (0, 0), (-1,  0), "Helvetica-Bold"),
    ("ALIGN",          (0, 0), (-1, -1), "CENTER"),
    ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#EEF2FF")]),
    ("GRID",           (0, 0), (-1, -1), 0.5, colors.grey),
    ("BOX",            (0, 0), (-1, -1), 1,   colors.black),
]))

doc.build([
    Paragraph("销售报告", styles["Title"]),
    table,
    PageBreak(),
    Paragraph("第二页内容", styles["Normal"]),
])
```

> **中文字体**：reportlab 默认字体不含中文，生成含中文内容时需先注册系统字体：
> ```python
> from reportlab.pdfbase import pdfmetrics
> from reportlab.pdfbase.ttfonts import TTFont
> pdfmetrics.registerFont(TTFont("CJK", r"C:\Windows\Fonts\msyh.ttc"))
> styles["Normal"].fontName = "CJK"
> ```
> 下标/上标用 XML 标签：`H<sub>2</sub>O`、`x<super>2</super>`。

---

## pypdfium2 — 渲染为图片

底层库：**pypdfium2**（基于 Chromium PDFium，无需 poppler）

```python
import pypdfium2 as pdfium

pdf = pdfium.PdfDocument("doc.pdf")
for i, page in enumerate(pdf):
    bitmap = page.render(scale=2.0)   # scale=2 约为 192 DPI
    bitmap.to_pil().save(f"page_{i+1}.png")

# 提取文本（纯文字型 PDF）
for i, page in enumerate(pdf):
    print(f"第 {i+1} 页：{page.get_text()[:100]}")
```

---

## pypdf — 批量处理与杂项

底层库：**pypdf**

```python
import glob
from pypdf import PdfReader, PdfWriter

# 批量合并目录下所有 PDF（容错跳过损坏文件）
writer = PdfWriter()
for pdf_file in sorted(glob.glob("input/*.pdf")):
    try:
        for page in PdfReader(pdf_file).pages:
            writer.add_page(page)
    except Exception as e:
        print(f"跳过 {pdf_file}：{e}")
with open("merged_all.pdf", "wb") as f:
    writer.write(f)

# 处理加密 PDF
reader = PdfReader("enc.pdf")
if reader.is_encrypted:
    reader.decrypt("password")

# 提取嵌入图片
from PIL import Image
import io
for page in reader.pages:
    for img_obj in page.images:
        Image.open(io.BytesIO(img_obj.data)).save(f"{img_obj.name}.png")

# 宽容模式读取损坏 PDF
reader = PdfReader("damaged.pdf", strict=False)
```
