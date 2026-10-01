#!/usr/bin/env python3
"""
技能快速校验脚本 - 轻量版本
"""

import re
import sys
from pathlib import Path
from typing import Optional

try:
    import yaml
except ModuleNotFoundError:
    yaml = None

MAX_SKILL_NAME_LENGTH = 64
_KNOWN_VARIANTS = {"local", "sandbox"}


def _extract_frontmatter(content: str) -> Optional[str]:
    lines = content.splitlines()
    if not lines or lines[0].strip() != "---":
        return None
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return "\n".join(lines[1:i])
    return None


def _parse_simple_frontmatter(frontmatter_text: str) -> Optional[dict[str, str]]:
    """
    当 PyYAML 不可用时使用的最小兜底解析器。
    只支持 `SKILL.md` frontmatter 中使用的简单 `key: value` 映射。
    """
    parsed: dict[str, str] = {}
    current_key: Optional[str] = None
    for raw_line in frontmatter_text.splitlines():
        stripped = raw_line.strip()
        if not stripped or stripped.startswith("#"):
            continue

        is_indented = raw_line[:1].isspace()
        if is_indented:
            if current_key is None:
                return None
            current_value = parsed[current_key]
            parsed[current_key] = (
                f"{current_value}\n{stripped}" if current_value else stripped
            )
            continue

        if ":" not in stripped:
            return None
        key, value = stripped.split(":", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            return None
        if (value.startswith('"') and value.endswith('"')) or (
            value.startswith("'") and value.endswith("'")
        ):
            value = value[1:-1]
        parsed[key] = value
        current_key = key
    return parsed


def validate_skill(skill_path):
    """执行技能的基础校验。"""
    skill_path = Path(skill_path)

    skill_md = skill_path / "SKILL.md"
    if not skill_md.exists():
        return False, "未找到 SKILL.md"

    try:
        content = skill_md.read_text(encoding="utf-8")
    except OSError as e:
        return False, f"无法读取 SKILL.md：{e}"

    frontmatter_text = _extract_frontmatter(content)
    if frontmatter_text is None:
        return False, "frontmatter 格式无效"
    if yaml is not None:
        try:
            frontmatter = yaml.safe_load(frontmatter_text)
            if not isinstance(frontmatter, dict):
                return False, "frontmatter 必须是 YAML 字典"
        except yaml.YAMLError as e:
            return False, f"frontmatter 中的 YAML 无效：{e}"
    else:
        frontmatter = _parse_simple_frontmatter(frontmatter_text)
        if frontmatter is None:
            return (
                False,
                "frontmatter 中的 YAML 无效：在未安装 PyYAML 时存在不受支持的语法",
            )

    allowed_properties = {"name", "description", "license", "allowed-tools", "metadata"}

    unexpected_keys = set(frontmatter.keys()) - allowed_properties
    if unexpected_keys:
        allowed = ", ".join(sorted(allowed_properties))
        unexpected = ", ".join(sorted(unexpected_keys))
        return (
            False,
            f"SKILL.md frontmatter 中存在未预期字段：{unexpected}。允许的字段有：{allowed}",
        )

    if "name" not in frontmatter:
        return False, "frontmatter 中缺少 'name'"
    if "description" not in frontmatter:
        return False, "frontmatter 中缺少 'description'"

    name = frontmatter.get("name", "")
    if not isinstance(name, str):
        return False, f"name 必须是字符串，当前类型为 {type(name).__name__}"
    name = name.strip()
    if name:
        if not re.match(r"^[a-z0-9-]+$", name):
            return (
                False,
                f"name '{name}' 应为连字符格式（仅允许小写字母、数字和连字符）",
            )
        if name.startswith("-") or name.endswith("-") or "--" in name:
            return (
                False,
                f"name '{name}' 不能以连字符开头/结尾，也不能包含连续连字符",
            )
        if len(name) > MAX_SKILL_NAME_LENGTH:
            return (
                False,
                f"name 过长（{len(name)} 个字符）。最大长度为 {MAX_SKILL_NAME_LENGTH} 个字符。",
            )

        dir_name = skill_path.name
        base_name = dir_name.rsplit(".", 1)[0] if "." in dir_name and dir_name.rsplit(".", 1)[1] in _KNOWN_VARIANTS else dir_name
        if base_name != name:
            return (
                False,
                f"目录名 '{base_name}' 与 frontmatter name '{name}' 不一致，两者应保持相同",
            )

    description = frontmatter.get("description", "")
    if not isinstance(description, str):
        return False, f"description 必须是字符串，当前类型为 {type(description).__name__}"
    description = description.strip()
    if description:
        if "<" in description or ">" in description:
            return False, "description 不能包含尖括号（< 或 >）"
        if len(description) > 1024:
            return (
                False,
                f"description 过长（{len(description)} 个字符）。最大长度为 1024 个字符。",
            )

    return True, "技能有效！"


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("用法：python quick_validate.py <skill_directory>")
        sys.exit(1)

    valid, message = validate_skill(sys.argv[1])
    print(message)
    sys.exit(0 if valid else 1)
