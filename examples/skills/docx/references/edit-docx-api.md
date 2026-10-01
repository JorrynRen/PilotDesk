# edit_docx() API 参考

通过 `python "<技能根目录>\scripts\docx_cli.py" edit` 对现有 `.docx` 做文本替换。内部流程：解包 → 合并相邻同样式 run → 文本替换 → 重打包 → 校验。

## 基本用法

编辑用户原始文件前，先复制编辑副本，后续 input/output 都使用编辑目录内文件：

```bash
mkdir -p output/docx_edit
cp "/path/to/input.docx" "output/docx_edit/input_edit.docx"
```

```bash
python "<技能根目录>\scripts\docx_cli.py" edit output/docx_edit/input_edit.docx -o output/docx_edit/output.docx \
  --replace "旧公司名称=新公司名称" \
  --replace "2024年=2025年"
```

## 参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `input_docx` | `str` | — | 输入文件完整路径 |
| `output_docx` | `str` | — | 输出文件完整路径 |
| `replacements` | `list[dict]` | — | 替换规则列表，每项含 `"from"` 和 `"to"` |
| `parts` | `list[str] \| None` | `None` | 要处理的 XML 部件（支持 glob），默认 `document.xml` + `header*.xml` + `footer*.xml` |
| `match_mode` | `str` | `"literal"` | `"literal"` 精确匹配 或 `"regex"` 正则匹配 |
| `ignore_case` | `bool` | `False` | 是否忽略大小写 |

## 返回值

```python
{
    "ok": True,
    "input_path": "...",
    "output_path": "...",
    "match_mode": "literal",
    "ignore_case": False,
    "files_modified": ["word/document.xml"],
    "replacements_applied": [
        {"from": "旧公司名称", "to": "新公司名称", "count": 3},
    ],
    "validation": {"ok": True, "errors": [], "warnings": []},
}
```

## 正则替换示例

```bash
python "<技能根目录>\scripts\docx_cli.py" edit output/docx_edit/input_edit.docx -o output/docx_edit/output.docx \
  --replace "Q([1-4]) 2024=Q\1 2025" --regex --ignore-case
```

## 低层 XML 编辑

如果 `edit` 替换命中数为 0，通常是目标文本被 Word 拆成了多个 XML run。可退回低层工具链手动处理：

```bash
# 解包（合并相邻同样式 run，提高命中率）
python "<技能根目录>\scripts\docx_cli.py" unpack output/docx_edit/input_edit.docx -o output/docx_edit/input_unpacked

# 用 file(read) / file(edit) 工具直接修改 XML 文件
# 重打包（自动校验，失败会报错）
python "<技能根目录>\scripts\docx_cli.py" pack output/docx_edit/input_unpacked -o output/docx_edit/output.docx
```

重点文件：

- `word/document.xml`：正文
- `word/header*.xml`：页眉
- `word/footer*.xml`：页脚
- `word/styles.xml`：样式

## 规则

- `edit_docx()` 是基础文本替换工具，不负责批注、修订和复杂域代码
- 解包时会合并相邻的同样式 run，提高文本替换命中率
- 对于被 Word 拆成复杂片段的文本，退回 `unpack_docx()` 手动处理
- `pack_docx()` 会自动修复常见的 XML 空白属性和 `durableId` 溢出问题
