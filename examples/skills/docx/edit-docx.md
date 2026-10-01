# 编辑 DOCX 文档

> 完整 API（参数、返回值、正则示例、低层 XML 编辑）见 `references/edit-docx-api.md`。

## 文件隔离

编辑用户提供的 DOCX 时，必须先在当前工作区创建编辑目录，并复制一份编辑副本；后续 `edit` / `unpack` / `pack` 都只操作这个副本或编辑目录内的输出文件，禁止直接修改用户原始文件。

```bash
mkdir -p output/docx_edit
cp "/path/to/input.docx" "output/docx_edit/input_edit.docx"
```

下文中的 `input.docx` 均指编辑副本；最终输出也写入 `output/docx_edit/`。

通过 **execute_command** 执行文本替换（内部自动：解包 → 合并同样式 run → 替换 → 重打包 → 校验）：

```bash
python "<技能根目录>\scripts\docx_cli.py" edit output/docx_edit/input_edit.docx -o output/docx_edit/output.docx \
  --replace "旧公司名称=新公司名称" \
  --replace "2024年=2025年"
```

**替换命中数为 0？** 通常是目标文本被 Word 拆成了多个 XML run，退回低层 `unpack` / `pack` 手动处理，详见 `references/edit-docx-api.md` "低层 XML 编辑"章节。

## 编辑规则

- `edit_docx()` 只做文本替换，不负责批注、修订和复杂域代码
- 解包时自动合并相邻同样式 run，提高命中率
- `pack_docx()` 自动修复常见的 XML 空白属性和 `durableId` 溢出问题

---

## Troubleshooting

| 问题 | 处理方式 |
|------|----------|
| `replacements_applied` 中某项 `count` 为 0 | 先 `python "<技能根目录>\scripts\docx_cli.py" unpack output/docx_edit/input_edit.docx -o output/docx_edit/input_unpacked` 检查目标文本是否被拆成多个 XML run |
| `DOCX 验证失败` | 检查修改过的 XML 文件是否有语法错误 |
