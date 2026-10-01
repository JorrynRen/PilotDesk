# Excel 脚本化工作流

创建/编辑表格时按本文执行：按任务编写脚本（如 `report.py`），再用 `python "<技能根目录>\scripts\xlsx_cli.py" build` 执行。公式规范、图表布局、财务模型等见 `<技能根目录>\SKILL.md`。
---

## 何时使用

**脚本化**（写 `<task>.py` + `xlsx_cli.py build`）用于创建/编辑表格。读取、预览表格时直接执行 Python，不走 `xlsx_cli.py build`。

| 场景 | 推荐方式 |
|------|----------|
| 创建 / 编辑表格 | 写 `<task>.py`（如 `report.py`）→ `xlsx_cli.py build` |
| 读取、预览表格 | 直接执行 Python |
| 修 1～2 个公式错误 | 局部修改脚本对应行 + 再次 `xlsx_cli.py build` |
| 修 recalc `array_formula_risk` | 改脚本中公式字符串 + 再次 `xlsx_cli.py build` |

**硬性规则：**

- 创建/编辑走脚本化；不要因「数据行少 / 单 Sheet」就改用 Python 直出交付文件
- 报错后只改 traceback / `error_summary` 指向的最小范围，**禁止整文件重写**
- 连续 3 次同位置失败 → 缩小该函数逻辑，先交付简化版
- 含公式时必须 recalc 至 `status: success`，再执行 audit；按 `summary` / `samples` 处理 warning（修复或确认误报/有意设计），禁止整批无说明放行
- 编辑已有文件保存后顺序：**save → recalc → audit**（含公式时）。**仅替换公式（如 `B+C+D+E` → `SUM`）也必须走完**，禁止 openpyxl 改完即交付
- 脚本调用统一用 execute_command 运行 `python "<技能根目录>\scripts\xlsx_cli.py"`；Python 脚本内定位 scripts 目录用 `<技能根目录>\scripts` 固定路径（技能根目录取自 load_skill 返回内容顶部），不使用环境变量

---

## 操作步骤

```
规划结构
    │
    ▼
识别数据表（Excel Table → 结构化引用）
    │
    ▼
写入 report.py（按任务命名）骨架
    │
    ▼
分块追加（数据 / 公式 / 格式 / 图表）
    │
    ▼
python "<技能根目录>\scripts\xlsx_cli.py" build report.py
    │
    ├─ 成功 + recalc success
    │         │
    │         ▼
    │   xlsx_cli.py audit ──► 无未处理 warning ──► 交付 .xlsx
    │         │
    │         └─ warnings_found ──► 修复或确认可保留 ──► 再 build / 再 audit
    │
    └─ 失败 ──► 读 traceback / error_summary
                    │
                    ▼
               局部修改脚本
                    │
                    └──► 再次 xlsx_cli.py build（不整文件重写）
```

### 分阶段写入顺序

1. **骨架**：import、`OUTPUT`、`main()`、recalc 钩子
2. **数据**：`build_data_sheets`（可先无公式，验证结构）
3. **公式**：`build_summary_sheet`（先 `print(list(ws.tables.keys()))`；有 Table 则 SUM/SUMIF/COUNTIF 等用结构化引用；先 2～3 个单元格试写，再批量）
4. **格式**：`apply_formatting`
5. **图表**：`add_charts`（最后加，避免 `insert_rows` 影响坐标）

每阶段执行一次 `xlsx_cli.py build`，失败时范围明确。长脚本宜分块追加（单次不宜过大），避免一次写完整大段后难定位。

---

## 脚本结构

按任务命名脚本（如 `report.py`、`sales_summary.py`），骨架如下：

```python
#!/usr/bin/env python3
"""生成 <output>.xlsx — <任务简述>"""
import os
import sys

sys.path.insert(0, os.path.join("<技能根目录>", "scripts"))
from recalc import recalc

OUTPUT = "report.xlsx"


def build_data_sheets(wb):
    """Sheet 1：原始数据"""
    ...


def build_summary_sheet(wb):
    """Sheet 2：汇总 + 公式"""
    ...


def apply_formatting(wb):
    """字体、列宽、数字格式"""
    ...


def add_charts(wb):
    """TwoCellAnchor 图表（见 SKILL.md 图表章节）"""
    ...


def main():
    from openpyxl import Workbook  # 编辑已有文件时改用 load_workbook

    wb = Workbook()
    build_data_sheets(wb)
    build_summary_sheet(wb)
    apply_formatting(wb)
    add_charts(wb)
    wb.save(OUTPUT)


if __name__ == "__main__":
    main()
    result = recalc(OUTPUT)
    print(result)
    if result.get("status") != "success":
        raise SystemExit(1)
```

要点：

- 脚本文件名按任务起名，**不要**用固定 `build_` 前缀（与 `xlsx_cli.py build` 重复）
- 按 **函数** 拆分：`build_data_sheets` / `build_summary_sheet` / `apply_formatting` / `add_charts`
- `OUTPUT` 常量化，路径不散落
- `__main__` 中 `main()` → `recalc(OUTPUT)` → 非 success 时 `SystemExit(1)`

超大任务可拆为多文件目录（`data.py`、`summary.py`、`charts.py`、`main.py`），单模块出错只改对应文件。

---

## 执行命令

```bash
# 执行构建脚本
python "<技能根目录>\scripts\xlsx_cli.py" build report.py

# 仅校验已有 xlsx 公式
python "<技能根目录>\scripts\xlsx_cli.py" recalc report.xlsx --timeout 60

# 公式结构审计（含公式时必须）
python "<技能根目录>\scripts\xlsx_cli.py" audit report.xlsx
```

---

## 错误修复

### Python 异常

根据 traceback 中的 **文件名 + 行号 + 函数名**，只改对应函数。禁止重写整个脚本。

### recalc 公式错误

根据 `error_summary` 中的 `locations`（如 `汇总!D6`），在脚本中定位写入该单元格的公式并修改，然后再次 `xlsx_cli.py build`。

`array_formula_risk` 常见修复：将 `MEDIAN(IF(range<>"", range))` 改为 `MEDIAN(range)` 或 `AVERAGEIF` / `AGGREGATE`。

### audit 结构 warning

先读 `summary`（各 type 原始命中数），再处理 `warnings`（已按类型聚合，含 `count` / `samples`）。关注 `range_gap`、`hardcoded_assumption`、`possible_overwrite`、`fixed_range_on_table`、`structured_ref_inside_table` 等：能修则改脚本后再次 `xlsx_cli.py build` 与 `xlsx_cli.py audit`；确认属误报或有意设计可保留并在交付说明中注明。禁止因 `raw_warning_count` 大就整批放行。

`structured_ref_inside_table`：把合计行缩进 Table 外，或改用 Totals Row；勿在表内写 `SUM(Table[列])`。

敏感性矩阵：轴写在标签行/列，公式引用轴单元格（如 `=B$1`），避免在每个格子写死步长常量。

### 修复原则

- 已跑通的阶段函数不要动
- 修完必须再次 `xlsx_cli.py build` 全脚本
- 一切修改回写脚本，保证可复现；不要只改 `.xlsx` 不改脚本

---

## 命名约定

| 类型 | 约定 |
|------|------|
| 构建脚本 | 按任务命名，如 `report.py`、`sales_summary.py`（中间产物，可保留在工作区） |
| 交付物 | `.xlsx` |

---

## 禁止事项

- 禁止因「数据行少 / 单 Sheet」就跳过脚本化、用 Python 直出交付文件（读取/预览除外）
- 禁止报错后整段重写脚本
- 禁止跳过 recalc 直接交付含公式的文件
- 禁止跳过 audit 直接交付含公式的文件
- 禁止用 `python -c` 传入复杂多行代码（应写入 `.py` 后用 `xlsx_cli.py build` 执行）
- 禁止 `find` / `ls` / `echo` 探测 skill 落盘目录；直接使用 `<技能根目录>` 固定路径
