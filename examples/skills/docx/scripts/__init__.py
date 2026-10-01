"""
DOCX Skill - Word 文档创建与编辑工具集

子模块说明：
  run_code_docx - run_node_docx / restore_backup：JS → DOCX 执行入口（自动修复、预检、后处理）
  edit_docx     - edit_docx：对 .docx 文件执行结构化编辑操作
  office        - pack_docx / unpack_docx / validate_docx：底层 Office XML 打包工具

使用方式：
    import sys
    sys.path.insert(0, os.path.join(os.getenv('SKILL_PATH'), 'docx', 'scripts'))
    from run_code_docx import run_node_docx
    from edit_docx import edit_docx
"""

from .edit_docx import edit_docx
from .run_code_docx import restore_backup, run_node_docx
from .office import pack_docx, unpack_docx, validate_docx

__all__ = [
    # JS → DOCX 执行
    "run_node_docx",
    "restore_backup",
    # 编辑
    "edit_docx",
    # Office XML 工具
    "pack_docx",
    "unpack_docx",
    "validate_docx",
]
