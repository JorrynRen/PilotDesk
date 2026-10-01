---
name: xlsx
description: 创建、编辑或分析 .xlsx / .xlsm / .xls / .csv / .tsv 文件。当用户要求生成、处理、下载 Excel 表格，或提及表格文件名/路径并希望对其操作时，使用本技能。包括：打开、读取、编辑、修复已有文件（增列、计算、格式化、图表、数据清洗），从零或其他数据源创建新表格，以及表格格式间的转换。对于结构混乱的表格数据（错行、表头错位、垃圾数据）清洗为规范表格也适用。交付物须为表格文件；当用户未指定格式时默认生成 .xlsx 而非 .csv 等其他格式。若主要产出为 Word 文档、HTML 报告、独立 Python 脚本或数据库管道，则不应触发本技能。
---

> ## PilotDesk 环境适配
> 本技能适配 PilotDesk 桌面客户端，无需任何预设环境变量（不使用 bash、不使用 $SKILL_PATH_BASH、不依赖任何硬编码安装路径）。
> - 技能根目录：由 load_skill 返回内容顶部给出（形如 `> 技能根目录：C:\...\skills\xlsx`）；
> - 所有脚本调用统一为：execute_command 运行 `python "<技能根目录>\scripts\xlsx_cli.py" <参数>`；
> - 本文档中所有命令均按此方式理解与执行。

# 核心原则

每份交付的 Excel 文件须同时满足以下四点：

1. **公式优先**：始终使用 Excel 公式，而非在 Python 中计算后将结果硬编码写入，这样表格才能在源数据变化时自动重算
2. **零公式错误**：通过 `recalc` 模块使用纯 Python 引擎重算所有公式，并扫描全部单元格检查错误，不得出现 `#REF!` `#DIV/0!` `#VALUE!` `#N/A` `#NAME?`
3. **视觉美观**：使用统一的专业字体（如 Arial、微软雅黑），表头与数据区层次分明，数字格式统一，列宽适配内容，整体风格协调
4. **图表配套**：数据中存在对比、趋势或占比维度时，配套生成相应图表（折线图 / 柱状图 / 饼图等），使用`TwoCellAnchor`网格系统精确定位，防止图表重叠
5. **散点图硬约束**：**禁止**手写 `Series(Reference...)` + `scatter.legend`；**必须** `chart_labels.add_scatter_xy_series(..., label_col=...)` + `finalize_scatter_chart`（点旁编号、无图例）
6. **保留已有模板**：修改现有文件时，必须完全匹配原有格式、样式和约定，绝不覆盖已有样式，现有约定优先于本指南
7. **区分任务类型，保护原表数据**：先判断用户意图——若是多个文件汇总为一个新表，可创建新文件；若是基于现有文件进行处理（如分析、汇总、增加图表），则必须在原文件基础上操作，保留所有原始 Sheet 及其数据，将结果写入新增的 Sheet，不得丢弃或覆盖原始数据

---

# 工作流

## 工具使用规则

输出文件保存到当前工作目录。

**脚本化。** 创建/编辑表格时：按任务编写脚本（如 `report.py`），再用 execute_command 运行 `python "<技能根目录>\scripts\xlsx_cli.py" build` 执行；详见 `<技能根目录>\build-workflow.md`。读取、预览表格时直接执行 Python（如 `df.head()`、查看若干单元格）。PilotDesk 不注入 SKILL_PATH* 环境变量，脚本调用统一走上述固定路径。

| 场景 | 推荐方式 |
|------|----------|
| 创建 / 编辑表格 | 写 `<task>.py` → `python "<技能根目录>\scripts\xlsx_cli.py" build <task>.py` |
| 读取、预览表格 | 直接执行 Python |
| 公式校验（含公式时必须） | `python "<技能根目录>\scripts\xlsx_cli.py" recalc <file.xlsx>` |
| 公式结构审计（含公式时必须） | `python "<技能根目录>\scripts\xlsx_cli.py" audit <file.xlsx>` |

**硬性规则：**

- 含公式时必须 recalc 至 `status: success`，再执行 audit；按 `summary` / `samples` 处理 warning（修复或确认误报/有意设计后保留），禁止整批无说明放行。
- 编辑已有文件保存后顺序为：**save → recalc（含公式时）→ audit（含公式时）**。**仅改公式、替换 `+` 为 `SUM`、改 1～N 个单元格也不例外**——改完即交付、跳过 audit 视为未完成。

## 执行步骤

1. **规划结构**：确定 Sheet 划分、数据布局、需要生成的图表类型
2. **识别数据表**（写公式前必须执行，仅看单元格/pandas **不够**）：用 openpyxl 打开后打印各 Sheet 的 Table：
   ```python
   from openpyxl import load_workbook
   wb = load_workbook("input.xlsx")
   for ws in wb.worksheets:
       print(ws.title, list(ws.tables.keys()))  # 如 ['InventoryTable']
   ```
   若存在 Table，针对该表的 SUM/SUMIF/COUNTIF/AVERAGE 等聚合**必须**用结构化引用，且公式格写在 Table **外**（禁止把合计行包进 Table 再 `SUM(Table[列])`，会循环引用）；行内可用 `[@列]`；跨表逐行取数仍可用 A1。
3. **选择库**：数据分析用 pandas，公式 / 格式 / 图表处理用 openpyxl
4. **编写并执行**：
   - **脚本化**（创建/编辑）：阅读 `<技能根目录>\build-workflow.md`，按任务写入脚本（如 `report.py`），再执行：
     ```bash
     python "<技能根目录>\scripts\xlsx_cli.py" build report.py
     ```
   - **读取 / 预览**：直接执行 Python（如 `pd.read_excel` + `df.head()`）；**编辑已有文件时预览后仍须完成步骤 2**
5. **验证与修复**：
   - 脚本 `__main__` 通常已调用 recalc；也可单独校验：
     ```bash
     python "<技能根目录>\scripts\xlsx_cli.py" recalc output.xlsx --timeout 60
     ```
   - 若返回 `status: "errors_found"`，根据 `error_summary` 修复后重跑；脚本化任务改脚本而非整段重写
   - 常见错误：`#REF!`、`#DIV/0!`、`#VALUE!`、`#NAME?`、`array_formula_risk`
6. **公式结构审计**（新建或修改了公式时必须执行；交付前最后一步）：
   ```bash
   python "<技能根目录>\scripts\xlsx_cli.py" audit output.xlsx
   ```
   或在短 Python / 脚本内：
   ```python
   import sys, os
   sys.path.insert(0, os.path.join('<技能根目录>', 'scripts'))
   from audit import audit_formulas
   result = audit_formulas(output_path)
   print(result)
   ```
   - 检测 recalc 发现不了的结构问题（启发式，可能误报）：
     - **引用范围缺失**：SUM 公式漏行（如 `=SUM(B3:B4)` 漏了数值 B2；上方为表头文字不报）
     - **公式被覆盖为硬编码值**：同列公式块中间，或紧邻块首/块尾一行的裸数字（≥2 列数字轴、或该格被公式引用时抑制）
     - **硬编码假设藏在公式里**：公式含非整数常量（如 `=B5*0.21`；同行/同列轴标签同值会抑制）
     - **同列公式不一致**：少数行公式结构与同列多数行不同（字符串字面量差异不计入，如 COUNTIF 分组名）
     - **Table 内固定 A1 聚合**：已建 Table 仍写 `=SUM/SUMIF/COUNTIF(...A1:Bn)` → `fixed_range_on_table`
     - **Table 内列结构化合计**：合计格包进 Table 且写 `=SUM(Table[列])` → `structured_ref_inside_table`（循环引用）
   - 返回含 `summary`（按 type 计数）、`warnings`（已聚合，含 `count`/`samples`）、`raw_warning_count`
   - 若 `status: "warnings_found"`：先读 `summary`，再按 `samples` 处理；能修则修；确认误报或有意设计可保留并说明原因。禁止仅因条数多为由整批放行

---

# 关键原则：使用公式，不要硬编码值

**始终使用 Excel 公式，而非在 Python 中计算后将结果硬编码写入。** 这样表格才能在源数据变化时自动重算。

### 错误做法
```python
total = df['Sales'].sum()
sheet['B10'] = total          # 硬编码为 5000

growth = (df.iloc[-1]['Revenue'] - df.iloc[0]['Revenue']) / df.iloc[0]['Revenue']
sheet['C5'] = growth          # 硬编码为 0.15
```

### 正确做法
```python
sheet['B10'] = '=SUM(B2:B9)'
sheet['C5'] = '=(C4-C2)/C2'
sheet['D20'] = '=AVERAGE(D2:D19)'
```

所有计算——合计、百分比、比率、差值——均适用此原则。

### 结构化引用：写能随数据扩展的公式

步骤 2 若发现 Excel Table（如 `InventoryTable`）：**对该表的聚合用结构化引用**，不要写 `D2:D9` 这类固定范围。

```python
# ✅ 列聚合写在 Table 外（Table ref 不含合计行）
# Table = A1:D9；合计在第 10 行
'=SUM(InventoryTable[库存数量])'          # 如 B10，不在 Table 内
'=COUNTIF(InventoryTable[类别],"配件")'
'=SUMIF(InventoryTable[类别],"配件",InventoryTable[库存数量])'

# ✅ 行内计算可写在 Table 内
'=InventoryTable[@Revenue]-InventoryTable[@Cost]'

# ❌ 表已是 Table 仍写固定范围
'=SUM(D2:D9)'

# ❌ 合计行包进 Table，再写列结构化合计 → 循环引用（WPS/Excel 报错，常显示 0）
# Table ref=A1:D10，B10='=SUM(销售月报[Revenue])'
```

**规则**：
- 数据在 Table 中 → 列聚合用 `TableName[Column]`，且公式格必须在 Table **外**（或用 Table Totals Row）
- 行内用 `[@Column]`，可写在 Table 内
- **禁止**把 `=SUM(Table[列])` 等列聚合写进该 Table 的 `ref` 范围内（会循环引用）
- 数据不在 Table 中但会增长 → 使用整列引用 `=SUM(B:B)`
- 一次性固定数据 → 固定范围可以接受

---

# 多 Sheet 处理复杂任务

当任务涉及多个维度、多个数据集或数据量较大时，将内容合理拆分到多个 Sheet，而非堆在一张表上。每个 Sheet 聚焦单一主题，并在顶部用简短说明交代该 Sheet 的用途与数据范围，帮助读者快速定位所需信息。

---

# 技术参考

## 读取与分析数据

### 使用 pandas

```python
import pandas as pd

df = pd.read_excel('file.xlsx')
all_sheets = pd.read_excel('file.xlsx', sheet_name=None)

df.head()
df.info()
df.describe()

df.to_excel('output.xlsx', index=False)
```

## 创建新 Excel 文件

```python
from openpyxl import Workbook
from openpyxl.styles import Font, PatternFill, Alignment

wb = Workbook()
sheet = wb.active

sheet['A1'] = 'Hello'
sheet['B1'] = 'World'
sheet.append(['Row', 'of', 'data'])

sheet['B2'] = '=SUM(A1:A10)'

sheet['A1'].font = Font(bold=True, color='FF0000')
sheet['A1'].fill = PatternFill('solid', start_color='FFFF00')
sheet['A1'].alignment = Alignment(horizontal='center')
sheet.column_dimensions['A'].width = 20

wb.save('output.xlsx')
```

## 编辑现有 Excel 文件

打开后**先打印 Table**（步骤 2），再改公式；不要只 `read_excel` / 看格子。

```python
from openpyxl import load_workbook

wb = load_workbook('existing.xlsx')
for ws in wb.worksheets:
    print(ws.title, list(ws.tables.keys()))  # 必做：有 Table 则聚合用结构化引用

sheet = wb.active
sheet['A1'] = 'New Value'
sheet.insert_rows(2)
sheet.delete_cols(3)

new_sheet = wb.create_sheet('NewSheet')
new_sheet['A1'] = 'Data'

wb.save('modified.xlsx')
```

编辑保存后若含公式（含仅改 F 列/`SUM` 替换等小改动），须按工作流执行 `recalc` → `audit` 后再交付；**不得**只 `wb.save` 就结束。

## 重算公式

openpyxl 写入的公式只是字符串，Excel 打开前不会有计算值。通过 `recalc` 模块使用纯 Python 引擎重算所有公式，并扫描全部单元格检查错误。无需安装任何外部软件。

### 使用方式

**推荐（CLI）：**

```bash
python "<技能根目录>\scripts\xlsx_cli.py" recalc /path/to/output.xlsx --timeout 60
```

**或在 build 脚本内：**

```python
import sys, os
sys.path.insert(0, os.path.join('<技能根目录>', 'scripts'))
from recalc import recalc

result = recalc("/path/to/output.xlsx")
print(result)
```

### 函数签名

```python
recalc(filename: str, timeout: int = 30) -> dict
```

- `filename`：xlsx 文件的绝对路径
- `timeout`：重算超时秒数，默认 30（大文件可显式传 60）

### 返回值

```json
{
  "status": "success",
  "total_errors": 0,
  "total_formulas": 42,
  "error_summary": {}
}
```

若 `status` 为 `errors_found`，`error_summary` 会列出每种错误的位置（最多 20 处）：

```json
{
  "status": "errors_found",
  "total_errors": 2,
  "total_formulas": 42,
  "error_summary": {
    "#REF!": {
      "count": 2,
      "locations": ["Sheet1!B5", "Sheet1!C10"]
    }
  }
}
```

除常规 Excel 错误外，还可能返回 `array_formula_risk`（recalc 算对但 Excel 打开会 `#VALUE!` 的隐式数组公式）：

```json
{
  "status": "errors_found",
  "total_errors": 5,
  "total_formulas": 42,
  "error_summary": {
    "array_formula_risk": {
      "count": 5,
      "locations": ["汇总!D6", "汇总!E6"],
      "hint": "请改写为 MEDIAN(range) 或 AVERAGEIF / AGGREGATE 等写法"
    }
  }
}
```

### 支持的函数范围

recalc 使用纯 Python 公式引擎，覆盖大多数常用 Excel 函数：
- 数学：SUM, AVERAGE, COUNT, COUNTA, MIN, MAX, ROUND, ABS, MOD
- 逻辑：IF, AND, OR, NOT, IFERROR
- 查找：VLOOKUP, HLOOKUP, INDEX, MATCH, OFFSET
- 条件：SUMIF, SUMIFS, COUNTIF, COUNTIFS, AVERAGEIF
- 文本：LEFT, RIGHT, MID, LEN, CONCATENATE, TRIM, UPPER, LOWER
- 日期：DATE, YEAR, MONTH, DAY, TODAY, NOW
- 财务：NPV, IRR, PMT, FV, PV

若遇到不支持的函数，recalc 会降级为静态分析（检查引用和结构），并在返回值中包含 `warning` 字段说明情况。

## 公式验证清单

### 必要验证
- [ ] 凡写入/修改公式：已跑 `xlsx_cli.py recalc`（含公式时）且 `xlsx_cli.py audit`，**未**用自制检查冒充
- [ ] 先在 2-3 个单元格上测试公式，确认值正确后再批量应用
- [ ] 列映射：确认 Excel 列对应关系（第 64 列 = BL）
- [ ] 行偏移：Excel 行从 1 开始（DataFrame 第 5 行 = Excel 第 6 行）
- [ ] 验证公式引用的所有单元格确实存在

### 常见陷阱
- [ ] NaN 处理：用 `pd.notna()` 检查空值
- [ ] 远右侧列：财年数据常在第 50+ 列
- [ ] 多重匹配：搜索所有匹配项而非只取第一个
- [ ] 除以零：在公式中使用 `/` 前检查分母（`#DIV/0!`）
- [ ] 跨表引用：使用正确格式（`Sheet1!A1`）
- [ ] 边界测试：包含零值、负数和超大数值的场景
- [ ] **数组公式盲区**：`recalc` 与 Excel 对区域运算的语义不同（见下文）
- [ ] **条形/柱状图轴标题**：用 `configure_bar_chart` + `set_bar_axis_titles`；禁止 `y_axis='公司'` 直觉写法
- [ ] **散点图**：`label_col` 点旁编号 + `finalize_scatter_chart`；**禁止** `title=` / `scatter.legend`（无图例）
- [ ] **图表数据列可见**：**禁止**隐藏图表 `Reference` 引用的列（含辅助数据列）；隐藏后 Excel/WPS 图表可能显示为空白

### ⚠️ 数组公式盲区（recalc 通过但 Excel 报 #VALUE!）

`recalc` 使用纯 Python 引擎，**默认把区域当列表做逐元素运算**；Excel 普通公式模式则默认标量运算，只有 Ctrl+Shift+Enter 数组公式或动态数组函数才会逐元素处理。`openpyxl` 写入公式时也不会自动添加 `ArrayFormula` 标记。

| 场景 | `recalc`（Python） | Excel（普通公式） |
| :--- | :--- | :--- |
| `IF(G13:G14<>"", G13:G14)` | 对区域逐元素判断，返回列表 | 只对区域**第一个单元格**判断，返回标量 |
| `MEDIAN(IF(...))` | 收到列表 → 正常算中位数 | 收到标量/错误 → **`#VALUE!`** |

因此 `recalc` 返回 `success`，但用户用 Excel 打开会看到 `#VALUE!`。`recalc` 现已对 `聚合函数(IF(区域...))` 模式做额外检测，命中时返回 `array_formula_risk`。

#### ❌ 错误写法
```python
# 过滤空值再取中位数 — recalc 能算，Excel 会 #VALUE!
sheet['D6'] = '=MEDIAN(IF(数据表!G13:G14<>"", 数据表!G13:G14))'
sheet['E6'] = '=AVERAGE(IF(数据表!H13:H14>0, 数据表!H13:H14))'
```

#### ✅ 正确写法
```python
# MEDIAN / AVERAGE 本身会忽略空单元格，无需 IF 过滤
sheet['D6'] = '=MEDIAN(数据表!G13:G14)'
sheet['E6'] = '=AVERAGE(数据表!H13:H14)'

# 条件聚合用原生支持区域的函数
sheet['F6'] = '=AVERAGEIF(数据表!G13:G14, "<>0", 数据表!G13:G14)'
sheet['G6'] = '=AGGREGATE(12, 6, 数据表!G13:G14)'  # 12=MEDIAN，6=忽略错误值

# 必须做复杂条件过滤时，用辅助列或在支持动态数组的环境用 FILTER
# sheet['H6'] = '=MEDIAN(FILTER(数据表!G13:G14, 数据表!G13:G14<>""))'
```

**原则**：优先写 Excel 原生支持区域参数的公式，避免 `聚合函数(IF(区域条件, 区域值))` 这种隐式数组写法。

---

# 财务模型规范

### 颜色编码（除非用户或现有模板另有说明）
- **蓝色文字（RGB: 0,0,255）**：硬编码输入值
- **黑色文字（RGB: 0,0,0）**：所有公式和计算结果
- **绿色文字（RGB: 0,128,0）**：同一工作簿内跨工作表的引用链接
- **红色文字（RGB: 255,0,0）**：链接到其他文件的外部引用
- **黄色背景（RGB: 255,255,0）**：关键假设或需要更新的单元格

### 数字格式
- **年份**：文本字符串（如 "2024" 而非 "2,024"）
- **货币**：`$#,##0`，表头注明单位（如 "Revenue ($mm)"）
- **零值**：显示为 "-"（格式：`$#,##0;($#,##0);-`）
- **百分比**：`0.0%`
- **倍数**：`0.0x`
- **负数**：括号形式 `(123)`

### 公式构建
- 将所有假设放在独立单元格，公式引用单元格而非硬编码：`=B5*(1+$B$6)` 而非 `=B5*1.05`
- 修改现有模板时，必须完全匹配原有格式、样式和约定，现有约定优先于本指南

### 硬编码值来源文档
- 所有硬编码数值须标注来源，格式：`Source: [系统/文档], [日期], [具体引用], [URL（如有）]`
- 示例：
  - `Source: 公司年报, FY2024, 第45页, 营收附注`
  - `Source: Bloomberg Terminal, 2025/8/15, AAPL US Equity`
  - `Source: Wind, 2025/8/20, 一致预期数据`

### 图表布局（TwoCellAnchor 网格系统）

使用 `TwoCellAnchor` 精确定位，**必须遵循网格布局规则防止重叠**。

#### 网格布局规则（必须遵守）

将图表区域看作网格，先确定布局参数，再算每个图表的坐标：

```
布局参数（先确定再写代码）:
  DATA_END_ROW  = 数据区最后一行（0-indexed）
  CHART_START   = DATA_END_ROW + 2（图表起始行，留 1 行缓冲）
  COLS_PER_CHART = 7（每个图表占的列数，推荐 6-8）
  ROWS_PER_CHART = 15（每个图表占的行数，推荐 14-16）
  GAP_COLS = 1（列间距，必须 ≥ 1）
  GAP_ROWS = 2（行间距，必须 ≥ 2）

网格坐标计算公式:
  第 i 行第 j 列图表（i, j 从 0 开始）:
    from_col = j * (COLS_PER_CHART + GAP_COLS)
    from_row = CHART_START + i * (ROWS_PER_CHART + GAP_ROWS)
    to_col   = from_col + COLS_PER_CHART
    to_row   = from_row + ROWS_PER_CHART
```

**核心规则**：
1. 相邻图表的坐标范围**绝不允许重叠** — 即一个图表的 `to_col` 必须 ≤ 下一列图表的 `from_col`，`to_row` 必须 ≤ 下一行图表的 `from_row`
2. **图表区域内禁止存放任何数据** — 图表网格占据的整个矩形区域（从 `CHART_START` 行、第 0 列开始，到最后一个图表的 `to_row`、`to_col`）内不得写入辅助数据
3. 图表需要的辅助数据（如分布统计、饼图源数据等）必须放在**图表区域之外**：写在主数据表的右侧空列（确保不与图表列重叠），或写在单独的 sheet 中
4. **禁止隐藏图表引用的列**（除非用户明确要求）：`Reference` 指向的列（含图表辅助列、标签列）必须保持可见。Excel/WPS 在源列隐藏时图表常显示为空白

```python
# ❌ 禁止：隐藏图表数据源列会导致图表空白
ws.column_dimensions['Q'].hidden = True
ws.column_dimensions['R'].hidden = True

# ✅ 辅助列保持可见；若嫌碍眼，可仅缩窄列宽，或把辅助数据放到独立 sheet
ws.column_dimensions['Q'].width = 10
```

若用户明确要求「隐藏辅助列」，须先确认其查看器行为可接受，或改为独立 `图表数据` sheet 引用。

#### 代码模板（6 个图表 2×3 网格）

```python
from openpyxl.chart import BarChart, PieChart, LineChart
from openpyxl.drawing.spreadsheet_drawing import TwoCellAnchor

# 1. 确定布局参数
CHART_START = 19      # 数据区结束后 +2
COLS = 7              # 每图表占 7 列
ROWS = 15             # 每图表占 15 行
GAP_C = 1             # 列间距（图表之间留 1 列空白）
GAP_R = 2             # 行间距（图表之间留 2 行空白）

def place_chart(ws, chart, grid_row, grid_col):
    """将图表放入网格的 (grid_row, grid_col) 位置，0-indexed"""
    a = TwoCellAnchor()
    a._from.col = grid_col * (COLS + GAP_C)
    a._from.row = CHART_START + grid_row * (ROWS + GAP_R)
    a.to.col = a._from.col + COLS
    a.to.row = a._from.row + ROWS
    chart.anchor = a
    ws._charts.append(chart)

# 2. 创建图表并放入网格（图表之间自动留白）
#    第 0 行: chart1(0,0)  [1列空白]  chart2(0,1)
#    [2行空白]
#    第 1 行: chart3(1,0)  [1列空白]  chart4(1,1)
#    [2行空白]
#    第 2 行: chart5(2,0)  [1列空白]  chart6(2,1)
place_chart(ws, chart1, 0, 0)  # A20:G34
place_chart(ws, chart2, 0, 1)  # I20:O34  (H列空白)
place_chart(ws, chart3, 1, 0)  # A37:G51  (35-36行空白)
place_chart(ws, chart4, 1, 1)  # I37:O51
place_chart(ws, chart5, 2, 0)  # A54:G68
place_chart(ws, chart6, 2, 1)  # I54:O68
```

**关键点**：
- `anchor._from.col/row` 和 `anchor.to.col/row` 都是 **0-indexed**
- 第 1 行 = `row = 0`，A 列 = `col = 0`
- 占据行数 = `to.row - _from.row`
- **不要手动计算每个图表的坐标** — 用 `place_chart` 函数或等价的公式统一计算，避免算错

#### 图表数据引用（Reference）

**关键原则**：`Reference` 的行列范围必须精确匹配数据区域，避免包含多余的表头或空行。

```python
from openpyxl.chart import Reference

# 假设数据结构：
# A1: 姓名  B1: 语文  C1: 数学  D1: 英语  E1: 总分
# A2: 张三  B2: 85    C2: 90    D2: 88    E2: 263
# A3: 李四  B3: 78    C3: 82    D3: 85    E3: 245
# ... (共 8 行数据，A2:E9)

# ✅ 正确：柱状图显示总分
data = Reference(ws, min_col=5, min_row=1, max_row=9)  # E1:E9（包含表头"总分"）
categories = Reference(ws, min_col=1, min_row=2, max_row=9)  # A2:A9（学生姓名，不含表头）
chart.add_data(data, titles_from_data=True)  # titles_from_data=True 会把 E1 作为系列名
chart.set_categories(categories)

# ✅ 正确：折线图显示三科成绩
data = Reference(ws, min_col=2, max_col=4, min_row=1, max_row=9)  # B1:D9（包含表头）
categories = Reference(ws, min_col=1, min_row=2, max_row=9)  # A2:A9（学生姓名）
chart.add_data(data, titles_from_data=True)  # B1:D1 作为系列名（语文、数学、英语）
chart.set_categories(categories)

# ❌ 错误：data 包含了姓名列
data = Reference(ws, min_col=1, max_col=4, min_row=1, max_row=9)  # 错误地包含了 A 列
chart.add_data(data, titles_from_data=True)  # 会把"姓名"也当成数据系列

# ❌ 错误：categories 包含了表头
categories = Reference(ws, min_col=1, min_row=1, max_row=9)  # 错误地包含了 A1
chart.set_categories(categories)  # 横轴会显示"姓名 张三 李四..."
```

**关键点**：
- `min_row=1` 且 `titles_from_data=True` → 第 1 行作为系列名（图例）
- `min_row=2` → 从第 2 行开始取数据
- `categories` 通常不包含表头（`min_row=2`）
- `data` 包含表头时设置 `titles_from_data=True`

**饼图特殊说明**：
```python
# 饼图通常只有一个数据系列，不需要系列名
# ✅ 正确：data 和 categories 都不包含表头
data = Reference(ws, min_col=2, min_row=2, max_row=5)  # B2:B5（数值）
categories = Reference(ws, min_col=1, min_row=2, max_row=5)  # A2:A5（分类名）
pie_chart.add_data(data, titles_from_data=False)  # 饼图不需要系列名
pie_chart.set_categories(categories)

# ❌ 错误：data 包含表头会导致标签显示 "表头名, 分类名, 值, 百分比"
data = Reference(ws, min_col=2, min_row=1, max_row=5)  # B1:B5（包含表头）
pie_chart.add_data(data, titles_from_data=True)  # 错误！饼图标签会变成 "人数, 90分以上, 10, 100%"
```

#### ⚠️ 条形/柱状图（BarChart）轴标题

openpyxl 将 **`x_axis` → catAx（分类轴）**、**`y_axis` → valAx（数值轴）**，与直觉上的「x=横轴数值、y=纵轴分类」**相反**。横向条形图（`type='bar'`）若按直觉写 `y_axis.title='公司'`，WPS 会出现**轴标题与数据对调**（截图：左侧公司名却标「市值」）。

##### ❌ 错误写法

```python
chart1 = BarChart()
chart1.type = 'bar'
chart1.y_axis.title = '公司'           # ❌ 直觉写法，标题会标到数值轴
chart1.x_axis.title = '市值 (亿USD)'    # ❌ 标题会标到分类轴
```

##### ✅ 正确写法

```python
from chart_labels import configure_bar_chart, set_bar_axis_titles

chart1 = BarChart()
chart1.title = '市值对比 (亿USD)'
configure_bar_chart(chart1, horizontal=True)  # axPos: 分类左、数值下
set_bar_axis_titles(chart1, category='公司', value='市值 (亿USD)')

data_ref = Reference(ws, min_col=7, min_row=5, max_row=DATA_END_ROW)
cat_ref = Reference(ws, min_col=2, min_row=DATA_START_ROW, max_row=DATA_END_ROW)
chart1.add_data(data_ref, titles_from_data=True)
chart1.set_categories(cat_ref)
# TwoCellAnchor 定位，禁止 add_chart + height/width
```

纵向柱状图用 `configure_bar_chart(chart, horizontal=False)` + 同一 `set_bar_axis_titles`。

#### ⚠️ 散点图（ScatterChart）

**标准模式（默认）**：点旁编号/名称，**无图例**；含义用 `scatter.title` + 轴标题。

散点图须用 `<技能根目录>\scripts\chart_labels.py`。**禁止**手写 `Series(Reference...)`、`scatter.legend`、`Marker` 循环。

| 易错点 | 说明 |
| :--- | :--- |
| `Series(Y, xvalues=X)` | 第一参数 = 纵轴，第二参数 = 横轴 |
| `axPos` 默认 `'l'` | 须 `x_axis.axPos='b'`、`y_axis.axPos='l'`，否则轴标题与数据对调 |
| `label_col=None` | 仅圆点，**无点旁编号** |
| `title='销售额'` / `scatter.legend` | **不要**；WPS 无法与点旁编号兼得 |
| `showCatName=True` | 散点图无 category 轴，**无效** |
| `DataLabel.txPr` / XML patch | WPS **不显示** |
| `scatter.style = 2` | 部分查看器强制连线，**禁止** |
| Y 含负值未设 `crosses` | 横轴默认在 **y=0** 交叉，刻度「跑到图中间」 |
| 未 `finalize_scatter_chart` | 可能连线、右侧 1～N 图例列表 |

##### ❌ 错误写法（含手写单系列 — 最常见）

```python
# ❌ 未 import chart_labels；单系列 + 图例 → 点旁无编号
xvalues = Reference(ws, min_col=2, min_row=2, max_row=20)
yvalues = Reference(ws, min_col=3, min_row=2, max_row=20)
series = Series(yvalues, xvalues, title='销售额')
scatter.series.append(series)
scatter.legend = Legend()
scatter.legend.position = 'r'
wb.save(OUTPUT)  # ❌ 未 finalize
```

```python
# ❌ 手写循环只设 title — WPS 点旁无文字
for row in range(DATA_START, CHART_DATA_END + 1):
    ...
wb.save(OUTPUT)  # ❌ 未 finalize
```

##### ✅ 标准写法（点旁编号，无图例）

```python
import os, sys
sys.path.insert(0, os.path.join('<技能根目录>', 'scripts'))
from chart_labels import configure_scatter_chart, add_scatter_xy_series, finalize_scatter_chart

scatter = ScatterChart()
scatter.title = '广告投入 vs 销售额'
configure_scatter_chart(scatter)
scatter.x_axis.title = '广告投入 (万元)'
scatter.y_axis.title = '销售额 (万元)'

add_scatter_xy_series(
    scatter, ws,
    x_col=2, y_col=3, min_row=DATA_START, max_row=DATA_END,
    label_col=1,           # A 列 → 点旁 1、2、3… 或公司名
)
# ... TwoCellAnchor 定位 ...
ws._charts.append(scatter)
wb.save(OUTPUT)
finalize_scatter_chart(OUTPUT)  # save 后必须
```

**公司名对标图**（列号按实际表替换）：

```python
scatter.title = 'PE(TTM) vs 营收增速(%) — 估值与成长性对标'
scatter.x_axis.title = 'PE(TTM)'
scatter.y_axis.title = '营收增速(%)'
add_scatter_xy_series(
    scatter, ws, x_col=18, y_col=19, min_row=DATA_START, max_row=DATA_END,
    label_col=17,
)
```

`label_col` 与 `min_row..max_row` 逐行对应。点旁标签走 `showSerName`（每点一个 series），**建议 ≤30 点**。

**无编号**（仅趋势圆点，仍无图例）：`label_col=None` — 少用，多数业务图需要 `label_col`。

**标题约定**：`"A vs B"` → 横轴（X / xVal）= A，纵轴（Y / yVal）= B。

**轴刻度（避免横轴跑到图中间）**：
- Y 含负值（如负增速）时，WPS 默认在 **y=0** 画横轴 → 横轴刻度浮在图中央
- `add_scatter_xy_series` 默认 `auto_scale=True`：按数据留 10% 边距；**数据全为正时 Y 轴不会显示负数**（默认 `y_floor_zero=True`，且边距下界钳在 0）
- 负 Y（如负增速）时自动 `x_axis.crosses='min'`（横轴贴底）
- 需要 Y 轴紧贴数据、不从 0 起：传 `y_floor_zero=False`（仍不会出现负刻度）
- 数据全为正且 X 从 0 起：`x_floor_zero=True`（默认已开）

**散点图自检（写完后必做）**：
- [ ] 已用 `add_scatter_xy_series(..., label_col=...)`，**未**手写只设 `series.title` 的循环
- [ ] 已调用 `configure_scatter_chart` + `finalize_scatter_chart(OUTPUT)`（在 `wb.save` 之后）
- [ ] `x_col` / `y_col` 与轴标题一致；禁止 `scatter.style = 2`
- [ ] 抽 1～2 点验算坐标（如 PE 高应在横轴右侧、增速高应在纵轴上方）
- [ ] WPS 打开：每点圆点 + 点旁标签；**无右侧图例**；无折线、无多余小色块
- [ ] 有负 Y 值时横轴在**图底部**，不在 y=0 中间
- [ ] 图表引用的列（含 `label_col` / `x_col` / `y_col`）**未隐藏**

`chart_labels.py` 通过 `<技能根目录>\scripts` 固定路径加载（与 `recalc` 相同），`python_cell_exec` 环境**始终可用**，禁止绕过 helper 手写 `DataLabelList` 或 `for`+`series.title` 循环。

#### ⚠️ 禁止使用 `add_chart` + `width/height`

不要使用 `ws.add_chart(chart, 'A3')` + `chart.width/height`，该方案的单位是 cm，极易算错导致重叠。**始终使用上述 TwoCellAnchor 网格系统。**

---

# 最佳实践

### 库的选择
- **pandas**：数据分析、批量操作、简单数据导出
- **openpyxl**：复杂格式、公式、Excel 特有功能

### openpyxl 注意事项
- 单元格索引从 1 开始（row=1, column=1 对应 A1）
- `data_only=True` 读取已计算的值；以此模式保存会永久丢失公式
- 大文件：读取用 `read_only=True`，写入用 `write_only=True`

### pandas 注意事项
- 指定数据类型：`pd.read_excel('file.xlsx', dtype={'id': str})`
- 只读取需要的列：`pd.read_excel('file.xlsx', usecols=['A', 'C', 'E'])`
- 日期处理：`pd.read_excel('file.xlsx', parse_dates=['date_column'])`

## 代码风格规范

**Python 脚本**：编写简洁代码，避免多余注释、冗长变量名和不必要的 print。

**Excel 文件本身**：为复杂公式或关键假设添加单元格注释，为硬编码值注明数据来源。

---

# .xls 文件处理（Excel 97-2003）

.xls 格式仅支持**只读**，编辑前必须转换为 .xlsx。转换后原始 `.xls` 文件保留，同时生成新的 `.xlsx` 文件。

## 读取 .xls

```python
import pandas as pd
df = pd.read_excel('data.xls', engine='xlrd')
```

## 编辑 .xls：转换为 .xlsx

```python
import pandas as pd

# 转换（原 xls 不会被修改）
all_sheets = pd.read_excel('data.xls', sheet_name=None, engine='xlrd')
with pd.ExcelWriter('data.xlsx', engine='openpyxl') as writer:
    for name, df in all_sheets.items():
        df.to_excel(writer, sheet_name=name, index=False)

# 后续对 data.xlsx 执行 openpyxl 编辑操作
```

最终用户看到：`data.xls`（原始文件保留）+ `data.xlsx`（编辑后的文件）。

**注意**：转换后格式（字体、颜色、边框、列宽等）会丢失，需在 xlsx 中重新设置。
