"""
XLSX Skill - Excel 公式重算、结构审计与脚本化构建

CLI：
    bash "$SKILL_PATH_BASH/xlsx/scripts/xlsx.sh" build report.py
    bash "$SKILL_PATH_BASH/xlsx/scripts/xlsx.sh" recalc report.xlsx
    bash "$SKILL_PATH_BASH/xlsx/scripts/xlsx.sh" audit report.xlsx

Python 模块：
    from recalc import recalc
    from audit import audit_formulas
    result = recalc("report.xlsx", timeout=60)
    audit = audit_formulas("report.xlsx")
"""

from .recalc import recalc
from .audit import audit_formulas

__all__ = [
    "recalc",
    "audit_formulas",
]
