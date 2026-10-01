from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import json
from pathlib import Path


# ── Environment ───────────────────────────────────────────────────

_npm_root_cache: str | None = None
_bundled_node: tuple[str, str] | None = None  # (node_exe, npm_cli_js)
_node_exe_cache: str | None = None


def _find_bundled_node() -> tuple[str, str] | None:
    """从环境变量获取 WPS 灵犀自带的 node 和 npm-cli.js 路径。

    依赖客户端注入的环境变量：
      - NODE_BIN_DIR: node 可执行文件所在目录
      - NPM_CLI_PATH: npm-cli.js 完整路径

    返回 (node_path, npm_cli_js_path) 或 None。
    结果缓存到 _bundled_node。
    """
    global _bundled_node
    if _bundled_node is not None:
        return _bundled_node

    node_bin_dir = os.environ.get("NODE_BIN_DIR", "").strip()
    npm_cli_path = os.environ.get("NPM_CLI_PATH", "").strip()

    if not node_bin_dir or not npm_cli_path:
        return None

    node_path = os.path.join(node_bin_dir, "node.exe")
    if not os.path.isfile(node_path):
        node_path = os.path.join(node_bin_dir, "node")
    if not os.path.isfile(node_path):
        return None
    if not os.path.isfile(npm_cli_path):
        return None

    _bundled_node = (node_path, npm_cli_path)
    return _bundled_node


def _get_node_exe() -> str:
    """返回 node 可执行文件路径，优先使用 bundled node，否则回退到系统 PATH 中的 node。

    结果缓存到 _node_exe_cache。
    """
    global _node_exe_cache
    if _node_exe_cache is not None:
        return _node_exe_cache

    bundled = _find_bundled_node()
    if bundled:
        _node_exe_cache = bundled[0]
    else:
        _node_exe_cache = "node"
    return _node_exe_cache


def _global_npm_root() -> str:
    """返回 node_modules 路径，结果缓存。

    探测优先级：
      1) NODE_BIN_DIR 环境变量 → NODE_BIN_DIR/node_modules（桌面端）
      2) 系统全局 npm → npm root -g 返回的路径（Web 端）
      3) 脚本自身目录下的 node_modules（兜底）
    会尽量在探测阶段创建目录，避免 npm root -g 返回路径存在但目录被清理时误判到兜底路径。
    """
    global _npm_root_cache
    if _npm_root_cache is not None:
        return _npm_root_cache

    node_bin_dir = os.environ.get("NODE_BIN_DIR", "").strip()
    if node_bin_dir:
        root = os.path.join(node_bin_dir, "node_modules")
        _npm_root_cache = root
        return root

    npm_exe = shutil.which("npm")
    if npm_exe:
        try:
            result = subprocess.run(
                [npm_exe, "root", "-g"],
                capture_output=True, text=True, timeout=10,
            )
            root = (result.stdout or "").strip()
            if result.returncode == 0 and root:
                try:
                    os.makedirs(root, exist_ok=True)
                except OSError:
                    # 目录不可创建（权限等）时继续走兜底路径。
                    pass
                if os.path.isdir(root):
                    _npm_root_cache = root
                    return root
        except (OSError, subprocess.TimeoutExpired):
            pass

    fallback = os.path.join(os.path.dirname(os.path.abspath(__file__)), "node_modules")
    _npm_root_cache = fallback
    return fallback


# 生成 DOCX 必需的核心 npm 模块（缺一个都跑不通）：
#   - docx           : docx-helper.js / formula.js 的底层依赖
#   - jszip          : docx_patches.js 解包/重打包 .docx zip 包
#   - @xmldom/xmldom : docx_patches.js 解析/序列化 XML
_CORE_NPM_DEPS = ("docx", "jszip", "@xmldom/xmldom")
_core_deps_checked = False


def _ensure_core_npm_modules() -> None:
    """启动时预检核心 npm 模块，缺失时自动安装一次再兜底报错。

    行为:
      1) 检测 docx/jszip/@xmldom/xmldom 是否存在
      2) 若缺失，优先用环境变量提供的 node+npm-cli 安装，否则尝试系统 npm
      3) 安装后复检；仍缺失则抛 RuntimeError 并附带手动安装命令

    可通过环境变量关闭自动安装:
      - DOCX_AUTO_INSTALL_CORE_DEPS=0 / false / no
    """
    global _core_deps_checked
    if _core_deps_checked:
        return

    npm_root = _global_npm_root()
    missing = [m for m in _CORE_NPM_DEPS if not os.path.isdir(os.path.join(npm_root, m))]

    if missing:
        auto_install_flag = os.environ.get("DOCX_AUTO_INSTALL_CORE_DEPS", "1").strip().lower()
        auto_install_enabled = auto_install_flag not in {"0", "false", "no"}

        install_stdout = ""
        install_stderr = ""
        install_return_code: int | None = None

        if auto_install_enabled:
            os.makedirs(npm_root, exist_ok=True)
            prefix_dir = os.path.dirname(npm_root)
            bundled = _find_bundled_node()

            if bundled:
                node_exe, npm_cli = bundled
                install_cmd_args = [node_exe, npm_cli, "install", "--prefix", prefix_dir, *missing]
                install_method = f"bundled node ({node_exe})"
            else:
                npm_exe = shutil.which("npm") or "npm"
                install_cmd_args = [npm_exe, "install", "--prefix", prefix_dir, *missing]
                install_method = f"system npm ({npm_exe})"

            print(f"[deps] 核心模块缺失: {', '.join(missing)}，通过 {install_method} 自动安装到 {npm_root}")

            try:
                install_result = subprocess.run(
                    install_cmd_args,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
                install_return_code = install_result.returncode
                install_stdout = (install_result.stdout or "").strip()
                install_stderr = (install_result.stderr or "").strip()
            except OSError as e:
                install_return_code = -1
                install_stderr = str(e)

            # 自动安装后复检（无论 install return code 如何，都以实际存在性为准）
            missing = [m for m in _CORE_NPM_DEPS if not os.path.isdir(os.path.join(npm_root, m))]
            if not missing:
                print("[deps] 自动安装成功")
            else:
                print(f"[deps] 自动安装后仍缺失: {', '.join(missing)} (returncode={install_return_code})")

        if missing:
            details = ""
            if auto_install_enabled:
                details += f"  自动安装返回码: {install_return_code}\n"
                if install_stderr:
                    details += (
                        "  自动安装 stderr（末尾）:\n"
                        + "\n".join(f"      {ln}" for ln in install_stderr.splitlines()[-8:])
                        + "\n"
                    )
                elif install_stdout:
                    details += (
                        "  自动安装输出（末尾）:\n"
                        + "\n".join(f"      {ln}" for ln in install_stdout.splitlines()[-8:])
                        + "\n"
                    )
            else:
                details += "  自动安装已禁用（DOCX_AUTO_INSTALL_CORE_DEPS=0）\n"

            prefix_dir = os.path.dirname(npm_root)
            node_bin_dir = os.environ.get("NODE_BIN_DIR", "")
            npm_cli_path = os.environ.get("NPM_CLI_PATH", "")

            if node_bin_dir and npm_cli_path:
                fix_cmd = f"<NODE_BIN_DIR>/node <NPM_CLI_PATH> install --prefix {prefix_dir} {' '.join(missing)}"
            else:
                fix_cmd = f"npm install --prefix {prefix_dir} {' '.join(missing)}"

            used_bundled = auto_install_enabled and bundled is not None
            hint_items = []
            if install_return_code == -1:
                hint_items.append("returncode=-1 → node 或 npm 可执行文件不存在，未安装到系统 PATH")
            if used_bundled:
                hint_items.append(
                    "使用了客户端内置 node → 检查 NODE_BIN_DIR / NPM_CLI_PATH 指向的文件是否完整可用"
                )
            elif auto_install_enabled:
                if shutil.which("npm"):
                    hint_items.append("使用了系统 npm 但安装失败 → 检查网络连接或目标目录写入权限")
                else:
                    hint_items.append(
                        "未检测到可用的 npm → "
                        "桌面端确认客户端已注入 NODE_BIN_DIR / NPM_CLI_PATH；"
                        "Web 端确保 npm 在系统 PATH 中"
                    )
            if install_return_code is not None and install_return_code > 0:
                hint_items.append("npm install 返回非零 → 检查网络连接，或手动执行上方安装命令")
            if install_return_code == 0 and missing:
                hint_items.append("npm 返回成功但模块仍缺失 → 检查目标目录写入权限")
            hint = ""
            if hint_items:
                hint = "\n\n  排查方向:\n" + "\n".join(f"  - {h}" for h in hint_items)

            raise RuntimeError(
                f"npm 模块缺失: {', '.join(missing)}\n"
                f"{details}"
                f"  手动修复: {fix_cmd}\n"
                f"  安装目录: {npm_root}\n"
                f"{hint}"
            )

    _core_deps_checked = True


def _with_global_node_path(env: dict[str, str] | None = None) -> dict[str, str]:
    merged = dict(env or os.environ)
    global_root = _global_npm_root()
    existing = merged.get("NODE_PATH", "").strip()
    merged["NODE_PATH"] = (
        f"{global_root}{os.pathsep}{existing}" if existing else global_root
    )
    return merged


def _extract_missing_modules(error_text: str) -> list[str]:
    """Extract module names from Node's "Cannot find module 'xxx'" errors."""
    modules = [
        m.group(1).strip()
        for m in re.finditer(r"""Cannot find module ['"]([^'"]+)['"]""", error_text or "")
    ]
    return sorted(set(m for m in modules if m))


def _missing_modules_hint(missing_modules: list[str], env: dict[str, str]) -> str:
    """Build guidance text for missing Node modules."""
    modules = sorted(set(m for m in missing_modules if m))
    install_root = env.get("NODE_PATH", "").split(os.pathsep)[0].strip() or _global_npm_root()
    prefix_dir = os.path.dirname(install_root)
    missing = ", ".join(f"`{m}`" for m in modules) if modules else "unknown"
    install_cmd = " ".join(modules) if modules else "<module>"
    return (
        f"模块未安装: {missing}。"
        f" 执行 `npm install --prefix {prefix_dir} {install_cmd}`"
    )


def _backup_path(script: Path) -> Path:
    return script.with_suffix(".js.bak")


# ── Error formatting ─────────────────────────────────────────────

def _extract_error_context(script: Path, stderr: str) -> str:
    """Parse Node.js error output and attach source context around the failing line."""
    lines = script.read_text(encoding="utf-8").splitlines()

    match = re.search(re.escape(str(script)) + r":(\d+)", stderr)
    if not match:
        match = re.search(re.escape(script.name) + r":(\d+)", stderr)
    if not match:
        return stderr

    error_line = int(match.group(1))

    # Extract just the error type + message (e.g. "SyntaxError: Unexpected token ')'")
    err_msg_match = re.search(r"((?:Syntax|Reference|Type|Range)Error:.+?)(?:\n|$)", stderr)
    err_msg = err_msg_match.group(1).strip() if err_msg_match else stderr.splitlines()[-1].strip()

    start = max(0, error_line - 3)
    end = min(len(lines), error_line + 2)

    def _trunc(s: str, limit: int = 120) -> str:
        return s[:limit] + "..." if len(s) > limit else s

    ctx = []
    for i in range(start, end):
        marker = " >>>" if i == error_line - 1 else "    "
        ctx.append(f"{marker} {i + 1:4d} | {_trunc(lines[i])}")

    return f"{script.name}:{error_line} — {err_msg}\n" + "\n".join(ctx)


# ── Auto-fix (silent corrections before any checks) ──────────────

_UNICODE_REPLACEMENTS: list[tuple[str, str, str]] = [
    ("\u201c", '"', "左双引号"),   # "
    ("\u201d", '"', "右双引号"),   # "
    ("\u2018", "'", "左单引号"),   # '
    ("\u2019", "'", "右单引号"),   # '
    ("\uff08", "(", "全角左括号"), # （
    ("\uff09", ")", "全角右括号"), # ）
    ("\uff1a", ":", "全角冒号"),   # ：
    ("\uff0c", ",", "全角逗号"),   # ，
    ("\uff1b", ";", "全角分号"),   # ；
    ("\uff3b", "[", "全角左方括号"), # ［
    ("\uff3d", "]", "全角右方括号"), # ］
    ("\uff5b", "{", "全角左花括号"), # ｛
    ("\uff5d", "}", "全角右花括号"), # ｝
]


# Characters after which a `/` starts a JS regex literal (expression-start
# context). Conservative subset — keyword-based contexts (return /x/, typeof /x/)
# are not covered, which is a documented false-negative.
_REGEX_PREV_SET = frozenset("({[,=;:?!|&+~^*<>%")


def _is_regex_start_context(prev: str) -> bool:
    return not prev or prev in _REGEX_PREV_SET


def _skip_js_regex_literal(source: str, start: int) -> int | None:
    r"""If ``source[start:]`` opens a JS regex literal, return index after it
    (including any trailing flags); else ``None``.

    Recognises char-class brackets and backslash escapes so ``/[/]/`` and
    ``/\//`` are tracked correctly. Returns ``None`` if the regex would
    span a newline (regex literals cannot wrap lines in JS).
    """
    n = len(source)
    if start >= n or source[start] != "/":
        return None
    j = start + 1
    in_class = False
    while j < n:
        ch = source[j]
        if ch == "\n":
            return None
        if ch == "\\" and j + 1 < n:
            j += 2
            continue
        if ch == "[":
            in_class = True
        elif ch == "]" and in_class:
            in_class = False
        elif ch == "/" and not in_class:
            j += 1
            while j < n and source[j] in "dgimsuyv":
                j += 1
            return j
        j += 1
    return None


def _find_string_ranges(source: str) -> list[tuple[int, int]]:
    """Return [(start, end), ...] character-index ranges of JS string literals.

    Recognises single-quoted, double-quoted, and template-literal strings
    with backslash escape handling.  Skips ``//`` / ``/* */`` comments and
    JS regex literals (``/.../flags``) so quotes inside ``.replace(/\\/g, '/')``
    are not misread as string delimiters.
    """
    ranges: list[tuple[int, int]] = []
    i = 0
    n = len(source)
    last_significant = ""
    while i < n:
        ch = source[i]
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            j = source.find("\n", i)
            i = j + 1 if j != -1 else n
            last_significant = ""
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            j = source.find("*/", i + 2)
            i = j + 2 if j != -1 else n
            continue
        if ch == "/" and _is_regex_start_context(last_significant):
            j = _skip_js_regex_literal(source, i)
            if j is not None:
                i = j
                last_significant = "/"
                continue
        if ch in ("'", '"', "`"):
            start = i
            i += 1
            while i < n:
                if source[i] == "\\" and i + 1 < n:
                    i += 2
                    continue
                if source[i] == ch:
                    i += 1
                    break
                i += 1
            ranges.append((start, i))
            last_significant = ch
            continue
        if ch == "\n":
            last_significant = ""
        elif not ch.isspace():
            last_significant = ch
        i += 1
    return ranges


def _replace_outside_strings(
    source: str,
    bad_char: str,
    good_char: str,
    string_ranges: list[tuple[int, int]],
) -> tuple[str, int]:
    """Replace *bad_char* → *good_char* only in code regions (outside string literals).

    All replacements are single-char → single-char so positions stay stable.
    Returns ``(new_source, replacement_count)``.
    """
    parts: list[str] = []
    count = 0
    last = 0
    for start, end in string_ranges:
        seg = source[last:start]
        c = seg.count(bad_char)
        if c:
            seg = seg.replace(bad_char, good_char)
            count += c
        parts.append(seg)
        parts.append(source[start:end])
        last = end
    seg = source[last:]
    c = seg.count(bad_char)
    if c:
        seg = seg.replace(bad_char, good_char)
        count += c
    parts.append(seg)
    return "".join(parts), count


_TS_ASSERTION_RE = re.compile(
    r"\s+as\s+(?:any|unknown|string|number|boolean|object|never|const)\b"
)


def _strip_ts_assertions(source: str) -> tuple[str, list[str]]:
    """Strip common TypeScript assertions from generated JS.

    LLM output occasionally leaks TS-only syntax like:
      verticalAlign: 'center' as any,
    which is invalid in plain Node.js execution.
    """
    lines = source.splitlines()
    fixes: list[str] = []
    changed = False

    for idx, line in enumerate(lines):
        if " as " not in line:
            continue

        ranges = _find_string_ranges(line)
        parts: list[str] = []
        last = 0
        count = 0

        for m in _TS_ASSERTION_RE.finditer(line):
            if any(start <= m.start() < end for start, end in ranges):
                continue
            parts.append(line[last:m.start()])
            last = m.end()
            count += 1

        if not count:
            continue

        parts.append(line[last:])
        lines[idx] = "".join(parts)
        fixes.append(f"第 {idx + 1} 行：移除 TypeScript 断言（{count} 处）")
        changed = True

    if not changed:
        return source, []

    suffix = "\n" if source.endswith("\n") else ""
    return "\n".join(lines) + suffix, fixes


def _index_in_ranges(index: int, ranges: list[tuple[int, int]]) -> bool:
    """Return True if *index* falls inside any ``(start, end)`` string range."""
    return any(start <= index < end for start, end in ranges)


def _fix_broken_arrow_suffix(line: str) -> str:
    """Strip stray closing tokens after an arrow at end-of-line.

    LLM output sometimes produces:
      ``cols.map((c, i) => } }``
    when it meant a multi-line arrow body:
      ``cols.map((c, i) =>``

    We only touch cases where everything after ``=>`` is whitespace plus
    closing tokens. Legitimate inline bodies like ``x => ({ a: 1 })`` are
    left untouched because they contain real code after the arrow.
    """
    match = re.search(r"=>\s*(?:[}\]),;]+(?:\s+[}\]),;]+)*)\s*$", line)
    if not match:
        return line

    if _index_in_ranges(match.start(), _find_string_ranges(line)):
        return line

    trailing_ws = re.search(r"\s*$", line)
    suffix = trailing_ws.group(0) if trailing_ws else ""
    return line[: match.start()] + "=>" + suffix


_FUNC_NO_SPACE_RE = re.compile(r"\bfunction([^\x00-\x7F\s(])")


def _fix_function_keyword_no_space(line: str) -> str:
    """Insert space between ``function`` keyword and CJK identifier.

    LLM sometimes writes ``function谈话记录()`` which is parsed as
    calling a function named ``function谈话记录`` instead of declaring one.
    """
    return _FUNC_NO_SPACE_RE.sub(r"function \1", line)


def _fix_latex_prime_in_sq(line: str) -> str:
    r"""Escape ``'`` inside LaTeX ``$...$`` blocks to prevent JS string breakage.

    LaTeX prime notation (``s'``, ``f'(x)``, ``_{j'}``) uses ``'`` which
    conflicts with JS single-quoted string delimiters::

        h.p('...$P(s'|s,a)$...')    // SyntaxError
        h.p('...$P(s\'|s,a)$...')   // Fixed — \' is valid inside '...'

    Only modifies ``'`` that appear *between* matched ``$…$`` pairs and
    are not already escaped.

    IMPORTANT:
      This fixer must stay scoped to a *single JS single-quoted literal*.
      A previous regex implementation scanned the whole line and could match
      across adjacent literals, e.g.:

        '...$x$)', 'next $y$'

      That falsely escaped the real string separators (``', '``) into
      ``\', \'`` and corrupted valid code.
    """
    if "$" not in line or "'" not in line:
        return line

    out: list[str] = []
    i = 0
    n = len(line)
    changed = False

    while i < n:
        ch = line[i]

        # Skip double-quoted / template strings intact.
        if ch in ('"', "`"):
            start = i
            i += 1
            while i < n:
                if line[i] == "\\" and i + 1 < n:
                    i += 2
                    continue
                if line[i] == ch:
                    i += 1
                    break
                i += 1
            out.append(line[start:i])
            continue

        # Skip comments at top-level.
        if ch == "/" and i + 1 < n and line[i + 1] == "/":
            out.append(line[i:])
            break

        if ch != "'":
            out.append(ch)
            i += 1
            continue

        # Enter single-quoted literal and only operate inside this literal.
        out.append("'")
        i += 1
        in_math = False

        while i < n:
            cur = line[i]

            # Preserve existing escapes.
            if cur == "\\" and i + 1 < n:
                out.append(line[i:i + 2])
                i += 2
                continue

            # Toggle inline math mode on unescaped `$`.
            if cur == "$":
                in_math = not in_math
                out.append(cur)
                i += 1
                continue

            # Prime inside `$...$` within single-quoted JS string:
            # escape it instead of closing the JS literal.
            if cur == "'" and in_math:
                out.append("\\'")
                changed = True
                i += 1
                continue

            # Normal single-quote closes the JS literal.
            if cur == "'":
                out.append("'")
                i += 1
                break

            out.append(cur)
            i += 1

    return "".join(out) if changed else line


_BARE_HELPER_CALL_RE = re.compile(
    r"(?<![.\w])"
    r"(p|h[1-3]|bold|italic|bullet|numbered|spacer|divider|pageBreak|toc|bookmark|fullWidth)"
    r"\s*\("
)


def _fix_bare_helper_calls(source: str) -> tuple[str, list[str]]:
    """Prefix bare helper function calls with ``h.``.

    LLM occasionally writes ``p('...')`` instead of ``h.p('...')``.
    Only applies when the file has a ``require('docx-helper')`` import.
    """
    if "docx-helper" not in source:
        return source, []

    fixes: list[str] = []
    lines = source.splitlines()
    for idx, line in enumerate(lines):
        # 跳过字符串内容密集的行（简单启发：引号超过3个）
        if line.count("'") + line.count('"') + line.count("`") > 6:
            continue
        new_line = _BARE_HELPER_CALL_RE.sub(r"h.\1(", line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：补全 h. 前缀（{line.strip()[:50]}）")
    if fixes:
        return "\n".join(lines), fixes
    return source, []


def _auto_fix_unicode(script: Path) -> list[str]:
    """Replace smart quotes and fullwidth punctuation with ASCII equivalents.

    Fullwidth characters in JS **code** are always wrong (common LLM output
    artifact), but fullwidth characters inside string literals may be
    intentional (e.g. Chinese parentheses ``（）`` in text content).
    Replacements therefore only apply outside JS string literals.

    Returns list of fix descriptions (empty if nothing changed).
    """
    original = script.read_text(encoding="utf-8")
    string_ranges = _find_string_ranges(original)
    source = original
    fixes = []

    for bad_char, good_char, desc in _UNICODE_REPLACEMENTS:
        new_source, count = _replace_outside_strings(
            source, bad_char, good_char, string_ranges,
        )
        if count > 0:
            old_lines = source.splitlines()
            new_lines = new_source.splitlines()
            hit_lines = [
                str(i)
                for i, (a, b) in enumerate(zip(old_lines, new_lines), 1)
                if a != b
            ]
            source = new_source
            loc = f"第 {','.join(hit_lines)} 行" if hit_lines else ""
            fixes.append(f"{desc} {bad_char!r} → {good_char!r} ({count} 处) {loc}")

    if fixes:
        script.write_text(source, encoding="utf-8")

    return fixes


def _fix_mismatched_backtick(line: str) -> str:
    r"""Fix a single line where a template literal opens with ` but closes with ' or " (or vice versa).

    Common LLM artifact: the model uses backtick to enable ``${var}``
    interpolation but accidentally closes the string with a single or
    double quote instead of a matching backtick.

    Example::

        text: `（自 ${dateStr} 起至 ${endDateStr} 止）。恳请批准。',
              ^                                                       ^
              backtick opens                             single quote (BUG)

    Fixed to::

        text: `（自 ${dateStr} 起至 ${endDateStr} 止）。恳请批准。`,

    Only triggers when the string body contains ``${`` (confirming
    template-literal intent), to avoid false positives with legitimate
    multi-line template literals.
    """
    n = len(line)
    i = 0
    parts: list[str] = []

    while i < n:
        ch = line[i]

        if ch not in ("`", "'", '"'):
            parts.append(ch)
            i += 1
            continue

        open_quote = ch
        j = i + 1
        has_interp = False
        close_j = -1

        while j < n:
            if line[j] == "\\" and j + 1 < n:
                j += 2
                continue
            if line[j] == "$" and j + 1 < n and line[j + 1] == "{":
                has_interp = True
            if line[j] == open_quote:
                close_j = j
                break
            j += 1

        if close_j >= 0:
            parts.append(line[i : close_j + 1])
            i = close_j + 1
            continue

        if not has_interp:
            parts.append(line[i:])
            return "".join(parts)

        for k in range(n - 1, i, -1):
            if line[k] in ("`", "'", '"') and line[k] != open_quote:
                rest = line[k + 1 :].strip()
                if not rest or rest[0] in ",);]:}":
                    parts.append("`")
                    parts.append(line[i + 1 : k])
                    parts.append("`")
                    parts.append(line[k + 1 :])
                    return "".join(parts)
                break

        parts.append(line[i:])
        return "".join(parts)

    return "".join(parts)


def _auto_fix_mismatched_quotes(script: Path) -> list[str]:
    r"""Fix backtick-opened strings that close with ``'`` or ``"`` (or vice versa).

    Must run **before** ``_auto_fix_unicode`` because a mismatched backtick
    causes ``_find_string_ranges`` to treat a huge span as one string,
    preventing fullwidth-punctuation replacements in code regions.
    """
    source = script.read_text(encoding="utf-8")
    lines = source.splitlines()
    fixes: list[str] = []
    changed = False

    for idx, line in enumerate(lines):
        new_line = _fix_mismatched_backtick(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(
                f"引号不匹配 (` / ' 或 \" 混用) → 统一为模板字符串 (第 {idx + 1} 行)"
            )
            changed = True

    if changed:
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")

    return fixes


def _auto_fix_broken_unicode_escape(script: Path) -> list[str]:
    r"""Fix truncated \uXXXX escapes from LLM streaming interruption.

    When file tool output is cut mid-unicode-escape and the model
    resumes with raw Chinese text, the file ends up with::

        bodyText("\u4e0b...\u5f3a\u化学习算法...")

    ``\u化`` is invalid JS (``\u`` must be followed by 4 hex digits).

    Fix strategy:
      1. Remove the broken ``\u`` prefix (keep the raw UTF-8 character —
         Node.js handles mixed escaped + raw UTF-8 in string literals).
      2. If the string literal is left unclosed (common when truncation
         happens inside a function argument), close it with ``"),``.
    """
    source = script.read_text(encoding="utf-8")

    broken_re = re.compile(r"\\u(?!\{[0-9a-fA-F]+\})(?![0-9a-fA-F]{4})")
    if not broken_re.search(source):
        return []

    lines = source.splitlines()
    fixes: list[str] = []

    for idx, line in enumerate(lines):
        if not broken_re.search(line):
            continue

        new_line = broken_re.sub("", line)

        in_str: str | None = None
        i = 0
        n = len(new_line)
        while i < n:
            ch = new_line[i]
            if in_str:
                if ch == "\\" and i + 1 < n:
                    i += 2
                    continue
                if ch == in_str:
                    in_str = None
            elif ch in ('"', "'", "`"):
                in_str = ch
            i += 1

        if in_str:
            stripped = new_line.rstrip()
            new_line = stripped + in_str + "),"
            fixes.append(f"截断的 Unicode 转义 + 闭合字符串 (第 {idx + 1} 行)")
        else:
            fixes.append(f"截断的 Unicode 转义 (第 {idx + 1} 行)")

        lines[idx] = new_line

    if fixes:
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")

    return fixes


def _auto_fix_unicode_escapes(script: Path) -> list[str]:
    r"""Convert \uXXXX escapes inside string literals back to raw characters.

    LLMs frequently output Chinese text as ``\u6700\u540e`` instead of
    ``最后``.  Both are valid JS, but raw characters improve readability.
    Only replaces inside string literals to avoid breaking code.
    """
    source = script.read_text(encoding="utf-8")
    escape_re = re.compile(r"\\u([0-9a-fA-F]{4})")
    if not escape_re.search(source):
        return []

    string_ranges = _find_string_ranges(source)
    if not string_ranges:
        return []

    chars: list[str] = list(source)
    count = 0
    for m in reversed(list(escape_re.finditer(source))):
        start, end = m.start(), m.end()
        if not any(s <= start < e for s, e in string_ranges):
            continue
        raw_char = chr(int(m.group(1), 16))
        if raw_char in ('\n', '\r', '\t', '\\', '"', "'", '`'):
            continue
        chars[start:end] = [raw_char]
        count += 1

    if count == 0:
        return []

    script.write_text("".join(chars), encoding="utf-8")
    return [f"\\uXXXX 转义 → 原始字符 ({count} 处)"]


def _auto_fix_unclosed_strings(script: Path) -> list[str]:
    r"""Fix lines where a ``"`` or ``'`` string literal opens but never closes.

    In JavaScript, single- and double-quoted strings **cannot** span multiple
    lines (unlike template literals).  So if a line opens ``"`` or ``'`` and
    reaches EOL without the matching close, it is always an error — typically
    caused by LLM streaming truncation.

    Strategy per unclosed line:

    1. Append the matching close quote.
    2. Heuristically determine a suffix:
       - If the line looks like a function argument (e.g. ``bodyText("...``),
         append ``),`` to close the call.
       - If the line looks like an object property value (e.g. ``text: "...``),
         append ``,`` to end the property.
       - Otherwise just close the quote.

    Template literals (backtick) are intentionally skipped — they legitimately
    span multiple lines.
    """
    source = script.read_text(encoding="utf-8")
    lines = source.splitlines()
    fixes: list[str] = []
    changed = False

    for idx, line in enumerate(lines):
        in_str: str | None = None
        i = 0
        n = len(line)
        open_pos = -1

        while i < n:
            ch = line[i]
            if in_str:
                if ch == "\\" and i + 1 < n:
                    i += 2
                    continue
                if ch == in_str:
                    in_str = None
                    open_pos = -1
            elif ch in ('"', "'"):
                in_str = ch
                open_pos = i
            elif ch == "`":
                # skip backtick strings on this line (may be multi-line)
                j = i + 1
                while j < n:
                    if line[j] == "\\" and j + 1 < n:
                        j += 2
                        continue
                    if line[j] == "`":
                        break
                    j += 1
                i = j + 1 if j < n else j
                continue
            elif ch == "/" and i + 1 < n:
                if line[i + 1] == "/":
                    break  # rest is line comment
                if line[i + 1] == "*":
                    break  # block comment start, stop scanning
            i += 1

        if in_str is None or in_str == "`":
            continue

        # This line has an unclosed " or ' — fix it
        stripped = line.rstrip()

        # Determine suffix: look at what precedes the opening quote
        prefix = line[:open_pos].rstrip() if open_pos >= 0 else ""

        # Check if line ends with a call-closing ')' pattern.  When the
        # unclosed string sits inside a function call (prefix ends with '('
        # or ',' indicating an argument position), the trailing ')' is almost
        # certainly the intended call-closer — insert the quote before it.
        if prefix.endswith("(") or prefix.endswith(","):
            if stripped.endswith("),") or stripped.endswith(");"):
                insert_pos = stripped.rfind(")")
                lines[idx] = stripped[:insert_pos] + in_str + stripped[insert_pos:]
            elif stripped.endswith(")"):
                insert_pos = stripped.rfind(")")
                lines[idx] = stripped[:insert_pos] + in_str + stripped[insert_pos:]
            elif prefix.endswith("("):
                lines[idx] = stripped + in_str + "),"
            else:
                lines[idx] = stripped + in_str + ","
        elif prefix.endswith(":"):
            lines[idx] = stripped + in_str + ","
        else:
            lines[idx] = stripped + in_str
        fixes.append(f"未闭合字符串 ({in_str}...{in_str}) → 自动补全 (第 {idx + 1} 行)")
        changed = True

    if changed:
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")

    return fixes


_CALL_PAREN_RE = re.compile(
    r"""\b(body|h\.p|h\.h[1-6]|h\.bullet|h\.numbered|h\.text|h\.spacer)\s*(['"`])""",
)


def _fix_missing_call_paren(line: str) -> str:
    """Fix ``body'...'`` → ``body('...')`` (missing left parenthesis).

    LLMs frequently drop the ``(`` when producing long sequences of
    ``body('...')`` calls.  The pattern is: a known function name
    immediately followed by a quote character with no ``(`` in between.
    """
    m = _CALL_PAREN_RE.search(line)
    if not m:
        return line
    return _CALL_PAREN_RE.sub(r"\1(\2", line)


# `h.bullet('...'],`  →  `h.bullet('...'),`
# `h.bold('...'])`    →  `h.bold('...'))`
# The LLM occasionally substitutes the call's closing ``)`` with ``]``
# (most often when it just finished writing an array of bullets, or
# when nesting helpers and the inner helper "leaks" the outer ``]``).
# The fix is unambiguous: a known helper name opens with ``(``, holds
# a *single* simple string, and the string is followed by ``]`` and
# then ``,``, ``;``, ``)`` or whitespace.
_BAD_CLOSE_BRACKET_RE = re.compile(
    r"""(\b(?:body|h\.[A-Za-z_][\w$]*)\s*\(\s*)        # call open
        (?P<q>['"`])                                     # opening quote
        (?P<body>(?:\\.|(?!(?P=q)).)*)                   # string body
        (?P=q)                                           # matching close
        \s*\]                                            # WRONG: ] instead of )
        (?P<tail>[\s,;)])                                # follow with , ; ) or space
    """,
    re.VERBOSE,
)


def _fix_call_close_bracket(line: str) -> str:
    """Rewrite ``fn('...'],`` to ``fn('...'),`` (and ``fn('...'])``
    to ``fn('...')`` — the stray ``]`` is deleted, the existing ``)``
    already closes the call).

    Conservative: only triggers on known helper-call shapes (``h.xxx(``
    or ``body(``) wrapping a single string literal. Array literals
    (``['a', 'b']``) lack the leading helper name so they are skipped.
    """
    if "']" not in line and '"]' not in line and "`]" not in line:
        return line

    def _repl(m: re.Match) -> str:
        tail = m.group("tail")
        prefix = f"{m.group(1)}{m.group('q')}{m.group('body')}{m.group('q')}"
        if tail == ")":
            # The stray ']' was the bug — the existing ')' already
            # closes the call. Drop ']' without inserting an extra ')'.
            return prefix + tail
        return prefix + ")" + tail

    return _BAD_CLOSE_BRACKET_RE.sub(_repl, line)


# `h.numbered([…]), { ref: 'x' })`  →  `h.numbered([…], { ref: 'x' })`
# The LLM closes the call too early, then trails the options object
# outside, which is a syntax error.  Pattern:
#   ARRAY_CLOSE + ')' + ',' + '{...}' + ')'
# We look for any line whose tail matches `]),\s*\{[^}]*\}\)` AND whose
# code earlier on the same line contains a function call with `(` and
# `[` (so the offending ')' is the one that closed the array's caller).
_STRAY_CALL_PAREN_RE = re.compile(
    r"""\]\)\s*,\s*           # offending ]), with a comma
        (\{[^{}]*\})          # the orphaned options object
        \s*\)                 # the redundant final ) we keep
    """,
    re.VERBOSE,
)


def _fix_stray_paren_in_array(line: str) -> str:
    """Drop a stray ``)`` that the LLM put where a ``]`` was meant.

    Two observed shapes (both same root cause — typing ``)`` instead of
    ``]`` to end an array element):

        ['x', 'y', 'z')]            ← extra `)` before the array `]`
        ['x', 'y', 'z'),            ← extra `)` ending a row inside `rows: [...]`

    Conservative trigger: walk the line tracking paren depth (skipping
    strings & comments). A ``)`` is rewritten as ``]`` (or simply
    deleted, if a real ``]`` already follows) only when:
      - the line currently has an *open* ``[`` (we're inside an array
        literal scoped to this line), AND
      - the local paren-depth at the ``)`` is 0 (so the ``)`` isn't
        actually closing some earlier ``(``).

    This catches the round-5 ``'…文档'),`` typo, the original
    ``'…文档')],`` shape, and any combination in between.
    """
    if ")" not in line:
        return line

    out: list[str] = []
    i = 0
    n = len(line)
    paren_depth = 0
    bracket_depth = 0
    in_str: str | None = None
    changed = False

    while i < n:
        ch = line[i]
        if in_str:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(line[i + 1])
                i += 2
                continue
            if ch == in_str:
                in_str = None
            i += 1
            continue
        if ch in ("'", '"', "`"):
            in_str = ch
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < n and line[i + 1] in ("/", "*"):
            # rest is comment; copy verbatim
            out.append(line[i:])
            return "".join(out) if changed else line
        if ch == "(":
            paren_depth += 1
        elif ch == ")":
            # Decide if this `)` is the LLM typo.
            if paren_depth == 0 and bracket_depth > 0:
                # Peek ahead, skipping spaces.
                k = i + 1
                while k < n and line[k] == " ":
                    k += 1
                # `)` followed by `]` `,` `;` OR end-of-line, all while
                # we're inside an array literal — clearly the typo.
                if k >= n or line[k] in "],;":
                    # If a real `]` already follows, drop the `)` (avoid
                    # double-closing); otherwise replace `)` with `]`.
                    if k < n and line[k] == "]":
                        changed = True
                        i += 1
                        continue
                    out.append("]")
                    changed = True
                    bracket_depth = max(0, bracket_depth - 1)
                    i += 1
                    continue
            paren_depth = max(0, paren_depth - 1)
        elif ch == "[":
            bracket_depth += 1
        elif ch == "]":
            bracket_depth = max(0, bracket_depth - 1)
        out.append(ch)
        i += 1

    return "".join(out) if changed else line


def _fix_stray_call_paren(line: str) -> str:
    """Rewrite ``helper([…]), {opts})`` to ``helper([…], {opts})``.

    Observed pattern (3 consecutive failures in a single session):
        h.numbered([h.bold('提取：'), '从…']), { ref: 'splitflow' }),
    becomes
        h.numbered([h.bold('提取：'), '从…'], { ref: 'splitflow' }),

    Conservative: only triggers when the line also contains a helper
    call with both ``(`` and ``[`` (so we know the stray ``)`` belongs
    to that call, not to an unrelated paren earlier).
    """
    if "])" not in line:
        return line
    # Require at least one helper-call-with-array signature on the line
    if not re.search(r"\b(?:body|h\.[A-Za-z_][\w$]*)\s*\(\s*\[", line):
        return line
    return _STRAY_CALL_PAREN_RE.sub(r"], \1)", line)


def _fix_missing_array_close_in_call(line: str) -> str:
    """Repair common bracket mistakes inside helper array arguments.

    Observed patterns::

        h.p([h.bold('偏差原因：'), '因为…进度。'),
        →
        h.p([h.bold('偏差原因：'), '因为…进度。']),

        h.p([h.bold('偏差原因：'), '因为…进度。')]),
        →
        h.p([h.bold('偏差原因：'), '因为…进度。']),

        h.p([h.bold('偏差原因：', '因为…进度。']),
        →
        h.p([h.bold('偏差原因：', '因为…进度。')]),

    Detection: walk the line tracking ``(`` and ``[`` in a stack.  If we
    encounter ``)`` while ``[`` is on top, either the array literal was never
    closed (insert ``]``) or the model typed an extra ``)`` before an existing
    ``]`` (delete ``)``).  If we encounter ``]`` while an inner helper call's
    ``(`` is still on top, close that call first.
    """
    if "[" not in line or ")" not in line:
        return line
    if not re.search(r"\bh\.\w+\s*\(\s*\[", line):
        return line

    out: list[str] = []
    stack: list[str] = []
    in_str: str | None = None
    i = 0
    n = len(line)
    changed = False

    while i < n:
        ch = line[i]

        if in_str:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(line[i + 1])
                i += 2
                continue
            if ch == in_str:
                in_str = None
            i += 1
            continue

        if ch in ("'", '"', "`"):
            in_str = ch
            out.append(ch)
            i += 1
            continue

        if ch == "/" and i + 1 < n and line[i + 1] in ("/", "*"):
            out.append(line[i:])
            return "".join(out) if changed else line

        if ch == "(":
            stack.append("(")
        elif ch == "[":
            stack.append("[")
        elif ch == "]":
            if stack and stack[-1] == "(" and "[" in stack[:-1]:
                out.append(")")
                stack.pop()
                changed = True
            if stack and stack[-1] == "[":
                stack.pop()
        elif ch == ")":
            if stack and stack[-1] == "[":
                k = i + 1
                while k < n and line[k].isspace():
                    k += 1
                if k < n and line[k] == "]":
                    # The real array close is already present; this ')' is
                    # the stray token that caused `...')]),`.
                    changed = True
                    i += 1
                    continue
                else:
                    out.append("]")
                    stack.pop()
                    changed = True
            if stack and stack[-1] == "(":
                stack.pop()

        out.append(ch)
        i += 1

    return "".join(out) if changed else line


def _fix_array_element_semicolons(script: Path) -> list[str]:
    """Fix `);` used instead of `),` between elements of an array literal.

    LLMs occasionally write::

        return [
          h.h1('…'),
          h.bullet([…]);   ← wrong: ';' breaks the array literal
          h.bullet([…]),
        ]

    The key safety check is the *enclosing-bracket context*: a global
    paren/bracket scan must say "the bracket on top of the stack at the
    start of this line is ``[``" — meaning we really are inside an
    array literal. Without this guard, the fixer mis-converted::

        const hf = h.headerFooter('A', 'B');   ← top-level statement
        h.build({ ... })

    into ``);` → `),`, breaking otherwise-valid code (observed regression).

    Additional bailouts (cheap pre-checks):
      - Only matches lines whose entire trailing token is ``);`` (no
        ``+ x);`` etc — preserves chained statements).
      - Requires the *next* non-empty line to be another helper call or
        a bare ``]``/``}`` (so the ``;`` is followed by an array peer,
        not a fresh top-level statement).
    """
    source = script.read_text(encoding="utf-8")
    if ");" not in source:
        return []

    lines = source.splitlines()
    masked = _mask_strings_and_comments(source)
    masked_lines = masked.splitlines()

    # Compute the bracket stack at the start of every line. Stack only
    # tracks ``(`` ``[`` ``{`` opens; the *top* tells us which kind of
    # literal we're inside.
    starts: list[list[str]] = []
    stack: list[str] = []
    for ml in masked_lines:
        starts.append(stack.copy())
        for ch in ml:
            if ch in "([{":
                stack.append(ch)
            elif ch in ")]}":
                if stack:
                    stack.pop()

    fixes: list[str] = []
    changed = False
    next_continues_re = re.compile(
        r"""^\s*(?:
            (?:body|h\.[A-Za-z_][\w$]*)\s*\(   # another helper call
            |[}\]]                               # or a literal closer
        )""",
        re.VERBOSE,
    )

    for idx, line in enumerate(lines):
        stripped = line.rstrip()
        if not stripped.endswith(");"):
            continue
        # ensure this looks like a helper call line itself
        if not re.search(r"\b(?:body|h\.[A-Za-z_][\w$]*)\s*\(", stripped):
            continue
        # CRITICAL: only act when this line lives inside an array literal
        # (otherwise we damage legitimate top-level `const x = h.foo();`).
        ctx = starts[idx] if idx < len(starts) else []
        if not ctx or ctx[-1] != "[":
            continue
        # peek next non-empty line — must continue the literal
        k = idx + 1
        while k < len(lines) and not lines[k].strip():
            k += 1
        if k >= len(lines):
            continue
        if not next_continues_re.match(lines[k]):
            continue
        lines[idx] = stripped[:-1] + "," + line[len(stripped):]
        fixes.append(f"第 {idx + 1} 行：数组元素分隔符 ';' → ','")
        changed = True

    if changed:
        script.write_text("\n".join(lines) + ("\n" if source.endswith("\n") else ""),
                          encoding="utf-8")
    return fixes


def _auto_fix_syntax(script: Path, env: dict[str, str]) -> list[str]:
    """Batch-fix syntax errors that can be repaired mechanically.

    Phase 1 (bulk scan, no subprocess): fix every line in one pass
      - Missing call parenthesis: body'...' → body('...')
      - require() `as` → `:`:  { Header as H } → { Header: H }
      - Nested quotes:  bodyText("简称"星川"") → bodyText('简称"星川"')
      - Unbalanced braces: { para: { ... } }) → { para: { ... } } })
    Phase 2 (loop with node --check): pick off any remaining edge cases
    """
    fixes = []
    source = script.read_text(encoding="utf-8")

    source, ts_fixes = _strip_ts_assertions(source)
    fixes.extend(ts_fixes)

    new_source, as_fixes = _fix_require_as(source)
    if as_fixes:
        source = new_source
        fixes.extend(as_fixes)

    source, enum_fixes = _fix_bad_enums(source)
    fixes.extend(enum_fixes)

    lines = source.splitlines()
    changed = bool(ts_fixes or as_fixes or enum_fixes)

    for idx, line in enumerate(lines):
        new_line = _fix_missing_call_paren(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：函数调用补全左括号")
            line = new_line
            changed = True

        new_line = _fix_call_close_bracket(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：函数调用的 ']' 修正为 ')'")
            line = new_line
            changed = True

        new_line = _fix_stray_call_paren(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：多余的 ')' 修正为合并到 array 参数内")
            line = new_line
            changed = True

        new_line = _fix_stray_paren_in_array(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：删除 array 元素后多余的 ')'")
            line = new_line
            changed = True

        new_line = _fix_missing_array_close_in_call(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：数组参数补全缺失的 ']'")
            line = new_line
            changed = True

        new_line = _fix_surplus_close(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：移除多余闭合符")
            line = new_line
            changed = True

        new_line = _fix_latex_prime_in_sq(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：LaTeX 公式中单引号转义")
            line = new_line
            changed = True

        new_line = _fix_nested_dq(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：嵌套双引号 → 单引号/模板字符串")
            line = new_line
            changed = True

        new_line = _fix_nested_sq(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：嵌套单引号 → 模板字符串/双引号")
            line = new_line
            changed = True

        new_line = _fix_broken_arrow_suffix(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：移除箭头函数后的多余闭合符")
            line = new_line
            changed = True

        new_line = _fix_unbalanced_braces(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：花括号补全")
            changed = True

        new_line = _fix_function_keyword_no_space(line)
        if new_line != line:
            lines[idx] = new_line
            fixes.append(f"第 {idx + 1} 行：function 关键字后补空格")
            changed = True

    if changed:
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")

    # 裸 helper 调用修复（需要整文件扫描确认 docx-helper import）
    current = script.read_text(encoding="utf-8")
    new_source, bare_fixes = _fix_bare_helper_calls(current)
    if bare_fixes:
        script.write_text(new_source, encoding="utf-8")
        fixes.extend(bare_fixes)

    already_tried: set[int] = set()
    for _attempt in range(10):
        r = subprocess.run(
            [_get_node_exe(), "--check", str(script)],
            capture_output=True, text=True, env=env,
            encoding="utf-8",
        )
        if r.returncode == 0:
            break

        match = re.search(re.escape(str(script)) + r":(\d+)", r.stderr)
        if not match:
            match = re.search(re.escape(script.name) + r":(\d+)", r.stderr)
        if not match:
            break

        line_no = int(match.group(1))
        if line_no in already_tried:
            break
        already_tried.add(line_no)

        lines = script.read_text(encoding="utf-8").splitlines()
        if not (0 < line_no <= len(lines)):
            break

        line = lines[line_no - 1]
        new_line = _fix_missing_call_paren(line)
        if new_line == line:
            new_line = _fix_call_close_bracket(line)
        if new_line == line:
            new_line = _fix_stray_call_paren(line)
        if new_line == line:
            new_line = _fix_stray_paren_in_array(line)
        if new_line == line:
            new_line = _fix_missing_array_close_in_call(line)
        if new_line != line:
            new_line = _fix_surplus_close(new_line)
        if new_line == line:
            new_line = _fix_surplus_close(line)
        if new_line == line:
            new_line = _fix_latex_prime_in_sq(line)
        if new_line == line:
            new_line = _fix_nested_dq(line)
        if new_line == line:
            new_line = _fix_nested_sq(line)
        if new_line == line:
            new_line = _fix_broken_arrow_suffix(line)
        if new_line == line:
            new_line = _fix_unbalanced_braces(line)
        if new_line == line:
            fixed_source = _fix_unclosed_brackets_at_line(script, line_no)
            if fixed_source:
                script.write_text(fixed_source, encoding="utf-8")
                fixes.append(f"第 {line_no} 行：补全未闭合的括号（全局平衡分析）")
                continue
            break

        lines[line_no - 1] = new_line
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")
        fixes.append(f"第 {line_no} 行：语法修复（phase 2）")

    return fixes


_ENUM_FIXES: list[tuple[str, str, str]] = [
    (r"BorderStyle\.NONE\b", "BorderStyle.NIL", "BorderStyle.NONE → NIL"),
    (r"AlignmentType\.JUSTIFY\b", "AlignmentType.JUSTIFIED", "AlignmentType.JUSTIFY → JUSTIFIED"),
    (r"ShadingType\.SOLID\b", "ShadingType.CLEAR", "ShadingType.SOLID → CLEAR"),
    (r"HeadingLevel\.HEADING(\d)(?!_)", r"HeadingLevel.HEADING_\1", "HeadingLevel.HEADING1 → HEADING_1"),
]


def _fix_bad_enums(source: str) -> tuple[str, list[str]]:
    """Auto-fix common docx-js enum mistakes."""
    fixes: list[str] = []
    for pattern, replacement, desc in _ENUM_FIXES:
        new_source = re.sub(pattern, replacement, source)
        if new_source != source:
            count = len(re.findall(pattern, source))
            fixes.append(f"{desc} ({count} 处)")
            source = new_source
    return source, fixes


def _fix_require_as(source: str) -> tuple[str, list[str]]:
    """Fix ESM-style `as` in CommonJS require destructuring (may span multiple lines).

    const { Header as H } = require("docx")  →  const { Header: H } = require("docx")
    """
    fixes: list[str] = []

    def _replace_block(m: re.Match) -> str:
        block = m.group(0)
        if ' as ' not in block:
            return block
        new_block = re.sub(r'(\w+)\s+as\s+(\w+)', r'\1: \2', block)
        if new_block != block:
            fixes.append("require() 解构中 `as` → `:` (ESM → CommonJS)")
        return new_block

    new_source = re.sub(
        r'const\s*\{[^}]*\}\s*=\s*require\s*\([^)]+\)',
        _replace_block,
        source,
        flags=re.DOTALL,
    )
    return new_source, fixes


def _fix_nested_dq(line: str) -> str:
    """Convert double-quoted strings that contain inner double quotes to single-quoted.

    Correctly skips over single-quoted and backtick-quoted strings so that
    literal " characters inside '...' or `...` are not mistaken for
    double-quoted JS string delimiters.
    """
    i = 0
    n = len(line)
    result: list[str] = []

    while i < n:
        ch = line[i]

        # Skip single-quoted and backtick strings entirely (they can contain literal ")
        if ch in ("'", '`'):
            j = i + 1
            while j < n:
                if line[j] == '\\' and j + 1 < n:
                    j += 2
                    continue
                if line[j] == ch:
                    j += 1
                    break
                j += 1
            result.append(line[i:j])
            i = j
            continue

        if ch != '"':
            result.append(ch)
            i += 1
            continue

        j = i + 1
        has_inner = False

        while j < n:
            if line[j] == '\\':
                j += 2
                continue
            if line[j] == '"':
                if _is_js_string_close(line, j):
                    break
                else:
                    has_inner = True
            j += 1
        else:
            result.append(line[i:])
            return ''.join(result)

        content = line[i + 1 : j]
        if has_inner:
            if "'" not in content:
                result.append("'" + content + "'")
            elif '`' not in content and '${' not in content:
                result.append('`' + content + '`')
            else:
                result.append('"' + content.replace('"', '\\"') + '"')
        else:
            result.append('"' + content + '"')
        i = j + 1

    return ''.join(result)


def _fix_nested_sq(line: str) -> str:
    """Convert single-quoted strings that contain inner single quotes to backtick strings.

    Mirror of ``_fix_nested_dq`` for the single-quote case.  Chinese text
    commonly nests single quotes inside double quotes: "他说'走吧'就离开了".
    When such text sits inside a JS single-quoted string the inner ``'``
    breaks the parser::

        body('"他说了什么'送到楼梯口'的话"'),   // SyntaxError

    Fixed to::

        body(`"他说了什么'送到楼梯口'的话"`),
    """
    i = 0
    n = len(line)
    result: list[str] = []

    while i < n:
        ch = line[i]

        # Skip double-quoted and backtick strings entirely
        if ch in ('"', '`'):
            j = i + 1
            while j < n:
                if line[j] == '\\' and j + 1 < n:
                    j += 2
                    continue
                if line[j] == ch:
                    j += 1
                    break
                j += 1
            result.append(line[i:j])
            i = j
            continue

        if ch != "'":
            result.append(ch)
            i += 1
            continue

        j = i + 1
        has_inner = False

        while j < n:
            if line[j] == '\\':
                j += 2
                continue
            if line[j] == "'":
                if _is_js_string_close(line, j):
                    break
                else:
                    has_inner = True
            j += 1
        else:
            result.append(line[i:])
            return ''.join(result)

        content = line[i + 1 : j]
        if has_inner:
            if '`' not in content and '${' not in content:
                result.append('`' + content + '`')
            elif '"' not in content:
                result.append('"' + content + '"')
            else:
                result.append("'" + content.replace("'", "\\'") + "'")
        else:
            result.append("'" + content + "'")
        i = j + 1

    return ''.join(result)


_JS_IDENT_START_RE = re.compile(r"[A-Za-z_$]")


def _is_js_string_close(line: str, pos: int) -> bool:
    """Determine if the quote at *pos* is a JS string-closing delimiter (not an inner Chinese quote)."""
    rest = line[pos + 1:]
    stripped = rest.lstrip()
    if not stripped:
        return True

    ch = stripped[0]

    if ch in '],;+|&?:}':
        return True

    if ch == ')':
        after_paren = stripped[1:].lstrip()
        if not after_paren or after_paren[0] in ',;.)]\n+|&?}':
            return True
        if after_paren.startswith('.then') or after_paren.startswith('.catch'):
            return True
        return False

    if ch == ',':
        return True

    # `'x'.method(...)` or `'x'.prop` — `.` after a quote unambiguously
    # marks string close. Without this, `'C:\\…'.replace(/\\/g, '/')` is
    # mis-scanned: the `'output'` close quote is treated as an inner
    # quote, the scanner extends past `.replace(...)` and eats the `'/'`
    # quotes, then wraps the whole expression in backticks (regression
    # observed in SOP-template session).
    if ch == "." and len(stripped) > 1 and _JS_IDENT_START_RE.match(stripped[1]):
        return True

    return False


_OPEN_TO_CLOSE = {"(": ")", "[": "]", "{": "}"}
_CLOSE_SET = {")", "]", "}"}


def _fix_unbalanced_braces(line: str) -> str:
    r"""Fix bracket-nesting errors within a single line.

    Handles two categories:

    1. **Missing ``}`` before ``)`` or ``]``** (most common LLM artifact)::

         { spacing: { after: 30, line: 260 })   →   { spacing: { after: 30, line: 260 } })

       When a ``)`` or ``]`` is encountered but the stack top expects ``}``,
       insert missing ``}`` closers one at a time until the ``)``/``]`` matches.

    2. **Surplus ``{`` at EOL** (original behaviour):
       If after the full scan more ``{`` than ``}`` remain (ignoring ``()``
       and ``[]`` which legitimately span multiple lines), append the
       missing ``}`` before the trailing ``),`` / ``);`` / ``)``.

    String literals and escape sequences are skipped.
    """
    # -- Phase 1: walk the line, fix mid-line } missing before ) or ] --
    parts: list[str] = []
    stack: list[str] = []  # expected closers
    in_str: str | None = None
    i = 0
    n = len(line)
    changed = False

    while i < n:
        ch = line[i]

        if in_str:
            if ch == "\\" and i + 1 < n:
                parts.append(line[i : i + 2])
                i += 2
                continue
            if ch == in_str:
                in_str = None
            parts.append(ch)
            i += 1
            continue

        if ch in ('"', "'", "`"):
            in_str = ch
            parts.append(ch)
            i += 1
            continue

        if ch == "/" and i + 1 < n and line[i + 1] == "/":
            parts.append(line[i:])
            i = n
            break

        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
            parts.append(ch)
            i += 1
            continue

        if ch in _CLOSE_SET:
            if stack and stack[-1] == ch:
                stack.pop()
                parts.append(ch)
                i += 1
                continue

            # Mismatch: e.g. stack top is "}" but we got ")".
            # Only insert missing "}" (braces), not ")" or "]".
            # Insert one "}" at a time, re-check after each.
            if stack and stack[-1] == "}" and ch in (")", "]"):
                inserted = 0
                while stack and stack[-1] == "}":
                    stack.pop()
                    inserted += 1
                    if stack and stack[-1] == ch:
                        break
                if stack and stack[-1] == ch:
                    stack.pop()
                    parts.append(" " + "} " * inserted + ch)
                    changed = True
                    i += 1
                    continue
                # Didn't find a match — restore stack and leave as-is
                for _ in range(inserted):
                    stack.append("}")

            parts.append(ch)
            i += 1
            continue

        parts.append(ch)
        i += 1

    # -- Phase 2: leftover unmatched { → append } before trailing ), --
    # Only count { vs } imbalance; ignore ( and [ (they span lines).
    brace_deficit = sum(1 for s in stack if s == "}")
    if brace_deficit <= 0:
        if changed:
            return "".join(parts)
        return line

    result = "".join(parts)
    rstripped = result.rstrip()

    # If the line ends with an opener, comma, or arrow (`=>`), the braces
    # are intentionally unclosed (multi-line structure) — don't touch.
    # `=>` means a multi-line arrow function body on the next line, e.g.:
    #   cols.map((c, i) =>
    #     new TableCell({ ... })
    #   )
    if rstripped and (
        rstripped[-1] in ("{", "[", "(", ",")
        or rstripped.endswith("=>")
    ):
        if changed:
            return result
        return line

    trail = result[len(rstripped):]
    insertion = " }" * brace_deficit

    for suffix in ["),", ");", ")", "],", "];", "]"]:
        if rstripped.endswith(suffix):
            insert_pos = len(rstripped) - len(suffix)
            return rstripped[:insert_pos] + insertion + suffix + trail

    return rstripped + insertion + trail


def _fix_unclosed_brackets_at_line(script: Path, error_line: int) -> str | None:
    """Scan from file start to *error_line* tracking ``(``, ``[``, ``{`` nesting.

    When the existing single-line fixers all fail, the error is often caused by
    a missing ``)`` or ``]`` far from where they were opened — e.g. the LLM
    writes ``refs.bibliography([...];`` instead of ``refs.bibliography([...]);``.

    Strategy:
      1. Walk the source up to (and including) the error line, maintaining a
         bracket stack (with line numbers) that skips string literals and
         comments.
      2. Look at the error line and the *next* non-empty line.  If the next
         line starts with a closer (``]``, ``)``, ``}``) that doesn't match
         the current stack top, there are missing closers in between.
      3. Collect the closers needed to bridge from the stack top down to the
         opener that matches the next line's closer, and insert them on the
         error line before the trailing ``;`` / ``,``.

    This avoids the false-positive of closing *all* unclosed brackets (which
    would close legitimately multi-line ``function {`` / ``return [`` etc.).

    Returns the fixed full source if a repair was made, or ``None``.
    """
    source = script.read_text(encoding="utf-8")
    lines = source.splitlines()
    if not (0 < error_line <= len(lines)):
        return None

    target_idx = error_line - 1
    scan_end = sum(len(lines[i]) + 1 for i in range(error_line))

    # (opener_char, line_number_1based)
    stack: list[tuple[str, int]] = []
    open_to_close = {"(": ")", "[": "]", "{": "}"}
    close_to_open = {")": "(", "]": "[", "}": "{"}
    in_str: str | None = None
    in_line_comment = False
    in_block_comment = False
    i = 0
    current_line = 1

    while i < scan_end and i < len(source):
        ch = source[i]

        if ch == "\n":
            in_line_comment = False
            current_line += 1
            i += 1
            continue

        if in_line_comment:
            i += 1
            continue

        if in_block_comment:
            if ch == "*" and i + 1 < len(source) and source[i + 1] == "/":
                in_block_comment = False
                i += 2
            else:
                i += 1
            continue

        if in_str:
            if ch == "\\" and i + 1 < len(source):
                i += 2
                continue
            if ch == in_str:
                in_str = None
            i += 1
            continue

        if ch in ("'", '"', "`"):
            in_str = ch
            i += 1
            continue

        if ch == "/" and i + 1 < len(source):
            if source[i + 1] == "/":
                in_line_comment = True
                i += 2
                continue
            if source[i + 1] == "*":
                in_block_comment = True
                i += 2
                continue

        if ch in open_to_close:
            stack.append((ch, current_line))
        elif ch in close_to_open:
            if stack and stack[-1][0] == close_to_open[ch]:
                stack.pop()

        i += 1

    if not stack:
        return None

    # Determine what the *next* non-empty line expects to close.
    # e.g. if error line is `    ];` and next line is `}`, V8 chokes because
    # it expected `)` (for `bibliography(`) before seeing `}`.
    next_closer: str | None = None
    for li in range(error_line, len(lines)):
        stripped = lines[li].lstrip()
        if not stripped:
            continue
        if stripped[0] in close_to_open:
            next_closer = stripped[0]
        break

    needed_closers: list[str] = []

    if next_closer:
        # Pop from stack top until we find the opener matching next_closer.
        # Everything popped along the way needs a closer inserted at error_line.
        target_opener = close_to_open[next_closer]
        for opener, _ln in reversed(stack):
            if opener == target_opener:
                break
            needed_closers.append(open_to_close[opener])
    else:
        # No next-line closer hint: fall back to closing unclosed ( and [
        # that were opened on the *same line* as the error — safest scope.
        for opener, ln in reversed(stack):
            if ln == error_line and opener in ("(", "["):
                needed_closers.append(open_to_close[opener])

    if not needed_closers:
        return None

    closers_str = "".join(needed_closers)

    line = lines[target_idx]
    rstripped = line.rstrip()

    if rstripped.endswith(";"):
        new_line = rstripped[:-1] + closers_str + ";"
    elif rstripped.endswith(","):
        new_line = rstripped[:-1] + closers_str + ","
    else:
        new_line = rstripped + closers_str

    if new_line == line:
        return None

    lines[target_idx] = new_line
    return "\n".join(lines) + "\n"


def _auto_fix_output_dirs(script: Path) -> list[str]:
    """Ensure output directories for writeFileSync exist.

    If a writeFileSync target's parent directory doesn't exist, create it
    and inject mkdirSync at the top of the script.
    """
    source = script.read_text(encoding="utf-8")
    dirs_to_create: dict[str, int] = {}  # dir_path → line_no

    for m in re.finditer(r"""writeFileSync\s*\(\s*['"]([^'"]+)['"]\s*""", source):
        fpath = m.group(1)
        parent = str(Path(fpath).parent)
        if parent and not Path(parent).exists():
            line_no = source[:m.start()].count("\n") + 1
            dirs_to_create[parent] = line_no

    if not dirs_to_create:
        return []

    fixes = []
    for d, line_no in dirs_to_create.items():
        Path(d).mkdir(parents=True, exist_ok=True)
        fixes.append(f"自动创建输出目录: {d} (writeFileSync 第 {line_no} 行)")

    return fixes


def _auto_fix_placeholders(script: Path) -> list[str]:
    """Replace ``<SKILL_DIR>`` placeholder left by the LLM.

    ``<SKILL_DIR>`` → ``path.join(process.env.SKILL_PATH, ...)``
    """
    source = script.read_text(encoding="utf-8")
    fixes: list[str] = []

    skill_re = re.compile(
        r"""require\s*\(\s*['"]<SKILL_DIR>/([^'"]+)['"]\s*\)""",
    )
    if skill_re.search(source):
        def _repl_skill(m: re.Match) -> str:
            segments = m.group(1).replace("\\", "/").split("/")
            joined = ", ".join(f"'{s}'" for s in segments)
            return f"require(path.join(process.env.SKILL_PATH, {joined}))"
        source = skill_re.sub(_repl_skill, source)
        if "const path = require('path')" not in source and 'const path = require("path")' not in source:
            source = "const path = require('path');\n" + source
        fixes.append("<SKILL_DIR> 占位符 → process.env.SKILL_PATH + path.join")

    if fixes:
        script.write_text(source, encoding="utf-8")

    return fixes


def _auto_fix_fake_env_vars(script: Path) -> list[str]:
    """Replace non-existent process.env directory variables with __dirname.

    LLMs often invent env vars like ``process.env.workspace_dir`` or
    ``process.env.OUTPUT_DIR`` that don't exist at runtime.  The script
    always runs from its own directory, so ``__dirname`` is the correct
    replacement.

    Special case: ``path.join(process.env.workspace_dir, 'output')`` →
    ``__dirname`` because the script is already inside the output folder.
    """
    source = script.read_text(encoding="utf-8")
    fixes: list[str] = []

    _fake_env_re = re.compile(
        r"process\.env\."
        r"(?:workspace_dir|output_dir|OUTPUT_DIR|work_dir|WORK_DIR"
        r"|project_dir|PROJECT_DIR|working_dir|WORKING_DIR"
        r"|base_dir|BASE_DIR|root_dir|ROOT_DIR"
        r"|WORKSPACE|workspace|WORKSPACE_DIR|WORKSPACE_ROOT"
        r"|OUTPUT|output|OUTPUT_ROOT|OUTPUT_PATH"
        r"|PROJECT_ROOT|projectRoot|projectDir"
        r"|WORK_PATH|workPath|workDir)"
        r"\b"
    )
    if not _fake_env_re.search(source):
        return fixes

    _join_output_re = re.compile(
        r"path\.join\s*\(\s*" + _fake_env_re.pattern + r"\s*,\s*['\"]output['\"]\s*\)"
    )
    new_source = _join_output_re.sub("__dirname", source)

    new_source = _fake_env_re.sub("__dirname", new_source)

    if new_source != source:
        script.write_text(new_source, encoding="utf-8")
        fixes.append("不存在的 process.env 目录变量 → __dirname")

    return fixes


def _auto_fix_python_raw_strings(script: Path) -> list[str]:
    """Fix Python-style raw string prefixes that LLMs sometimes write in JS.

    Observed shapes (all illegal in JS — ``r`` is just a bare identifier):
        r'C:\\Users\\...'    → 'C:\\\\Users\\\\...'
        r"C:\\Users\\..."    → "C:\\\\Users\\\\..."
        r`C:\\Users\\...`    →  `C:\\\\Users\\\\...`  (round-5 case)
    """
    source = script.read_text(encoding="utf-8")
    fixes: list[str] = []

    # Single- / double-quoted variants
    _py_raw_re = re.compile(r"(?<![a-zA-Z_$0-9])r(['\"])(.+?)\1")

    def _repl(m: re.Match) -> str:
        q = m.group(1)
        content = m.group(2).replace("\\", "\\\\")
        return f"{q}{content}{q}"

    new_source = _py_raw_re.sub(_repl, source)

    # Template-literal variant: `r`...`` — backticks legitimately span
    # lines so we can't use `(.+?)\1` non-greedy. Walk it manually.
    def _strip_raw_backtick(src: str) -> str:
        out: list[str] = []
        i = 0
        n = len(src)
        while i < n:
            ch = src[i]
            # Identify `r` as the raw prefix only when preceded by a
            # non-identifier char (or start of file).
            if (
                ch == "r"
                and i + 1 < n
                and src[i + 1] == "`"
                and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] in "_$"))
            ):
                # Find the matching closing backtick (skip escapes).
                j = i + 2
                buf: list[str] = []
                while j < n:
                    c = src[j]
                    if c == "\\" and j + 1 < n:
                        # In a raw string the backslash is literal — double it.
                        buf.append("\\\\")
                        buf.append(src[j + 1])
                        j += 2
                        continue
                    if c == "`":
                        break
                    buf.append(c)
                    j += 1
                if j < n and src[j] == "`":
                    out.append("`" + "".join(buf) + "`")
                    i = j + 1
                    continue
            out.append(ch)
            i += 1
        return "".join(out)

    new_source = _strip_raw_backtick(new_source)

    if new_source != source:
        script.write_text(new_source, encoding="utf-8")
        fixes.append("Python r'...'/r\"...\"/r`...` 原始字符串语法 → JS 转义字符串")

    return fixes


# Minimal CSS named-color table (covers what LLMs actually emit for
# docx documents — we don't need full CSS 4 coverage).
_CSS_NAMED_COLORS: dict[str, str] = {
    "black": "000000", "white": "FFFFFF", "red": "FF0000",
    "green": "008000", "blue": "0000FF", "yellow": "FFFF00",
    "cyan": "00FFFF", "magenta": "FF00FF", "gray": "808080",
    "grey": "808080", "silver": "C0C0C0", "maroon": "800000",
    "olive": "808000", "purple": "800080", "teal": "008080",
    "navy": "000080", "orange": "FFA500", "pink": "FFC0CB",
    "brown": "A52A2A", "lime": "00FF00", "indigo": "4B0082",
    "violet": "EE82EE", "gold": "FFD700", "darkgray": "A9A9A9",
    "darkgrey": "A9A9A9", "lightgray": "D3D3D3", "lightgrey": "D3D3D3",
    "transparent": "FFFFFF",  # docx-js can't express alpha; collapse to white
}

_RGBA_FN_RE = re.compile(
    r"""rgba?\s*\(\s*
        (\d{1,3})\s*,\s*
        (\d{1,3})\s*,\s*
        (\d{1,3})
        (?:\s*,\s*([\d.]+))?
        \s*\)""",
    re.VERBOSE,
)
_HEX_HASH_RE = re.compile(r"#([0-9a-fA-F]{6})\b")
_HEX_HASH_SHORT_RE = re.compile(r"#([0-9a-fA-F]{3})\b")


def _rgba_to_hex(r: int, g: int, b: int, a: float = 1.0) -> str:
    """Flatten rgba() against a white background → 6-digit hex.

    docx-js refuses alpha (`Expected 6 digit hex value`), so we composite
    against #FFFFFF using the common ``out = src * a + bg * (1-a)`` rule.
    `a` is clamped to [0,1]. Channel values are clamped to [0,255].
    """
    r = max(0, min(255, r)); g = max(0, min(255, g)); b = max(0, min(255, b))
    a = max(0.0, min(1.0, a))
    cr = round(r * a + 255 * (1 - a))
    cg = round(g * a + 255 * (1 - a))
    cb = round(b * a + 255 * (1 - a))
    return f"{cr:02X}{cg:02X}{cb:02X}"


def _auto_fix_color_values(script: Path) -> list[str]:
    """Normalize CSS color values inside string literals to 6-digit hex.

    docx-js only accepts bare 6-digit hex (no ``#``, no ``rgba()``, no
    CSS named colors). LLMs frequently emit those formats anyway —
    typical regressions seen in production:

      - ``h.divider('rgba(255,255,255,0.4)')``
      - ``color: '#FFFFFF'``
      - ``fill: 'white'``

    Only string-literal contents are touched (code regions are left
    alone) and only when the entire string is a recognisable color
    value, so legitimate URLs/text containing ``#abc123`` are not
    mangled.
    """
    source = script.read_text(encoding="utf-8")
    if not any(tag in source for tag in ("rgb", "#", *_CSS_NAMED_COLORS)):
        return []

    string_ranges = _find_string_ranges(source)
    if not string_ranges:
        return []

    fixes: list[str] = []
    counts: dict[str, int] = {}

    chunks: list[str] = []
    last = 0
    for start, end in string_ranges:
        chunks.append(source[last:start])
        # String literal is source[start:end] inclusive of delimiters.
        delim = source[start]
        body = source[start + 1:end - 1]
        new_body = body
        stripped = body.strip()
        replaced: str | None = None
        kind: str | None = None

        m = _RGBA_FN_RE.fullmatch(stripped)
        if m:
            r, g, b = int(m.group(1)), int(m.group(2)), int(m.group(3))
            a = float(m.group(4)) if m.group(4) else 1.0
            replaced = _rgba_to_hex(r, g, b, a)
            kind = "rgba/rgb()"
        elif _HEX_HASH_RE.fullmatch(stripped):
            replaced = stripped[1:].upper()
            kind = "#RRGGBB"
        elif _HEX_HASH_SHORT_RE.fullmatch(stripped):
            short = stripped[1:]
            replaced = (short[0] * 2 + short[1] * 2 + short[2] * 2).upper()
            kind = "#RGB"
        elif stripped.lower() in _CSS_NAMED_COLORS:
            replaced = _CSS_NAMED_COLORS[stripped.lower()]
            kind = f"CSS '{stripped}'"

        if replaced is not None and replaced != body:
            new_body = replaced
            counts[kind] = counts.get(kind, 0) + 1

        chunks.append(delim + new_body + delim)
        last = end
    chunks.append(source[last:])

    new_source = "".join(chunks)
    if new_source == source:
        return []

    script.write_text(new_source, encoding="utf-8")
    for kind, n in counts.items():
        fixes.append(f"色值归一化 {kind} → 6 位 hex ({n} 处)")
    return fixes


# ── Auto-fix pipeline ────────────────────────────────────────────

def _node_check_clean(script: Path, env: dict[str, str]) -> bool:
    """Return True if `node --check` reports no syntax error."""
    try:
        r = subprocess.run(
            [_get_node_exe(), "--check", str(script)],
            capture_output=True, text=True, env=env,
            encoding="utf-8",
        )
        return r.returncode == 0
    except OSError:
        # Treat missing node as "clean" so a missing toolchain doesn't
        # cause us to roll back legitimate fixes.
        return True


_CONST_DECL_RE = re.compile(r"^(\s*const\s+)(\w+)(\s*=)")


def _auto_fix_duplicate_const(script: Path) -> list[str]:
    """Remove duplicate top-level ``const`` declarations caused by LLM append.

    Common pattern when the model writes long documents via append workflow:
    the skeleton (first write) declares ``const hf = h.headerFooter(...)``
    and the final append re-declares it before ``h.build()``.  V8 rejects
    the second ``const`` with ``SyntaxError: Identifier 'X' has already
    been declared``.

    Strategy: keep the *first* occurrence of each ``const <name> =`` and
    convert later duplicates to plain assignments (``<name> =``), which is
    semantically harmless for the variables we see in practice (hf, C, refs).
    We only touch lines at brace-depth 0 (top-level) to avoid mangling
    local variables inside functions that legitimately shadow outer names.
    """
    source = script.read_text(encoding="utf-8")
    masked = _mask_strings_and_comments(source)
    lines = source.splitlines()
    masked_lines = masked.splitlines()

    seen: dict[str, int] = {}
    fixes: list[str] = []
    depth = 0

    for i, (raw_line, mline) in enumerate(zip(lines, masked_lines)):
        for ch in mline:
            if ch in "{[(":
                depth += 1
            elif ch in "}])":
                depth -= 1

        if depth != 0:
            continue

        m = _CONST_DECL_RE.match(mline)
        if not m:
            continue

        var_name = m.group(2)
        if var_name not in seen:
            seen[var_name] = i
            continue

        rm = _CONST_DECL_RE.match(raw_line)
        if not rm:
            continue
        indent = rm.group(1).replace("const ", "").replace("const\t", "")
        lines[i] = f"{indent}{rm.group(2)}{rm.group(3)}" + raw_line[rm.end():]
        fixes.append(
            f"第 {i + 1} 行：重复的 const {var_name} 声明 → 改为赋值"
            f"（首次声明在第 {seen[var_name] + 1} 行）"
        )

    if fixes:
        script.write_text(
            "\n".join(lines) + ("\n" if source.endswith("\n") else ""),
            encoding="utf-8",
        )
    return fixes


_BUILD_CALL_RE = re.compile(r"^(\s*)(\w+)\.build\s*\(\s*\{")


def _auto_fix_duplicate_build(script: Path) -> list[str]:
    """Remove earlier ``h.build()`` calls when the file contains multiple.

    Common pattern when the model writes ``h.build()`` in the skeleton
    (first write) AND again in the final append.  The first call sits
    between function declarations and is always incomplete — only the
    *last* call has all sections.  Node.js would execute both sequentially,
    but the first one references functions not yet declared (append order),
    and even if hoisting saves it, having two build calls is wasteful and
    often causes bracket-balance confusion in the auto-fix pipeline.

    Strategy: find all top-level ``h.build({`` occurrences (brace depth 0),
    identify the block extent of each (matching closing ``});``), keep only
    the last one, and delete all earlier ones.
    """
    source = script.read_text(encoding="utf-8")
    masked = _mask_strings_and_comments(source)
    lines = source.splitlines()
    masked_lines = masked.splitlines()

    build_starts: list[int] = []
    depth = 0

    for i, mline in enumerate(masked_lines):
        at_top = depth == 0
        for ch in mline:
            if ch in "{[(":
                depth += 1
            elif ch in "}])":
                depth -= 1
        if at_top and _BUILD_CALL_RE.match(mline):
            build_starts.append(i)

    if len(build_starts) <= 1:
        return []

    to_remove: list[tuple[int, int]] = []
    for start_idx in build_starts[:-1]:
        d = 0
        d_was_positive = False
        end_idx = start_idx
        for j in range(start_idx, len(masked_lines)):
            for ch in masked_lines[j]:
                if ch in "{[(":
                    d += 1
                    d_was_positive = True
                elif ch in "}])":
                    d -= 1
            if d_was_positive and d == 0:
                end_idx = j
                break
        to_remove.append((start_idx, end_idx))

    fixes: list[str] = []
    remove_set: set[int] = set()
    for start, end in to_remove:
        for k in range(start, end + 1):
            remove_set.add(k)
        fixes.append(
            f"第 {start + 1}-{end + 1} 行：删除多余的 h.build() 调用"
            f"（保留最后一个完整调用，在第 {build_starts[-1] + 1} 行）"
        )

    new_lines = [l for i, l in enumerate(lines) if i not in remove_set]
    script.write_text(
        "\n".join(new_lines) + ("\n" if source.endswith("\n") else ""),
        encoding="utf-8",
    )
    return fixes


def auto_fix_pipeline(script: Path, env: dict[str, str]) -> list[str]:
    """Run all auto-fix stages in order, return combined fix descriptions.

    Stage 1 — Normalization (text-level, no syntax awareness needed):
      - Placeholder replacement (<SKILL_DIR> → process.env)
      - Fake env vars (process.env.workspace_dir etc.) → __dirname
      - Python r'...' raw strings → JS escaped strings
      - CSS color values (rgba/rgb/#hex/named) → 6-digit hex
      - Mismatched quote delimiters (backtick open / single close)
      - Fullwidth punctuation outside strings → ASCII
      - Broken \\uXXXX escapes from streaming truncation
      - \\uXXXX inside strings → raw characters

    Stage 2 — Syntax repair (line-level and AST-aware):
      - Unclosed string literals
      - Missing call parentheses: body'...' → body('...')
      - Nested quotes, TS assertions, ESM→CJS
      - Unbalanced/surplus brackets
      - Enum typos (BorderStyle.NONE → NIL, etc.)
      - node --check loop for remaining edge cases

    Stage 3 — Environment (filesystem):
      - Create missing output directories for writeFileSync

    Safety net:
      If the LLM-written script is already `node --check`-clean and the
      pipeline somehow makes it un-parseable (auto-fix regression),
      restore the original content and report the rollback instead.
      Without this any bug in a single fixer becomes a poison pill for
      every downstream caller.
    """
    fixes: list[str] = []

    baseline = script.read_text(encoding="utf-8")
    baseline_clean = _node_check_clean(script, env)

    # ── Stage 1: Normalization ──
    fixes.extend(_auto_fix_placeholders(script))
    fixes.extend(_auto_fix_fake_env_vars(script))
    fixes.extend(_auto_fix_python_raw_strings(script))
    fixes.extend(_auto_fix_color_values(script))
    fixes.extend(_auto_fix_mismatched_quotes(script))
    fixes.extend(_auto_fix_unicode(script))
    fixes.extend(_auto_fix_broken_unicode_escape(script))
    fixes.extend(_auto_fix_unicode_escapes(script))

    # ── Stage 2: Syntax repair ──
    fixes.extend(_auto_fix_duplicate_build(script))
    fixes.extend(_auto_fix_duplicate_const(script))
    fixes.extend(_auto_fix_unclosed_strings(script))
    fixes.extend(_auto_fix_syntax(script, env))
    # `;` → `,` must run *after* bracket-typo fixes (`]` ↔ `)`),
    # otherwise the bracket-stack tracking used by the semicolon fixer
    # gets confused by the still-misplaced brackets and skips the line.
    fixes.extend(_fix_array_element_semicolons(script))
    fixes.extend(_auto_fix_surplus_brackets(script, env))
    fixes.extend(_auto_fix_orphan_premature_close(script))
    fixes.extend(_auto_fix_stray_close_brackets(script, env))
    # Surplus-close repair can turn `helper(...)])];` into `helper(...);`.
    # If that line lives inside a children array, run the delimiter fixer once
    # more after bracket cleanup.
    fixes.extend(_fix_array_element_semicolons(script))

    # ── Stage 3: Environment ──
    fixes.extend(_auto_fix_output_dirs(script))

    # ── Safety net: rollback if a clean script became un-parseable ──
    if baseline_clean and script.read_text(encoding="utf-8") != baseline:
        if not _node_check_clean(script, env):
            script.write_text(baseline, encoding="utf-8")
            return [
                "[rollback] auto-fix 将原本可解析的脚本改坏，已回滚到 LLM 原始版本（"
                f"丢弃 {len(fixes)} 个 fix 以保证可执行性）"
            ]

    return fixes


_STRAY_CLOSE_ONLY_RE = re.compile(r"^\s*[\])};,\s]+$")
_PURE_CLOSE_LINE_RE = re.compile(r"^\s*(?:\]\)\];?|\]\);?|\)\];?|\)\)\];?|\]\]\)?;?)\s*$")


def _auto_fix_stray_close_brackets(script: Path, env: dict[str, str]) -> list[str]:
    r"""Delete a line consisting purely of stray closing brackets.

    Observed pattern (5× in round-4, one session hit it 4 times in a row):

        function chapter2() {
          return [
            ...,
            h.p('社群分层运营 …'),
          ];
          ])];        ← stray: extra `)` plus an extra `]`
        }

    The line is recognisable mechanically:
      1. ``node --check`` reports ``Unexpected token ']'`` (or '),', ',')
         on a specific line number;
      2. that line, with whitespace stripped, is purely closing tokens
         like ``])]``, ``])];``, ``)];``, ``]]);``;
      3. **deleting** that line makes the rest of the file parse.

    Conservative: we only act when (1)+(2)+(3) all hold. If deletion
    doesn't fix parsing, we put the line back and bail.
    """
    fixes: list[str] = []
    attempts = 0
    while attempts < 5:
        attempts += 1
        r = subprocess.run(
            [_get_node_exe(), "--check", str(script)],
            capture_output=True, text=True, env=env, encoding="utf-8",
        )
        if r.returncode == 0:
            break

        # Only act on "Unexpected token ']'", "Unexpected token ')'" etc.
        if "Unexpected token" not in (r.stderr or ""):
            break

        match = re.search(re.escape(str(script)) + r":(\d+)", r.stderr)
        if not match:
            match = re.search(re.escape(script.name) + r":(\d+)", r.stderr)
        if not match:
            break
        line_no = int(match.group(1))

        source = script.read_text(encoding="utf-8")
        lines = source.splitlines()
        if not (0 < line_no <= len(lines)):
            break

        target = lines[line_no - 1]
        if not _PURE_CLOSE_LINE_RE.match(target):
            break

        # Try deletion and see if parser is happier
        trial_lines = lines[:line_no - 1] + lines[line_no:]
        trial = "\n".join(trial_lines) + ("\n" if source.endswith("\n") else "")
        script.write_text(trial, encoding="utf-8")
        r2 = subprocess.run(
            [_get_node_exe(), "--check", str(script)],
            capture_output=True, text=True, env=env, encoding="utf-8",
        )
        if r2.returncode == 0:
            fixes.append(f"第 {line_no} 行：删除孤立的多余闭合括号 `{target.strip()}`")
            break

        # Did deletion move the error to a *different* line? If so, keep
        # the deletion and loop (chained strays).
        m2 = re.search(re.escape(str(script)) + r":(\d+)", r2.stderr)
        if not m2:
            m2 = re.search(re.escape(script.name) + r":(\d+)", r2.stderr)
        new_line_no = int(m2.group(1)) if m2 else -1

        if new_line_no > 0 and new_line_no != line_no:
            fixes.append(f"第 {line_no} 行：删除孤立的多余闭合括号 `{target.strip()}`")
            continue

        # Deletion didn't help — restore the line and bail.
        script.write_text(source, encoding="utf-8")
        break

    return fixes


def _auto_fix_orphan_premature_close(script: Path) -> list[str]:
    """Auto-fix the most common "function prematurely closed during append" pattern.

    Only handles the high-confidence shape (covers ~80% of real LLM bugs):

    **Shape A** — ``}`` on its own line::

        function chapterX() {
          return [
            ...,
            h.p('...'),
          ];      ← premature close
        }         ← premature close

        h.p('...'),    ← orphan (top-level expression statement w/ trailing comma)
        h.p('...'),    ← orphan
        ...,
        ]              ← lone closing bracket at end

    **Shape B** — ``}`` fused with first orphan on same line::

          ];      ← premature close
        }    h.h2('6.3 ...'),   ← brace + first orphan on one line

        h.p('...'),    ← orphan
        ...,
        ]              ← lone closing bracket at end

    Conservative bailouts (let preflight error msg guide the model instead):
      - more than one isolated `]` at top level
      - orphan block contains non-helper statements (if/for/assignment/etc.)
      - orphan block > 30 lines (avoid mass-editing)
      - cannot find a clean `];` + `}` pair right above first orphan
    """
    raw = script.read_text(encoding="utf-8")
    masked = _mask_strings_and_comments(raw)
    lines = raw.splitlines()
    masked_lines = masked.splitlines()

    # 计算每行起始时的累计括号深度
    starts: list[int] = []
    depth = 0
    for line in masked_lines:
        starts.append(depth)
        for ch in line:
            if ch in "{[(":
                depth += 1
            elif ch in "}])":
                depth -= 1

    orphan_helper_lines: list[int] = []   # 顶层 h.xxx(...), 行索引
    isolated_close_lines: list[int] = []  # 顶层孤立 `]` 行索引
    other_orphans: list[int] = []         # 顶层其它可疑（控制流/赋值/嵌套）→ 放弃自动修

    for i, (ml, ds) in enumerate(zip(masked_lines, starts)):
        if ds != 0:
            continue
        stripped = ml.strip()
        if not stripped:
            continue
        if _TOP_LEVEL_LEGAL_RE.match(ml):
            continue
        if re.match(
            r"^\s*[\w$]+(\s*\.\s*[\w$]+|\s*\[[^\]]*\])*\s*(=(?!=)|\+=|-=|\*=|/=|\|\|=|&&=|\?\?=)",
            ml,
        ):
            # 顶层赋值不视为悬空（避免误改）
            continue
        if re.fullmatch(r"\s*[)};\s]+", ml):
            continue
        # 顶层 helper 调用 + 尾随逗号
        if (
            re.match(r"^\s*[\w$]+\s*\.\s*[\w$]+\s*\(", ml)
            and stripped.endswith(",")
        ):
            orphan_helper_lines.append(i)
            continue
        # 顶层孤立 `]` / `}]` / `]);` 等闭合行
        if re.fullmatch(r"\s*\][\s\];,]*\s*", ml):
            isolated_close_lines.append(i)
            continue
        # 其它可疑顶层行 → 放弃自动修
        other_orphans.append(i)

    # 高置信度判定
    if not orphan_helper_lines or not isolated_close_lines:
        return []
    if len(isolated_close_lines) != 1:
        return []
    if other_orphans:
        return []
    if len(orphan_helper_lines) > 30:
        return []

    first_orphan = orphan_helper_lines[0]
    last_orphan = orphan_helper_lines[-1]
    iso_close = isolated_close_lines[0]

    # 孤立 `]` 必须在悬空 helper 段之后
    if iso_close <= last_orphan:
        return []

    # 第一处悬空 → 往上跳过空行 → 期望最近一行是 `}` 或 `} + helper(...),`
    j = first_orphan - 1
    while j >= 0 and not masked_lines[j].strip():
        j -= 1
    if j < 0:
        return []

    fused_orphan_tail: str | None = None
    brace_ml = masked_lines[j].strip()
    if brace_ml == "}":
        close_brace_line = j
    elif (
        re.match(r"^\s*\}\s*[\w$]+\s*\.\s*[\w$]+\s*\(", masked_lines[j])
        and brace_ml.endswith(",")
    ):
        close_brace_line = j
        fused_orphan_tail = re.sub(r"^\s*\}\s*", "", lines[j])
    else:
        return []

    # 再往上跳过空行 → 期望最近一行匹配 `\s*\];?\s*`
    j -= 1
    while j >= 0 and not masked_lines[j].strip():
        j -= 1
    if j < 0 or not re.fullmatch(r"\s*\];?\s*", masked_lines[j]):
        return []
    close_array_line = j

    # 推断缩进：以提前闭合的 `];` 行缩进为基准
    base_indent = len(lines[close_array_line]) - len(lines[close_array_line].lstrip())
    content_indent = " " * (base_indent + 2)
    closing_array = " " * base_indent + "];"
    closing_brace = "}"

    delete_set: set[int] = {close_array_line, iso_close}
    if fused_orphan_tail is None:
        delete_set.add(close_brace_line)

    # 顺便吃掉 `}` 后紧邻的空白行（修复后这段空白属于章节函数内部，留着会很丑）
    k = close_brace_line + 1
    while k < len(lines) and not lines[k].strip():
        delete_set.add(k)
        k += 1

    orphan_set = set(orphan_helper_lines)
    new_lines: list[str] = []
    for i, line in enumerate(lines):
        if i in delete_set:
            continue
        if i == close_brace_line and fused_orphan_tail is not None:
            new_lines.append(content_indent + fused_orphan_tail.lstrip())
        elif i in orphan_set:
            new_lines.append(content_indent + line.lstrip())
        else:
            new_lines.append(line)
        if i == last_orphan:
            new_lines.append(closing_array)
            new_lines.append(closing_brace)

    script.write_text("\n".join(new_lines) + ("\n" if raw.endswith("\n") else ""), encoding="utf-8")

    total_orphans = len(orphan_helper_lines) + (1 if fused_orphan_tail else 0)
    return [
        f"自动修复: 章节函数被提前 `];}}` 闭合（原第 {close_array_line + 1}-{close_brace_line + 1} 行），"
        f"已把 {total_orphans} 行悬空内容并回 return 数组，"
        f"并删除末尾第 {iso_close + 1} 行的孤立 `]`"
    ]


_SURPLUS_BRACKET_RE = re.compile(r"(['\"`])\s*\)\s*\]\s*\)\s*[,;]?\s*$")


def _auto_fix_surplus_brackets(script: Path, env: dict[str, str]) -> list[str]:
    """Remove surplus ] or ) that cause 'Unexpected token' errors.

    Common LLM pattern: body('text')]),  →  body('text'),
    The model confuses string-argument body() with array-argument body([...]).

    Only triggers when node --check reports 'Unexpected token' on a specific
    line, to avoid false positives.
    """
    fixes: list[str] = []
    already_tried: set[int] = set()

    for _attempt in range(10):
        r = subprocess.run(
            [_get_node_exe(), "--check", str(script)],
            capture_output=True, text=True, env=env,
            encoding="utf-8",
        )
        if r.returncode == 0:
            break

        if "Unexpected token" not in r.stderr:
            break

        match = re.search(re.escape(str(script)) + r":(\d+)", r.stderr)
        if not match:
            match = re.search(re.escape(script.name) + r":(\d+)", r.stderr)
        if not match:
            break

        line_no = int(match.group(1))
        if line_no in already_tried:
            break
        already_tried.add(line_no)

        lines = script.read_text(encoding="utf-8").splitlines()
        if not (0 < line_no <= len(lines)):
            break

        line = lines[line_no - 1]
        new_line = _fix_call_close_bracket(line)
        if new_line != line:
            new_line = _fix_stray_call_paren(new_line)
            new_line = _fix_stray_paren_in_array(new_line)
            new_line = _fix_missing_array_close_in_call(new_line)
            new_line = _fix_surplus_close(new_line)
        else:
            new_line = _fix_stray_call_paren(line)
            if new_line == line:
                new_line = _fix_stray_paren_in_array(line)
            if new_line == line:
                new_line = _fix_missing_array_close_in_call(line)
            if new_line != line:
                new_line = _fix_surplus_close(new_line)
            if new_line == line:
                new_line = _fix_surplus_close(line)
        if new_line == line:
            break

        lines[line_no - 1] = new_line
        script.write_text("\n".join(lines) + "\n", encoding="utf-8")
        fixes.append(f"第 {line_no} 行：移除多余闭合符")

    return fixes


def _fix_surplus_close(line: str) -> str:
    """Fix body('...')]), → body('...'), by removing surplus ]).

    Skips lines where the function takes an array argument (contains '([')
    because body([...]), is a valid pattern.
    """
    stripped = line.rstrip()

    def _expected_closers(prefix: str) -> str:
        stack: list[str] = []
        in_str: str | None = None
        i = 0
        while i < len(prefix):
            ch = prefix[i]
            if in_str:
                if ch == "\\" and i + 1 < len(prefix):
                    i += 2
                    continue
                if ch == in_str:
                    in_str = None
                i += 1
                continue
            if ch in ("'", '"', "`"):
                in_str = ch
            elif ch == "/" and i + 1 < len(prefix) and prefix[i + 1] in ("/", "*"):
                break
            elif ch == "(":
                stack.append(")")
            elif ch == "[":
                stack.append("]")
            elif ch == "{":
                stack.append("}")
            elif ch in ")]}" and stack and stack[-1] == ch:
                stack.pop()
            i += 1
        return "".join(reversed(stack))

    def _trim_surplus_array_tail(s: str) -> str:
        # Generalised form of the concrete `])])` fix below.  For helper
        # array-argument lines, compute the exact closers still needed by the
        # prefix and discard any extra closers typed after that sequence.
        end = len(s)
        while end > 0 and s[end - 1].isspace():
            end -= 1
        sep = ""
        if end > 0 and s[end - 1] in ",;":
            sep = s[end - 1]
            end -= 1
        closer_end = end
        while end > 0 and s[end - 1] in ")]}":
            end -= 1
        closers = s[end:closer_end]
        if not closers:
            return s
        prefix = s[:end]
        expected = _expected_closers(prefix)
        if expected and closers != expected and closers.startswith(expected):
            return prefix + expected + sep
        return s

    if "([" in stripped:
        trimmed = _trim_surplus_array_tail(stripped)
        if trimmed != stripped:
            return trimmed

    # Pattern: h.bullet([h.bold('title', 'body')])]), → h.bullet([h.bold('title', 'body')]),
    # This is an array-argument line, so handle it before the general "skip
    # array args" guard below.  It appears after another fixer repairs the
    # missing inner ')' in the original shape:
    #   h.bullet([h.bold('title', 'body'])]),
    m0 = re.search(r"""\]\)\]\)(\s*[,;]?\s*)$""", stripped)
    if m0 and "([" in stripped:
        return stripped[:m0.start()] + "])" + m0.group(1)

    # Skip lines with array arguments: body([...]), is valid
    if "([" in stripped:
        return line

    # Pattern: body('text')]),  → the ]) between ) and , is surplus
    m = re.search(r"""(['"` ])\)\]\)(\s*[,;]?\s*)$""", stripped)
    if m:
        idx = m.start(0) + len(m.group(1))
        return stripped[:idx] + ")" + m.group(2)

    # Pattern: body('text']),  → the ] between ' and ) is surplus
    m2 = re.search(r"""(['"` ])\]\)(\s*[,;]?\s*)$""", stripped)
    if m2:
        idx = m2.start(0) + len(m2.group(1))
        return stripped[:idx] + ")" + m2.group(2)

    # Pattern: h.p('text')),  → double close-paren, outer ) is surplus
    # Most common LLM error in long paper JS files.
    # Guard: only fix if line has more ')' than '(' (surplus paren).
    m3 = re.search(r"""(['"}`])\)\)(\s*[,;]?\s*)$""", stripped)
    if m3 and stripped.count(")") > stripped.count("("):
        idx = m3.start(0) + len(m3.group(1))
        return stripped[:idx] + ")" + m3.group(2)

    return line


# ── Pre-flight checks (all run BEFORE node executes) ─────────────

def _check_syntax(script: Path, env: dict[str, str]) -> list[str]:
    """node --check + regex pre-scan for common LLM code-gen mistakes.

    node --check only reports ONE SyntaxError at a time (V8 limitation).
    We supplement it with regex-based scanning that catches multiple
    instances of frequent LLM mistakes in a single pass.
    """
    errors: list[str] = []
    source = script.read_text(encoding="utf-8")
    lines = source.splitlines()

    # --- Regex pre-scan: catch common LLM syntax mistakes in bulk ---
    _DECL_RE = re.compile(r"^\s*(?:const|let|var)\s")

    _LLM_SYNTAX_PATTERNS: list[tuple[re.Pattern, str, bool]] = [
        # cell="foo", width) instead of cell("foo", width)
        # Skip lines that are normal variable declarations (const/let/var)
        (re.compile(r'\b(\w+)\s*=\s*"[^"]*"\s*,\s*(?:\w+|\d+)'),
         "疑似函数调用写成了赋值（如 `cell=\"x\", w` → `cell(\"x\", w)`）",
         True),  # needs declaration filter
        # doubled opening brackets: arr[[0], func(("x", obj({{
        (re.compile(r'\w+\[\[(?=\d+\])'),
         "双方括号（如 `arr[[0]` → `arr[0]`）",
         False),
        (re.compile(r'\w+\(\((?=["\'])'),
         "双圆括号（如 `func((\"x\"` → `func(\"x\"`）",
         False),
        (re.compile(r'\w+\(\{\{'),
         "双大括号（如 `Table({{` → `Table({`）",
         False),
    ]
    for pat, desc, filter_decl in _LLM_SYNTAX_PATTERNS:
        for m in pat.finditer(source):
            line_no = source[:m.start()].count("\n") + 1
            line_text = lines[line_no - 1]
            if filter_decl and _DECL_RE.match(line_text):
                continue
            errors.append(f"第 {line_no} 行：{desc}\n    {line_text.strip()}")

    # --- node --check: authoritative syntax validation ---
    r = subprocess.run(
        [_get_node_exe(), "--check", str(script)],
        capture_output=True, text=True, env=env,
        encoding="utf-8",
    )
    if r.returncode != 0:
        errors.append(_extract_error_context(script, r.stderr.strip()))

    return errors


def _check_modules(script: Path, env: dict[str, str]) -> list[str]:
    """Verify all require()'d modules can be resolved."""
    source = script.read_text(encoding="utf-8")
    modules = set(re.findall(r"""require\s*\(\s*['"]([^'"./][^'"]*)['"]\s*\)""", source))
    if not modules:
        return []

    checks = "; ".join(
        f'try {{ require.resolve("{m}"); }} catch(e) {{ bad.push("{m}"); }}'
        for m in sorted(modules)
    )
    code = f'const bad = []; {checks} if (bad.length) {{ console.log(JSON.stringify(bad)); process.exit(1); }}'
    r = subprocess.run(
        [_get_node_exe(), "-e", code],
        capture_output=True, text=True, env=env,
        encoding="utf-8",
    )
    if r.returncode != 0:
        missing_modules: list[str] = []
        try:
            parsed = json.loads(r.stdout.strip())
            if isinstance(parsed, list):
                missing_modules = [m for m in parsed if isinstance(m, str)]
        except Exception:
            pass
        if not missing_modules:
            missing_modules = _extract_missing_modules((r.stderr or "") + "\n" + (r.stdout or ""))
        if not missing_modules:
            missing_modules = sorted(modules)
        return [_missing_modules_hint(missing_modules, env)]
    return []


def _check_file_paths(script: Path) -> list[str]:
    """Verify files referenced in readFileSync/writeFileSync actually exist or are writable."""
    source = script.read_text(encoding="utf-8")
    errors = []

    for m in re.finditer(r"""readFileSync\s*\(\s*['"]([^'"]+)['"]\s*\)""", source):
        fpath = m.group(1)
        if not Path(fpath).exists():
            line_no = source[:m.start()].count("\n") + 1
            errors.append(f"第 {line_no} 行：readFileSync 引用的文件不存在: {fpath}")

    for m in re.finditer(r"""writeFileSync\s*\(\s*['"]([^'"]+)['"]\s*""", source):
        fpath = m.group(1)
        parent = Path(fpath).parent
        if not parent.exists():
            line_no = source[:m.start()].count("\n") + 1
            errors.append(f"第 {line_no} 行：writeFileSync 目标目录不存在: {parent}")

    return errors


_BAD_PATTERNS: list[tuple[str, str, str]] = [
    # Patterns NOT auto-fixed (require human/model decision)
    (r"new\s+Document\s*\(\s*\)", "Document() 缺少参数", "至少传 { sections: [...] }"),
    (r'writeFileSync\s*\([^)]*,\s*doc\s*[,)]', "不能直接写 Document 对象", "用 Packer.toBuffer(doc) 转为 Buffer 再写"),
    # Note: BorderStyle.NONE, ShadingType.SOLID, HeadingLevel.HEADING1, AlignmentType.JUSTIFY
    # are now auto-fixed by _fix_bad_enums() and no longer checked here.
]


def _check_patterns(script: Path) -> list[str]:
    """Detect common docx-js mistakes via pattern matching."""
    source = script.read_text(encoding="utf-8")
    errors = []
    for pattern, desc, fix in _BAD_PATTERNS:
        if not desc:
            continue
        for m in re.finditer(pattern, source):
            line_no = source[:m.start()].count("\n") + 1
            errors.append(f"第 {line_no} 行：{desc}。{fix}")
    return errors


def _check_table_widths(script: Path) -> list[str]:
    """Check if columnWidths arrays have wildly inconsistent sums.

    Tolerance is 1000 twips (~1.7cm). Small overflows are harmless.
    Searches for the table ``width`` declaration within the same
    ``new Table({`` constructor to avoid confusing cell widths or
    outer-table widths with the current table's width.
    """
    source = script.read_text(encoding="utf-8")
    errors = []

    page_cfg_match = re.search(
        r"page\s*:\s*\{[^}]*width\s*:\s*(\d+)", source
    )
    default_content_width = 9360
    if page_cfg_match:
        page_width = int(page_cfg_match.group(1))
        margins_match = re.search(
            r"margins\s*:\s*\{[^}]*(?:left|right)\s*:\s*(\d+)", source
        )
        margin = int(margins_match.group(1)) if margins_match else 1440
        default_content_width = page_width - margin * 2

    for m in re.finditer(r"columnWidths\s*:\s*\[([^\]]+)\]", source):
        try:
            widths = [int(x.strip()) for x in m.group(1).split(",") if x.strip().isdigit()]
            if not widths:
                continue
            total = sum(widths)
            line_no = source[:m.start()].count("\n") + 1

            window_start = max(0, m.start() - 400)
            window = source[window_start:m.start()]

            table_start = None
            for tm in re.finditer(r"new\s+Table\s*\(\s*\{", window):
                table_start = tm.start()

            if table_start is not None:
                table_window = window[table_start:]
                w_match = re.search(
                    r"width\s*:\s*\{\s*size\s*:\s*(\d+)", table_window
                )
                if w_match:
                    table_width = int(w_match.group(1))
                elif re.search(r"width\s*:\s*\{[^}]*contentWidth", table_window):
                    table_width = default_content_width
                elif re.search(r"width\s*:\s*\{", table_window):
                    continue
                else:
                    table_width = default_content_width
            else:
                table_width = default_content_width

            if abs(total - table_width) > 1000:
                errors.append(
                    f"第 {line_no} 行：columnWidths 之和 ({total}) 与表格宽度 ({table_width}) 差距过大，"
                    f"调整 widths 或改用 h.table() 自动缩放"
                )
        except (ValueError, IndexError):
            pass
    return errors


def _check_table_row_cells(script: Path) -> list[str]:
    """Detect rows where cell count (incl. columnSpan) doesn't match columnWidths.

    Pure-Python regex approach targeting the most common LLM mistake:
    using verticalMerge + .map() where the data array has one element too many.

    Handles:
      - Inline columnWidths: ``columnWidths: [w1, w2, ...]``
      - Variable references: ``columnWidths: compColW`` (resolves ``const compColW = [...]``)
      - columnSpan in any argument object: ``cell("x", w, {columnSpan: 5})``
      - .map() data rows: ``...([["a","b"], ...].map((row) => new TableRow({...})))``
    """
    source = script.read_text(encoding="utf-8")
    lines = source.splitlines()
    errors: list[str] = []

    # ① Resolve variable → array element count
    var_counts: dict[str, int] = {}
    for m in re.finditer(r"(?:const|let|var)\s+(\w+)\s*=\s*\[([^\]]*)\]", source):
        items = [x.strip() for x in m.group(2).split(",") if x.strip()]
        var_counts[m.group(1)] = len(items)

    # ② Collect columnWidths with line number + column count
    cw_list: list[tuple[int, int]] = []
    for m in re.finditer(r"columnWidths\s*:\s*(?:\[([^\]]+)\]|(\w+))", source):
        line_no = source[: m.start()].count("\n") + 1
        if m.group(1):
            n = len([x.strip() for x in m.group(1).split(",") if x.strip()])
            cw_list.append((line_no, n))
        elif m.group(2) in var_counts:
            cw_list.append((line_no, var_counts[m.group(2)]))

    if not cw_list:
        return errors

    # ③ Find .map() blocks that create TableRow from data arrays
    #    Supports:  [["a","b"], ...].map(   and   ...([["a","b"], ...].map(
    map_re = re.compile(
        r"(?:\.\.\.\()?\s*\[\s*\n"
        r"(?P<rows>(?:[ \t]*\[[^\n\[\]]*\],?\n)+)"
        r"[ \t]*\]\.map\(",
    )

    for dm in map_re.finditer(source):
        map_line = source[: dm.start()].count("\n") + 1

        # Count elements in the first sub-array
        first = re.search(r"\[([^\[\]]+)\]", dm.group("rows"))
        if not first:
            continue
        data_cols = len([x.strip() for x in first.group(1).split(",") if x.strip()])

        # Scan the children block after .map( for explicit cells + spread
        block = source[dm.end() : dm.end() + 1200]
        explicit = 0
        has_spread = False
        col_span_total = 0
        for bline in block.split("\n"):
            s = bline.strip()
            if s.startswith("...") and ".map(" in s:
                has_spread = True
                break
            if any(kw in s for kw in ("new TableCell(", "makeCell(", "headCell(", "cell(")):
                # Check for columnSpan in this line
                cs_m = re.search(r"columnSpan\s*:\s*(\d+)", s)
                if cs_m:
                    col_span_total += int(cs_m.group(1))
                else:
                    explicit += 1
            if re.match(r"\s*\]\s*[,})\]]", bline):
                break

        if not has_spread:
            continue

        total = explicit + col_span_total + data_cols

        # Find the nearest preceding columnWidths
        col_count = None
        cw_line = None
        for cw_l, cw_n in reversed(cw_list):
            if cw_l < map_line:
                col_count = cw_n
                cw_line = cw_l
                break

        if col_count is None:
            continue

        if total != col_count:
            expect_data_cols = max(0, col_count - explicit - col_span_total)
            errors.append(
                f"第 {map_line} 行：TableRow 有 {total} 个 cell 但表格声明 {col_count} 列"
                f"（columnWidths 第 {cw_line} 行），数据数组应约 {expect_data_cols} 列"
            )

    return errors


def _check_raw_table_with_helper_params(script: Path) -> list[str]:
    """Detect ``new Table({...header:...})`` — mixing raw Table constructor with h.table() params.

    The raw ``Table`` constructor expects ``rows: [TableRow(...), ...]`` whereas
    ``h.table()`` accepts ``header: [...]`` / ``rows: [['a','b'], ...]``.  Using
    ``new Table`` with helper-style params causes runtime ``TypeError``.
    """
    source = script.read_text(encoding="utf-8")
    errors: list[str] = []

    for m in re.finditer(r"new\s+Table\s*\(\s*\{", source):
        line_no = source[: m.start()].count("\n") + 1
        block = source[m.end(): m.end() + 600]
        brace_depth = 1
        end_idx = 0
        for i, ch in enumerate(block):
            if ch == "{":
                brace_depth += 1
            elif ch == "}":
                brace_depth -= 1
                if brace_depth == 0:
                    end_idx = i
                    break
        top_block = block[:end_idx]

        helper_keys = re.findall(r"^\s*(header|headerColor|altColor|widths)\s*:", top_block, re.MULTILINE)
        if helper_keys:
            keys_str = ", ".join(sorted(set(helper_keys)))
            errors.append(
                f"第 {line_no} 行：new Table() 混用了 h.table() 参数（{keys_str}），"
                f"改为 h.table({{widths, header, rows}})"
            )
    return errors


_HELPER_RICH_VALUE_METHODS = (
    "text",
    "bold",
    "italic",
    "p",
    "paragraph",
    "bullet",
    "numbered",
    "link",
    "image",
    "bookmark",
)


def _check_helper_object_string_concat(script: Path) -> list[str]:
    """Detect docx-helper objects being concatenated with strings.

    Helpers such as h.bold()/h.text()/h.p() return TextRun/Paragraph objects
    (or arrays of runs). JavaScript ``+`` coerces them to strings like
    ``[object Object]`` or ``[object Object],[object Object]``. This often
    still generates a DOCX, but table cells or paragraphs show corrupted/empty
    content, so fail early with an explicit remediation.
    """
    raw_source = script.read_text(encoding="utf-8")
    source = _mask_strings_and_comments(raw_source)

    helper_vars = {"h"}
    for m in _HELPER_REQUIRE_RE.finditer(raw_source):
        helper_vars.add(m.group(1))

    methods = "|".join(re.escape(m) for m in _HELPER_RICH_VALUE_METHODS)
    errors: list[str] = []
    seen: set[int] = set()

    for helper in sorted(helper_vars):
        helper_call = rf"\b{re.escape(helper)}\s*\.\s*(?:{methods})\s*\("
        patterns = [
            re.compile(helper_call + r"[\s\S]{0,500}?\)\s*\+"),
            re.compile(r"\+\s*" + helper_call),
        ]
        for pat in patterns:
            for m in pat.finditer(source):
                # Require an actual + near the raw match. The masked match
                # only proves structure; the raw text provides the readable sample.
                line_no = raw_source[: m.start()].count("\n") + 1
                if line_no in seen:
                    continue
                seen.add(line_no)
                line = raw_source.splitlines()[line_no - 1].strip()
                preview = line if len(line) <= 110 else line[:107] + "..."
                errors.append(
                    f"第 {line_no} 行：不要用 `+` 拼接 {helper}.bold()/text()/p() 等富文本对象，"
                    "会在 DOCX 中变成 [object Object] 或导致表格单元格显示异常。"
                    " 改为数组 children/runs，例如 "
                    f"`{helper}.p([{helper}.text('股票(代码)', {{ bold: true }}), {helper}.text(' 说明')])`。"
                    f" 当前行: {preview}"
                )

    return errors



def _extract_first_call_arg(source: str, call_open_paren_idx: int) -> tuple[str, int]:
    """Extract the first argument of a call given the index of its opening `(`.

    Returns (arg_text, end_index_exclusive). Handles nested brackets, string
    literals (single/double/backtick) and escape sequences. Returns ("", idx)
    if the call has no argument before its closing paren.
    """
    n = len(source)
    if call_open_paren_idx >= n or source[call_open_paren_idx] != "(":
        return "", call_open_paren_idx
    i = call_open_paren_idx + 1
    start = i
    depth = 1
    while i < n:
        ch = source[i]
        if ch in ("'", '"', "`"):
            quote = ch
            i += 1
            while i < n:
                if source[i] == "\\" and i + 1 < n:
                    i += 2
                    continue
                if source[i] == quote:
                    i += 1
                    break
                i += 1
            continue
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth == 0:
                return source[start:i].strip(), i
        elif ch == "," and depth == 1:
            return source[start:i].strip(), i
        i += 1
    return source[start:i].strip(), i


_HELPER_REQUIRE_RE = re.compile(
    r"""(?:const|let|var)\s+(\w+)\s*=\s*require\s*\([^)]*docx-helper[^)]*\)""",
)


def _mask_strings_and_comments(source: str) -> str:
    """Replace string literals and comments with spaces (preserving offsets).

    Used to prevent regex scanners from matching keywords inside comments or
    string content (e.g. ``// 漏写了 h.build()`` should not trigger the
    "missing spec arg" warning).
    """
    n = len(source)
    out = list(source)
    i = 0
    while i < n:
        ch = source[i]
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            j = source.find("\n", i)
            end = j if j != -1 else n
            for k in range(i, end):
                out[k] = " " if source[k] != "\n" else "\n"
            i = end
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            j = source.find("*/", i + 2)
            end = j + 2 if j != -1 else n
            for k in range(i, end):
                out[k] = " " if source[k] != "\n" else "\n"
            i = end
            continue
        if ch in ("'", '"', "`"):
            quote = ch
            j = i + 1
            while j < n:
                if source[j] == "\\" and j + 1 < n:
                    j += 2
                    continue
                if source[j] == quote:
                    j += 1
                    break
                j += 1
            # 保留首尾引号，内部内容置空 → _extract_first_call_arg 仍能识别"以引号开头"
            for k in range(i + 1, j - 1 if j - 1 > i else j):
                out[k] = " " if source[k] != "\n" else "\n"
            i = j
            continue
        i += 1
    return "".join(out)


_TOP_LEVEL_LEGAL_RE = re.compile(
    r"^\s*("
    r"//|/\*|\*|"                                         # 注释行
    r"(const|let|var|function|async|class|"               # 声明 / 函数 / 类
    r"if|else|switch|case|default|"                       # 控制流
    r"for|while|do|try|catch|finally|"
    r"return|break|continue|throw|"
    r"require|import|export|module|process|console|"      # 模块 / 内置
    r"global|globalThis|delete|void|typeof|new|"
    r"await|yield|debugger)\b"
    r")"
)


def _check_orphan_top_level_code(script: Path) -> list[str]:
    """Detect orphan content-generation statements at module top level.

    Common failure mode from streaming append: while writing chapter3 the
    model prematurely writes the function tail ``];`` + ``}``, then later
    appended chunks (3.4 / 3.5 / ...) end up *outside* any function as
    orphan expression statements like::

        h.p('3.4 内容...'),
        h.h1('3.5 小节'),
        ]

    These cause a SyntaxError far below the structural root cause (Node
    reports e.g. line 218 `Unexpected token ']'` but the bug is at line
    110 where chapter3 was closed too early). The model then wastes
    20-50 rounds chasing the wrong location, often rewriting the whole
    file and introducing fresh typos (``h.hullet`` etc.).

    We catch this with three structural signals at module-top-level
    (bracket depth == 0):
      ① helper calls like ``h.p(...)`` (not the build/createDoc whitelist)
      ② any line ending with ``,`` (top-level statements should end with ``;``)
      ③ isolated closing brackets ``]`` / ``}`` / ``)`` on their own line
    Plus a global signal: final cumulative bracket depth != 0.
    """
    raw_source = script.read_text(encoding="utf-8")
    masked = _mask_strings_and_comments(raw_source)
    raw_lines = raw_source.splitlines()
    masked_lines = masked.splitlines()

    # 计算每行起始时的括号累计深度
    starts: list[int] = []
    depth = 0
    for line in masked_lines:
        starts.append(depth)
        for ch in line:
            if ch in "{[(":
                depth += 1
            elif ch in "}])":
                depth -= 1

    orphans: list[tuple[int, str, str]] = []  # (line_no, raw_text, reason)

    for i, (mline, raw_line, ds) in enumerate(zip(masked_lines, raw_lines, starts)):
        if ds != 0:
            continue
        stripped_m = mline.strip()
        if not stripped_m:
            continue
        if _TOP_LEVEL_LEGAL_RE.match(mline):
            continue
        # 顶层赋值：foo = ..., foo.bar = ..., foo[bar] = ...
        if re.match(r"^\s*[\w$]+(\s*\.\s*[\w$]+|\s*\[[^\]]*\])*\s*(=(?!=)|\+=|-=|\*=|/=|\|\|=|&&=|\?\?=)", mline):
            continue
        # 收尾型：}); / )); / 单独 }; / 单独 ; / 单独 ()
        if re.fullmatch(r"\s*[)};\s]+", mline):
            continue
        if re.fullmatch(r"\s*\(\)\s*;?\s*", mline):
            continue
        # 析构赋值等：{ a, b } = ... 起始是 { —— 在顶层但通常 depth 已经不为 0，先不特判

        # ① 行末是逗号 → 顶层表达式不完整（数组/参数列表元素掉到了函数外）
        if stripped_m.endswith(","):
            orphans.append((i + 1, raw_line.rstrip(), "顶层行末逗号（属于某个数组/参数列表的元素，但已脱离了函数）"))
            continue

        # ② 孤立闭合符行（仅含 ] }] ]); 等）
        if re.fullmatch(r"\s*[\])\}][\s\])\};,]*\s*", mline):
            orphans.append((i + 1, raw_line.rstrip(), "孤立的闭合符（顶层不应有未配对的 ] } )）"))
            continue

        # ③ 顶层 helper 内容调用：h.p(...) / h.h1(...) / h.bullet(...) 等
        m = re.match(r"^\s*([\w$]+)\s*\.\s*([\w$]+)\s*\(", mline)
        if m:
            helper, method = m.group(1), m.group(2)
            # 已知合法顶层调用：build / createDoc / 用户自定义工具函数返回结果赋值（已被上面赋值规则过滤）
            if method in {"build", "createDoc"}:
                continue
            # 跳过 console / process / module / global 上的调用（已被 legal 正则过滤但双保险）
            if helper in {"console", "process", "module", "global", "globalThis", "Object", "Array", "JSON", "Math"}:
                continue
            orphans.append((
                i + 1,
                raw_line.rstrip(),
                f"顶层裸调用 {helper}.{method}() —— 内容生成函数必须放在某个 section 的 children 数组里",
            ))
            continue

    errors: list[str] = []

    if orphans:
        first_line = orphans[0][0]
        last_line = orphans[-1][0]
        line_range = f"第 {first_line} 行" if first_line == last_line else f"第 {first_line}-{last_line} 行"
        sample = "\n".join(
            f"      {ln:4d} | {text[:100]}"
            for ln, text, _reason in orphans[:5]
        )
        more = f"\n      ...（还有 {len(orphans) - 5} 处）" if len(orphans) > 5 else ""
        read_start = max(1, first_line - 8)
        errors.append(
            f"{line_range}：{len(orphans)} 处悬空代码 — 某章节函数被提前 `];}}` 闭合，后续内容掉到函数外。\n"
            f"{sample}{more}\n"
            f"    检查第 {read_start}-{first_line} 行，找到提前闭合的 `];` 或 `}}`，将其移到章节末尾。"
            f"优先修此错误而非 SyntaxError。"
        )

    if depth != 0:
        sign = "多" if depth > 0 else "少"
        errors.append(
            f"括号不平衡：`{{` `(` `[` 比 `}}` `)` `]` {sign} {abs(depth)} 个"
        )

    return errors


def _check_build_call(script: Path) -> list[str]:
    """Statically verify h.build() is called and called correctly.

    This is the cheapest, most effective guard against the #1 source of
    "silent success" bugs: the model generates a JS script that never
    actually triggers a writeFileSync. Even if h.build() throws at runtime,
    docx-helper.js now sets process.exitCode=1, but pre-flight catches it
    one round earlier with a precise diagnosis.
    """
    raw_source = script.read_text(encoding="utf-8")
    # 屏蔽注释和字符串内容，避免 `// h.build()` 之类的注释被误识别
    source = _mask_strings_and_comments(raw_source)
    errors: list[str] = []

    helper_var = "h"
    # require 检测必须在 raw_source 上做——mask 会把字符串 'docx-helper' 替换掉，
    # 导致 path.join(SKILL_PATH, 'docx', 'scripts', 'docx-helper') 写法被误判。
    m = _HELPER_REQUIRE_RE.search(raw_source)
    if m:
        helper_var = m.group(1)

    # 检查 helper 变量是否有定义（require 语句存在）
    # 如果文件中使用了 h.xxx 但没有 require docx-helper，运行时必定 ReferenceError
    if not m:
        any_helper_use = re.search(rf"\bh\.\w+\s*\(", source)
        if any_helper_use:
            errors.append(
                "使用了 h.xxx() 但缺少 require('docx-helper')，运行时将报 ReferenceError。"
                " 在文件开头添加 docx-helper 的 require 初始化。"
            )
            return errors

    call_pattern = re.compile(rf"\b{re.escape(helper_var)}\.build\s*\(")
    calls = list(call_pattern.finditer(source))

    if not calls:
        errors.append(
            f"未找到 {helper_var}.build() 调用，没有 build 就不会生成 DOCX。"
            f" 在文件末尾追加 {helper_var}.build({{ sections: [...] }})。"
        )
        return errors

    # 设计原则：
    # - "无法被 JS 端容错"的 case → 加入 errors（preflight 阻塞执行，强制模型修复）
    # - "能被 JS 端容错"的 case → 不报错，docx-helper.js 会自动归一化并 console.warn 给模型看
    for call in calls:
        line_no = source[: call.start()].count("\n") + 1
        first_arg_masked, arg_end = _extract_first_call_arg(source, call.end() - 1)
        raw_arg = raw_source[call.end():arg_end].strip()

        if not first_arg_masked:
            errors.append(
                f"第 {line_no} 行：{helper_var}.build() 缺少参数，应为 {helper_var}.build({{ sections: [...] }})"
            )
            continue

        # 第一个参数是对象字面量但既无 sections 也无 children 也无 spec → 完全识别不出
        # （JS 端能识别 {sections}、{children}、{spec}；其他对象会 throw）
        if (
            first_arg_masked.startswith("{")
            and "sections" not in first_arg_masked
            and "children" not in first_arg_masked
            and "spec" not in first_arg_masked
        ):
            preview = raw_arg if len(raw_arg) <= 80 else raw_arg[:77] + "..."
            errors.append(
                f"第 {line_no} 行：{helper_var}.build() 参数缺少 sections 或 children 字段。"
                f" 实际: {preview}"
            )
            continue

        # 其他写法（字符串路径在前、路径变量在前、单 patch、纯数组等）一律放行——
        # docx-helper.js 的 _normalizeBuildArgs 会自动归一化并 console.warn 提示。

    return errors


def preflight(script_path: str | Path) -> list[str]:
    """Run all pre-flight checks on a JS file. Returns list of error strings (empty = all good)."""
    script = Path(script_path).expanduser().resolve()
    if not script.exists():
        return [f"JS 文件不存在：{script}"]

    env = _with_global_node_path()
    errors: list[str] = []

    # Phase 1: syntax
    syntax_errors = _check_syntax(script, env)
    errors.extend(syntax_errors)

    # Phase 2: modules, files, patterns (all independent, run all)
    # Module check needs node to import — skip when syntax is broken
    if not syntax_errors:
        errors.extend(_check_modules(script, env))
    errors.extend(_check_table_row_cells(script))
    errors.extend(_check_raw_table_with_helper_params(script))
    errors.extend(_check_helper_object_string_concat(script))
    # Regex-based checks work regardless of syntax validity
    errors.extend(_check_file_paths(script))
    errors.extend(_check_patterns(script))
    errors.extend(_check_table_widths(script))
    errors.extend(_check_orphan_top_level_code(script))
    errors.extend(_check_build_call(script))

    return errors


# ── Post-processing: fix docx.js bugs in generated files ─────────

def _fix_bookmark_ids_in_xml(content: str) -> tuple[str, int]:
    """Fix duplicate w:id on bookmarkStart/bookmarkEnd (docx.js v9.x bug).

    docx.js assigns w:id="1" to every Bookmark.  This reassigns unique
    incremental IDs while preserving start/end pairing via document order.

    Returns (fixed_content, num_unique_bookmarks).
    """
    start_tag_re = re.compile(r"<w:bookmarkStart\b[^/]*?/>")
    id_re = re.compile(r'w:id="(\d+)"')
    name_re = re.compile(r'w:name="([^"]*)"')

    starts = list(start_tag_re.finditer(content))
    if len(starts) <= 1:
        return content, 0

    old_ids = []
    for m in starts:
        id_m = id_re.search(m.group())
        if id_m:
            old_ids.append(id_m.group(1))
    if len(set(old_ids)) == len(old_ids):
        return content, 0

    name_to_id: dict[str, str] = {}
    counter = 0
    for m in starts:
        name_m = name_re.search(m.group())
        name = name_m.group(1) if name_m else f"_bm{counter}"
        if name not in name_to_id:
            name_to_id[name] = str(counter)
            counter += 1

    all_tags = list(re.finditer(
        r"<w:bookmarkStart\b[^/]*?/>|<w:bookmarkEnd\b[^/]*?/>", content,
    ))

    replacements: list[tuple[int, int, str]] = []
    stack: list[str] = []
    for tag_m in all_tags:
        tag = tag_m.group()
        if "bookmarkStart" in tag:
            nm = name_re.search(tag)
            name = nm.group(1) if nm else f"_bm{len(replacements)}"
            new_id = name_to_id.get(name, str(counter))
            replacements.append((tag_m.start(), tag_m.end(), id_re.sub(f'w:id="{new_id}"', tag)))
            stack.append(new_id)
        else:
            new_id = stack.pop() if stack else "0"
            replacements.append((tag_m.start(), tag_m.end(), id_re.sub(f'w:id="{new_id}"', tag)))

    result = content
    for start, end, new_tag in reversed(replacements):
        result = result[:start] + new_tag + result[end:]

    return result, len(name_to_id)


def _fix_docx_bookmark_ids(docx_path: Path) -> int:
    """Fix duplicate bookmark IDs in a DOCX file in-place. Returns count of unique bookmarks fixed."""
    import tempfile
    import zipfile

    modified: dict[str, str] = {}
    total = 0

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            if not name.endswith(".xml"):
                continue
            raw = zin.read(name).decode("utf-8")
            if "bookmarkStart" not in raw:
                continue
            fixed, count = _fix_bookmark_ids_in_xml(raw)
            if count > 0:
                modified[name] = fixed
                total += count

    if not modified:
        return 0

    tmp_fd, tmp_path = tempfile.mkstemp(suffix=".docx")
    os.close(tmp_fd)
    try:
        with zipfile.ZipFile(docx_path, "r") as zin:
            with zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
                for item in zin.infolist():
                    if item.filename in modified:
                        zout.writestr(item, modified[item.filename])
                    else:
                        zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return total


def _fix_docx_update_fields(docx_path: Path) -> bool:
    """Ensure <w:updateFields w:val="true"/> in settings.xml for documents with TOC fields.

    docx-js generates TOC field instructions but cannot pre-render entries.
    By setting updateFields w:val="true", Word/WPS will auto-refresh all
    fields (including TOC) when the document is opened.

    Also fixes a docx-js bug: ``features: { updateFields: true }`` generates
    a bare ``<w:updateFields/>`` tag without ``w:val="true"``, which WPS
    ignores.  This function replaces such bare tags with the correct form.

    Returns True if settings.xml was modified.
    """
    import zipfile

    CORRECT_TAG = '<w:updateFields w:val="true"/>'

    has_toc = False
    settings_name: str | None = None
    settings_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            raw = zin.read(name).decode("utf-8", errors="replace")
            if "TOC " in raw or ("w:sdt" in raw and "TOC" in raw):
                has_toc = True
            if name.endswith("settings.xml"):
                settings_name = name
                settings_raw = raw

    if not has_toc or settings_name is None or settings_raw is None:
        return False

    if 'w:val="true"' in settings_raw and "updateFields" in settings_raw:
        return False

    bare_tag_re = re.compile(r"<w:updateFields\s*/?>")
    if bare_tag_re.search(settings_raw):
        new_raw = bare_tag_re.sub(CORRECT_TAG, settings_raw)
    else:
        insert_after = re.search(r"<w:settings\b[^>]*>", settings_raw)
        if not insert_after:
            return False
        pos = insert_after.end()
        new_raw = settings_raw[:pos] + CORRECT_TAG + settings_raw[pos:]

    if new_raw == settings_raw:
        return False

    tmp_path = str(docx_path) + ".tmp"
    try:
        with zipfile.ZipFile(docx_path, "r") as zin, \
             zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == settings_name:
                    zout.writestr(item, new_raw.encode("utf-8"))
                else:
                    zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return True


def _remove_update_fields(docx_path: Path) -> bool:
    """Remove <w:updateFields .../> from settings.xml.

    Called after TOC cached entries are successfully populated — the cached
    content is sufficient for display and updateFields would cause WPS to
    clear the cache and attempt a live rebuild that often fails.
    """
    import zipfile

    settings_name: str | None = None
    settings_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            if name.endswith("settings.xml"):
                settings_name = name
                settings_raw = zin.read(name).decode("utf-8", errors="replace")
                break

    if settings_name is None or settings_raw is None:
        return False

    tag_re = re.compile(r"<w:updateFields[^/>]*/?>")
    if not tag_re.search(settings_raw):
        return False

    new_raw = tag_re.sub("", settings_raw)
    if new_raw == settings_raw:
        return False

    tmp_path = str(docx_path) + ".tmp"
    try:
        with zipfile.ZipFile(docx_path, "r") as zin, \
             zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == settings_name:
                    zout.writestr(item, new_raw.encode("utf-8"))
                else:
                    zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return True


_TOC_STYLES_XML = """\
<w:style w:type="paragraph" w:styleId="TOC1" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:name w:val="toc 1"/>
  <w:basedOn w:val="Normal"/>
  <w:uiPriority w:val="39"/>
  <w:pPr>
    <w:spacing w:before="120" w:after="0" w:line="360" w:lineRule="auto"/>
  </w:pPr>
  <w:rPr><w:b/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr>
</w:style>
<w:style w:type="paragraph" w:styleId="TOC2" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:name w:val="toc 2"/>
  <w:basedOn w:val="Normal"/>
  <w:uiPriority w:val="39"/>
  <w:pPr>
    <w:spacing w:before="0" w:after="0" w:line="360" w:lineRule="auto"/>
    <w:ind w:left="420"/>
  </w:pPr>
  <w:rPr><w:sz w:val="21"/><w:szCs w:val="21"/></w:rPr>
</w:style>
<w:style w:type="paragraph" w:styleId="TOC3" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:name w:val="toc 3"/>
  <w:basedOn w:val="Normal"/>
  <w:uiPriority w:val="39"/>
  <w:pPr>
    <w:spacing w:before="0" w:after="0" w:line="360" w:lineRule="auto"/>
    <w:ind w:left="840"/>
  </w:pPr>
  <w:rPr><w:sz w:val="21"/><w:szCs w:val="21"/></w:rPr>
</w:style>"""


def _fix_docx_toc_styles(docx_path: Path) -> bool:
    """Inject TOC 1/2/3 paragraph styles with indentation into styles.xml.

    docx-js generates TOC field instructions but does not include the
    ``TOC 1`` / ``TOC 2`` / ``TOC 3`` paragraph styles.  Word has built-in
    defaults with indentation, but WPS does not — causing all TOC levels
    to render at the same indent.  This injects explicit styles so that
    sub-headings are properly indented in all word processors.
    """
    import zipfile

    has_toc = False
    styles_name: str | None = None
    styles_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            raw = zin.read(name).decode("utf-8", errors="replace")
            if "TOC " in raw or ("w:sdt" in raw and "TOC" in raw):
                has_toc = True
            if name.endswith("styles.xml"):
                styles_name = name
                styles_raw = raw

    if not has_toc or styles_name is None or styles_raw is None:
        return False

    if re.search(r'w:styleId="TOC[123]"', styles_raw):
        return False

    close_tag = "</w:styles>"
    if close_tag not in styles_raw:
        return False

    new_raw = styles_raw.replace(close_tag, _TOC_STYLES_XML + close_tag)

    tmp_path = str(docx_path) + ".tmp"
    try:
        with zipfile.ZipFile(docx_path, "r") as zin, \
             zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == styles_name:
                    zout.writestr(item, new_raw.encode("utf-8"))
                else:
                    zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return True


def _fix_docx_heading_outline_levels(docx_path: Path) -> bool:
    """Add missing outlineLvl to Heading styles so TOC field evaluation works.

    docx-js defines Heading1–6 styles but omits ``<w:outlineLvl>`` in their
    ``<w:pPr>``.  The TOC ``\\o`` switch relies on outline levels to collect
    entries; without them WPS reports "未找到目录项" when the user manually
    updates the field.  This injects the correct outline levels.
    """
    import zipfile

    has_toc = False
    styles_name: str | None = None
    styles_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            raw = zin.read(name).decode("utf-8", errors="replace")
            if "TOC " in raw or ("w:sdt" in raw and "TOC" in raw):
                has_toc = True
            if name.endswith("styles.xml"):
                styles_name = name
                styles_raw = raw

    if not has_toc or styles_name is None or styles_raw is None:
        return False

    heading_re = re.compile(
        r'(<w:style\b[^>]*w:styleId="Heading(\d)"[^>]*>)'
        r'(.*?)'
        r'(</w:style>)',
        re.DOTALL,
    )

    patched = False
    new_raw = styles_raw

    for m in heading_re.finditer(styles_raw):
        level = int(m.group(2))
        body = m.group(3)
        if "outlineLvl" in body:
            continue

        outline_tag = f'<w:outlineLvl w:val="{level - 1}"/>'

        ppr_match = re.search(r"(<w:pPr\b[^>]*>)(.*?)(</w:pPr>)", body, re.DOTALL)
        if ppr_match:
            new_body = body.replace(
                ppr_match.group(3),
                outline_tag + ppr_match.group(3),
                1,
            )
        else:
            new_body = f"<w:pPr>{outline_tag}</w:pPr>" + body

        old_block = m.group(0)
        new_block = m.group(1) + new_body + m.group(4)
        new_raw = new_raw.replace(old_block, new_block, 1)
        patched = True

    if not patched:
        return False

    tmp_path = str(docx_path) + ".tmp"
    try:
        with zipfile.ZipFile(docx_path, "r") as zin, \
             zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == styles_name:
                    zout.writestr(item, new_raw.encode("utf-8"))
                else:
                    zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return True


_WORD_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
_XML_NS = "http://www.w3.org/XML/1998/namespace"
_HEADING_STYLE_RE = re.compile(r"Heading([1-9])$")
_TOC_RANGE_RE = re.compile(r'\\o\s+"(\d+)-(\d+)"')


def _w_tag(local_name: str) -> str:
    return f"{{{_WORD_NS}}}{local_name}"


_ILLEGAL_XML_CHARS_RE = re.compile(
    r"[\x00-\x08\x0b\x0c\x0e-\x1f\ufffe\uffff]"
)


def _strip_illegal_xml_chars(xml_text: str) -> str:
    """Remove characters illegal in XML 1.0 from a string.

    OMML generated from inline LaTeX math ($...$) may inject control characters
    like U+0008 (backspace) that cause ET.fromstring() to raise ParseError.
    """
    return _ILLEGAL_XML_CHARS_RE.sub("", xml_text)


def _register_xml_namespaces(xml_text: str) -> None:
    import io
    import xml.etree.ElementTree as ET

    for _, (prefix, uri) in ET.iterparse(
        io.BytesIO(xml_text.encode("utf-8")),
        events=("start-ns",),
    ):
        if prefix == "xml":
            continue
        try:
            ET.register_namespace(prefix, uri)
        except ValueError:
            continue


def _get_heading_level(paragraph) -> int | None:
    ppr = paragraph.find(_w_tag("pPr"))
    if ppr is None:
        return None

    style = ppr.find(_w_tag("pStyle"))
    if style is None:
        return None

    val = style.get(_w_tag("val")) or style.get(f"{{{_WORD_NS}}}val") or style.get("w:val")
    if not val:
        return None

    match = _HEADING_STYLE_RE.fullmatch(val)
    if not match:
        return None
    return int(match.group(1))


def _paragraph_visible_text(paragraph) -> str:
    parts = []
    for text_node in paragraph.iter(_w_tag("t")):
        if text_node.text:
            parts.append(text_node.text)
    return "".join(parts).strip()


def _get_toc_heading_range(instr_text: str) -> tuple[int, int]:
    match = _TOC_RANGE_RE.search(instr_text)
    if not match:
        return 1, 9

    start = int(match.group(1))
    end = int(match.group(2))
    if start > end:
        start, end = end, start
    return max(start, 1), min(end, 9)


def _find_existing_bookmark_name(paragraph) -> str | None:
    for bookmark in paragraph.findall(_w_tag("bookmarkStart")):
        name = bookmark.get(_w_tag("name")) or bookmark.get(f"{{{_WORD_NS}}}name") or bookmark.get("w:name")
        if name and name != "_GoBack":
            return name
    return None


def _insert_heading_bookmark(paragraph, bookmark_name: str, bookmark_id: int) -> None:
    import xml.etree.ElementTree as ET

    start = ET.Element(
        _w_tag("bookmarkStart"),
        {
            _w_tag("name"): bookmark_name,
            _w_tag("id"): str(bookmark_id),
        },
    )
    end = ET.Element(_w_tag("bookmarkEnd"), {_w_tag("id"): str(bookmark_id)})

    insert_at = 0
    if len(paragraph) > 0 and paragraph[0].tag == _w_tag("pPr"):
        insert_at = 1
    paragraph.insert(insert_at, start)
    paragraph.append(end)


def _collect_toc_entries(root, min_level: int, max_level: int) -> list[dict[str, object]]:
    body = root.find(_w_tag("body"))
    if body is None:
        return []
    body_children = list(body)

    next_bookmark_id = 1
    for bookmark in root.iter(_w_tag("bookmarkStart")):
        raw_id = bookmark.get(_w_tag("id")) or bookmark.get(f"{{{_WORD_NS}}}id") or bookmark.get("w:id")
        try:
            next_bookmark_id = max(next_bookmark_id, int(raw_id) + 1)
        except (TypeError, ValueError):
            continue

    has_toc_sdt = any(
        "TOC " in "".join((i.text or "") for i in sdt.iter(_w_tag("instrText")))
        for sdt in root.iter(_w_tag("sdt"))
    )

    entries: list[dict[str, object]] = []

    for idx, child in enumerate(body_children):
        level = child.tag == _w_tag("p") and _get_heading_level(child) or None
        if isinstance(level, int) and min_level <= level <= max_level:
            title = _paragraph_visible_text(child)
            if title:
                title_compact = title.replace(" ", "").replace("\u3000", "")
                if has_toc_sdt and title_compact in {"目录", "TableofContents"}:
                    continue
                anchor = _find_existing_bookmark_name(child)
                if not anchor:
                    anchor = f"_toc_auto_{len(entries) + 1}"
                    _insert_heading_bookmark(child, anchor, next_bookmark_id)
                    next_bookmark_id += 1

                entries.append(
                    {
                        "title": title,
                        "level": level,
                        "anchor": anchor,
                    }
                )

    return entries


def _toc_has_visible_cache(sdt_content) -> bool:
    for text_node in sdt_content.iter(_w_tag("t")):
        if text_node.text and text_node.text.strip():
            return True
    return False


def _build_toc_paragraph(entry: dict[str, object], instr_text: str | None, hyperlink: bool, position: str):
    import xml.etree.ElementTree as ET

    level = int(entry["level"])
    anchor = str(entry["anchor"])
    title = str(entry["title"])

    paragraph = ET.Element(_w_tag("p"))

    ppr = ET.SubElement(paragraph, _w_tag("pPr"))
    ET.SubElement(ppr, _w_tag("pStyle"), {_w_tag("val"): f"TOC{level}"})
    indent_left = (level - 1) * 480
    if indent_left > 0:
        ET.SubElement(ppr, _w_tag("ind"), {_w_tag("left"): str(indent_left)})

    if position == "first":
        run = ET.SubElement(paragraph, _w_tag("r"))
        ET.SubElement(
            run,
            _w_tag("fldChar"),
            {
                _w_tag("fldCharType"): "begin",
                _w_tag("dirty"): "true",
            },
        )
        instr = ET.SubElement(run, _w_tag("instrText"))
        instr.set(f"{{{_XML_NS}}}space", "preserve")
        instr.text = instr_text or 'TOC \\h \\o "1-3"'
        ET.SubElement(run, _w_tag("fldChar"), {_w_tag("fldCharType"): "separate"})

    content_parent = paragraph
    if hyperlink and anchor:
        content_parent = ET.SubElement(
            paragraph,
            _w_tag("hyperlink"),
            {
                _w_tag("history"): "1",
                _w_tag("anchor"): anchor,
            },
        )

    entry_run = ET.SubElement(content_parent, _w_tag("r"))

    title_node = ET.SubElement(entry_run, _w_tag("t"))
    title_node.set(f"{{{_XML_NS}}}space", "default")
    title_node.text = title

    if position == "last":
        end_run = ET.SubElement(paragraph, _w_tag("r"))
        ET.SubElement(end_run, _w_tag("fldChar"), {_w_tag("fldCharType"): "end"})

    return paragraph


def _build_toc_end_paragraph():
    import xml.etree.ElementTree as ET

    paragraph = ET.Element(_w_tag("p"))
    run = ET.SubElement(paragraph, _w_tag("r"))
    ET.SubElement(run, _w_tag("fldChar"), {_w_tag("fldCharType"): "end"})
    return paragraph


def _fix_docx_toc_cached_entries(docx_path: Path) -> bool:
    """Populate visible TOC entries from Heading paragraphs when the TOC is empty.

    Unlike Word's normal TOC cache, this backfill writes only structure and
    dotted leaders. It intentionally omits page numbers so stale pagination is
    never shown when the document opens before a field refresh.
    """
    import zipfile
    import xml.etree.ElementTree as ET

    document_name: str | None = None
    document_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            if name.endswith("document.xml"):
                document_name = name
                document_raw = zin.read(name).decode("utf-8", errors="replace")
                break

    if document_name is None or document_raw is None:
        return False
    if "TOC " not in document_raw or "Heading" not in document_raw:
        return False

    document_raw = _strip_illegal_xml_chars(document_raw)
    _register_xml_namespaces(document_raw)
    root = ET.fromstring(document_raw)
    body = root.find(_w_tag("body"))

    patched = False
    for sdt in list(root.iter(_w_tag("sdt"))):
        instr_parts = [
            (instr.text or "")
            for instr in sdt.iter(_w_tag("instrText"))
        ]
        instr_text = "".join(instr_parts).strip()
        if "TOC " not in instr_text:
            continue

        sdt_content = sdt.find(_w_tag("sdtContent"))
        if sdt_content is None:
            continue

        min_level, max_level = _get_toc_heading_range(instr_text)
        entries = _collect_toc_entries(root, min_level, max_level)
        if not entries:
            continue

        hyperlink = "\\h" in instr_text
        new_children = []
        for index, entry in enumerate(entries):
            if len(entries) == 1:
                position = "first"
            elif index == 0:
                position = "first"
            elif index == len(entries) - 1:
                position = "last"
            else:
                position = "middle"
            new_children.append(_build_toc_paragraph(entry, instr_text, hyperlink, position))

        if len(entries) == 1:
            new_children.append(_build_toc_end_paragraph())

        sdt_content[:] = new_children
        patched = True

        # docx-js wraps TableOfContents inside a <w:p> when the model uses
        # h.p([new TableOfContents(...)]).  A block-level SDT (containing <w:p>
        # children) nested inside another <w:p> is invalid OOXML — WPS silently
        # ignores the whole TOC.  Hoist the SDT to be a direct body child.
        if body is not None:
            for bi, body_child in enumerate(list(body)):
                if body_child.tag == _w_tag("p") and sdt in list(body_child):
                    body_child.remove(sdt)
                    body.insert(bi + 1, sdt)
                    remaining = [c for c in body_child if c.tag != _w_tag("pPr")]
                    if not remaining:
                        body.remove(body_child)
                    break

    if not patched:
        return False

    new_raw = ET.tostring(root, encoding="utf-8", xml_declaration=True).decode("utf-8")

    tmp_path = str(docx_path) + ".tmp"
    try:
        with zipfile.ZipFile(docx_path, "r") as zin, \
             zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == document_name:
                    zout.writestr(item, new_raw.encode("utf-8"))
                else:
                    zout.writestr(item, zin.read(item.filename))
        shutil.move(tmp_path, str(docx_path))
    finally:
        if Path(tmp_path).exists():
            Path(tmp_path).unlink()

    return True


def _fix_docx_toc_dot_leaders(docx_path: Path) -> bool:
    """Fix pre-populated TOC entries to match real TOC styling.

    1. Remove dot leaders and trailing ``<w:tab/>`` from cached entries
       (no page numbers → dots trail into empty space).
    2. Assign ``TOC1``/``TOC2``/``TOC3`` paragraph styles to cached entries
       so font, indent, spacing match the real TOC after field update.
    """
    import zipfile
    import xml.etree.ElementTree as ET

    document_name: str | None = None
    document_raw: str | None = None

    with zipfile.ZipFile(docx_path, "r") as zin:
        for name in zin.namelist():
            if name.endswith("document.xml"):
                document_name = name
                document_raw = zin.read(name).decode("utf-8", errors="replace")
                break

    if document_name is None or document_raw is None:
        return False

    document_raw = _strip_illegal_xml_chars(document_raw)
    _register_xml_namespaces(document_raw)
    root = ET.fromstring(document_raw)

    changed = False
    tab_tag = _w_tag("tab")
    tabs_tag = _w_tag("tabs")
    ppr_tag = _w_tag("pPr")
    pstyle_tag = _w_tag("pStyle")
    val_attr = _w_tag("val")
    ind_tag = _w_tag("ind")
    left_attr = _w_tag("left")
    p_tag = _w_tag("p")
    fld_char_tag = _w_tag("fldChar")

    for sdt in root.iter(_w_tag("sdt")):
        instr_parts = [
            (instr.text or "") for instr in sdt.iter(_w_tag("instrText"))
        ]
        if "TOC " not in "".join(instr_parts):
            continue

        sdt_content = sdt.find(_w_tag("sdtContent"))
        if sdt_content is None:
            continue

        for para in list(sdt_content.iter(p_tag)):
            if para.find(f".//{fld_char_tag}") is not None:
                has_text = False
                for t_el in para.iter(_w_tag("t")):
                    if t_el.text and t_el.text.strip():
                        has_text = True
                        break
                if not has_text:
                    continue

            ppr = para.find(ppr_tag)

            # Determine level from existing pStyle or indent
            level = 1
            if ppr is not None:
                ps = ppr.find(pstyle_tag)
                if ps is not None:
                    sval = ps.get(val_attr, "")
                    if sval.startswith("TOC") and sval[3:].isdigit():
                        level = int(sval[3:])
                    elif sval.startswith("TOCHeading"):
                        level = 1
                ind = ppr.find(ind_tag)
                if ind is not None and level == 1:
                    left = int(ind.get(left_attr, "0") or "0")
                    if left >= 800:
                        level = 3
                    elif left >= 400:
                        level = 2

            toc_style = f"TOC{level}"

            # Set/update pStyle to TOC{level}
            if ppr is None:
                ppr = ET.SubElement(para, ppr_tag)
                para.insert(0, ppr)
            ps = ppr.find(pstyle_tag)
            if ps is None:
                ps = ET.SubElement(ppr, pstyle_tag)
                ppr.insert(0, ps)
            if ps.get(val_attr) != toc_style:
                ps.set(val_attr, toc_style)
                changed = True

            # Remove inline tabs definition (style handles indentation)
            for tabs_el in list(ppr.iter(tabs_tag)):
                ppr.remove(tabs_el)
                changed = True

            # Remove inline indent (style handles it)
            for ind_el in list(ppr.findall(ind_tag)):
                ppr.remove(ind_el)
                changed = True

            # Remove <w:tab/> elements from runs
            for run in list(para.iter(_w_tag("r"))):
                for tab_el in list(run):
                    if tab_el.tag == tab_tag:
                        run.remove(tab_el)
                        changed = True

    if not changed:
        return False

    new_xml = ET.tostring(root, encoding="unicode", xml_declaration=True)
    import io
    buf = io.BytesIO()
    with zipfile.ZipFile(docx_path, "r") as zin:
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                if item.filename == document_name:
                    zout.writestr(item, new_xml)
                else:
                    zout.writestr(item, zin.read(item.filename))
    docx_path.write_bytes(buf.getvalue())
    return True


def _post_fix_generated_docx(script: Path, working_dir: Path, output_path: Path | None = None) -> list[str]:
    """Find DOCX files generated by the script and fix docx.js bugs."""
    source = script.read_text(encoding="utf-8")
    fixes: list[str] = []

    docx_paths: set[Path] = set()

    # Strategy 0: use explicitly passed output_path (most reliable)
    if output_path and output_path.exists():
        docx_paths.add(output_path)

    # Strategy 1: literal string paths in writeFileSync('xxx.docx')
    for m in re.finditer(r"""writeFileSync\s*\(\s*['"]([^'"]+\.docx)['"]\s*""", source):
        p = Path(m.group(1))
        if not p.is_absolute():
            p = working_dir / p
        if p.exists():
            docx_paths.add(p)

    # Strategy 2: resolve variable-based paths like OUTPUT_DIR + '/xxx.docx'
    for m in re.finditer(
        r"""writeFileSync\s*\(\s*(\w+)\s*\+\s*['"]([^'"]*\.docx)['"]\s*""", source,
    ):
        var_name = m.group(1)
        suffix = m.group(2)
        var_match = re.search(
            rf"""{re.escape(var_name)}\s*=\s*['"]([^'"]+)['"]\s*;""", source,
        )
        if var_match:
            p = Path(var_match.group(1) + suffix)
            if p.exists():
                docx_paths.add(p)

    # Strategy 3: resolve variable-based outputPath in writeFileSync(outputPath, ...)
    for m in re.finditer(
        r"""writeFileSync\s*\(\s*(\w+)\s*,\s*""", source,
    ):
        var_name = m.group(1)
        if var_name == 'outputPath':
            # outputPath is set from process.argv[2] in docx-helper.js
            continue
        # Try to resolve from source
        var_match = re.search(
            rf"""{re.escape(var_name)}\s*=\s*['"]([^'"]+\.docx)['"]\s*""", source,
        )
        if var_match:
            p = Path(var_match.group(1))
            if p.exists():
                docx_paths.add(p)

    # Strategy 4: fallback — scan working_dir for any .docx files
    if not docx_paths:
        for p in working_dir.rglob("*.docx"):
            docx_paths.add(p)

    for p in sorted(docx_paths):
        try:
            _fix_docx_toc_styles(p)
        except Exception:
            pass

        try:
            _fix_docx_heading_outline_levels(p)
        except Exception:
            pass

        try:
            _fix_docx_toc_cached_entries(p)
        except Exception:
            pass

        try:
            _fix_docx_toc_dot_leaders(p)
        except Exception:
            pass

        try:
            _remove_update_fields(p)
        except Exception:
            pass

        try:
            count = _fix_docx_bookmark_ids(p)
            if count:
                fixes.append(f"修复 {count} 个重复 bookmark ID: {p.name}")
        except Exception:
            pass

        qc_warnings = _check_docx_quality(p)
        if qc_warnings:
            print(f"\n[quality-check] 检测到 {len(qc_warnings)} 个质量问题（{p.name}）：")
            for w in qc_warnings:
                print(f"  [注意] {w}")

    return fixes


# ── Post-generation quality checks ────────────────────────────────

_DOUBLE_NUM_AT_START_RE = re.compile(r"^\s*\[(\d+)\]\s*\[(\d+)\]")


def _check_docx_quality(docx_path: Path) -> list[str]:
    """Scan generated DOCX for known quality issues and return warnings.

    Currently checks:
      - Double numbering in reference entries: [1] [2] Author... indicates
        that bibliography() auto-numbered [1] but entry.text also started
        with [2], producing duplicate labels.
        Only triggers when [N] [M] appears at the start of a paragraph
        (not inline citations like 研究[2][3]).
    """
    import zipfile
    import xml.etree.ElementTree as ET

    warnings: list[str] = []

    try:
        with zipfile.ZipFile(docx_path, "r") as zin:
            for name in zin.namelist():
                if not name.endswith("document.xml"):
                    continue
                raw = zin.read(name).decode("utf-8", errors="replace")
                raw = _strip_illegal_xml_chars(raw)
                _register_xml_namespaces(raw)
                root = ET.fromstring(raw)

                for para in root.iter(_w_tag("p")):
                    text = _paragraph_visible_text(para)
                    if not text:
                        continue
                    m = _DOUBLE_NUM_AT_START_RE.search(text)
                    if m:
                        preview = text[:100] + ("..." if len(text) > 100 else "")
                        warnings.append(
                            f"参考文献双重编号: \"{preview}\"。"
                            f" bibliography() 已自动编号，去掉 entry.text 开头的 \"[数字] \""
                        )
    except Exception:
        pass

    return warnings


# ── Citation cleanup: remove hallucinated [@key] ─────────────────

_AUTO_BIB_JSON_RE = re.compile(r"""autoBibliography\s*\(\s*(['"])([^'"]+\.json)\1""")
_AUTO_BIB_VAR_RE = re.compile(r"""autoBibliography\s*\(\s*([A-Za-z_$][\w$]*)\s*(?:,|\))""")
_AUTO_BIB_JOIN_RE = re.compile(r"""autoBibliography\s*\(\s*path\.join\s*\(([^;\n]*)\)\s*\)""")
_JS_STR_ASSIGN_RE = re.compile(
    r"""(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(['"`])([^'"`]+)\2\s*;?"""
)
_JS_ALIAS_ASSIGN_RE = re.compile(
    r"""(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*([A-Za-z_$][\w$]*)\s*;?"""
)
_JS_PATH_JOIN_ASSIGN_RE = re.compile(
    r"""(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*path\.join\s*\(([^;\n]*)\)\s*;?"""
)
_TMPL_VAR_RE = re.compile(r"""\$\{([A-Za-z_$][\w$]*)\}""")
_INLINE_CITE_BLOCK_RE = re.compile(r"""\[@([^\]]+?)\]""")
_INLINE_CITE_SPLIT_RE = re.compile(r"[,;，；]")
_CJK_RE = re.compile(r"[\u4e00-\u9fff]")
_LATIN_RE = re.compile(r"[A-Za-z]")


def _extract_inline_citation_keys(source: str) -> list[str]:
    """Extract citation keys from all inline citation blocks.

    Supports both single and multi-key forms:
      - [@Wang2024]
      - [@Wang2024; @Li2023]
      - [@Wang2024, @Li2023]
    """
    keys: list[str] = []
    for m in _INLINE_CITE_BLOCK_RE.finditer(source or ""):
        block = m.group(1)
        for tok in _INLINE_CITE_SPLIT_RE.split(block):
            key = tok.strip()
            if key.startswith("@"):
                key = key[1:].strip()
            if key:
                keys.append(key)
    return keys


def _extract_auto_bibliography_json_paths(source: str) -> list[str]:
    paths = [m.group(2).strip() for m in _AUTO_BIB_JSON_RE.finditer(source)]

    var_values: dict[str, str] = {}
    alias_values: dict[str, str] = {}
    for m in _JS_STR_ASSIGN_RE.finditer(source):
        name = m.group(1).strip()
        value = m.group(3).strip()
        if value:
            var_values[name] = value

    for m in _JS_ALIAS_ASSIGN_RE.finditer(source):
        left = m.group(1).strip()
        right = m.group(2).strip()
        if left and right:
            alias_values[left] = right

    def _resolve_template(expr: str) -> str | None:
        if "${" not in expr:
            return expr

        def _replace(m: re.Match[str]) -> str:
            key = m.group(1).strip()
            resolved = _resolve_var(key)
            return resolved if resolved is not None else m.group(0)

        out = _TMPL_VAR_RE.sub(_replace, expr)
        return out if "${" not in out else None

    def _split_join_args(arg_text: str) -> list[str]:
        parts: list[str] = []
        cur = []
        quote: str | None = None
        for ch in arg_text:
            if quote:
                cur.append(ch)
                if ch == quote:
                    quote = None
                continue
            if ch in ("'", '"', "`"):
                quote = ch
                cur.append(ch)
                continue
            if ch == ",":
                token = "".join(cur).strip()
                if token:
                    parts.append(token)
                cur = []
                continue
            cur.append(ch)
        token = "".join(cur).strip()
        if token:
            parts.append(token)
        return parts

    def _token_to_path_part(token: str) -> str | None:
        token = token.strip()
        if not token or token in {"__dirname", "process.cwd()"}:
            return ""
        if (token[0] == token[-1]) and token[0] in ("'", '"', "`"):
            inner = token[1:-1]
            return _resolve_template(inner) or inner
        if re.fullmatch(r"[A-Za-z_$][\w$]*", token):
            return _resolve_var(token)
        return None

    def _resolve_join_expr(arg_text: str) -> str | None:
        parts = []
        for tok in _split_join_args(arg_text):
            part = _token_to_path_part(tok)
            if part is None:
                return None
            if part:
                parts.append(part)
        if not parts:
            return None
        return str(Path(*parts)).replace("\\", "/")

    def _resolve_var(name: str) -> str | None:
        seen = set()
        cur = name
        for _ in range(8):
            if cur in seen:
                return None
            seen.add(cur)
            # Normalize aliases like: const OUTROOT = __dirname;
            if cur in {"__dirname", "process.cwd()"}:
                return ""
            if cur in var_values:
                return _resolve_template(var_values[cur]) or var_values[cur]
            nxt = alias_values.get(cur)
            if not nxt:
                return None
            cur = nxt
        return None

    for m in _JS_PATH_JOIN_ASSIGN_RE.finditer(source):
        name = m.group(1).strip()
        arg_text = m.group(2).strip()
        resolved = _resolve_join_expr(arg_text)
        if name and resolved:
            var_values[name] = resolved

    for m in _AUTO_BIB_VAR_RE.finditer(source):
        var_name = m.group(1).strip()
        resolved = _resolve_var(var_name)
        if resolved:
            paths.append(resolved)

    for m in _AUTO_BIB_JOIN_RE.finditer(source):
        arg_text = m.group(1).strip()
        resolved = _resolve_join_expr(arg_text)
        if resolved:
            paths.append(resolved)

    deduped: list[str] = []
    seen = set()
    for p in paths:
        if p not in seen:
            seen.add(p)
            deduped.append(p)
    return deduped


def _is_mixed_zh_author_en_title(authors: str, title: str) -> bool:
    """Heuristic for mixed-language references we want to suppress in CN output."""
    a = (authors or "").strip()
    t = (title or "").strip()
    if not a or not t:
        return False
    return bool(_CJK_RE.search(a)) and bool(_LATIN_RE.search(t)) and not bool(_CJK_RE.search(t))


def _load_reference_meta(json_file: Path) -> tuple[set[str], set[str]]:
    raw = json_file.read_text(encoding="utf-8")
    data = json.loads(raw)
    if not isinstance(data, list):
        return set(), set()
    keys = set()
    mixed_keys = set()
    for item in data:
        if isinstance(item, dict):
            key = str(item.get("key", "")).strip()
            if key:
                keys.add(key)
                if _is_mixed_zh_author_en_title(
                    str(item.get("authors", "") or ""),
                    str(item.get("title", "") or ""),
                ):
                    mixed_keys.add(key)
    return keys, mixed_keys


def _clean_missing_citation_keys(script: Path, base_dir: Path) -> list[str]:
    """Remove inline citations [@key] whose key does not exist in references.json."""
    source = script.read_text(encoding="utf-8")
    raw_paths = _extract_auto_bibliography_json_paths(source)
    if not raw_paths:
        return []

    ref_keys: set[str] = set()
    mixed_ref_keys: set[str] = set()
    used_json_paths: list[Path] = []
    for raw_path in raw_paths:
        p = Path(raw_path).expanduser()
        if not p.is_absolute():
            p = (base_dir / p).resolve()
        if not p.exists():
            continue
        try:
            keys, mixed_keys = _load_reference_meta(p)
            ref_keys |= keys
            mixed_ref_keys |= mixed_keys
            used_json_paths.append(p)
        except Exception:
            continue

    if not ref_keys:
        return []

    missing_counts: dict[str, int] = {}
    mixed_counts: dict[str, int] = {}
    for key in _extract_inline_citation_keys(source):
        if key not in ref_keys:
            missing_counts[key] = missing_counts.get(key, 0) + 1
        elif key in mixed_ref_keys:
            mixed_counts[key] = mixed_counts.get(key, 0) + 1

    if not missing_counts and not mixed_counts:
        return []

    def _replace_missing(match: re.Match[str]) -> str:
        raw_block = match.group(1)
        kept: list[str] = []
        for tok in _INLINE_CITE_SPLIT_RE.split(raw_block):
            key = tok.strip()
            if key.startswith("@"):
                key = key[1:].strip()
            if not key:
                continue
            if key in missing_counts or key in mixed_counts:
                continue
            kept.append(key)
        if not kept:
            return ""
        return "[@" + "; @".join(kept) + "]"

    cleaned = _INLINE_CITE_BLOCK_RE.sub(_replace_missing, source)
    if cleaned != source:
        script.write_text(cleaned, encoding="utf-8")

    notes: list[str] = []
    notes.append(
        "引用列表已自动处理完毕：错误或缺失的引用已被自动忽略，无需关注。禁止修改 references.json 或手动插入引用条目"
    )
    return notes


def _check_zero_inline_citations(script: Path, base_dir: Path) -> str | None:
    """Refuse to execute if autoBibliography references a populated JSON but the JS has 0 [@key].

    Only triggers when ALL conditions are met:
      1. JS contains an autoBibliography(...) call
      2. At least one referenced JSON file exists and has valid entries
      3. The JS text contains zero [@key] inline citations

    If no autoBibliography call exists, or the JSON file is missing/empty
    (e.g. web-search-only scenario), this check passes silently.
    """
    source = script.read_text(encoding="utf-8")
    raw_paths = _extract_auto_bibliography_json_paths(source)
    if not raw_paths:
        return None

    total_ref_count = 0
    for raw_path in raw_paths:
        p = Path(raw_path).expanduser()
        if not p.is_absolute():
            p = (base_dir / p).resolve()
        if not p.exists():
            continue
        try:
            keys, _ = _load_reference_meta(p)
            total_ref_count += len(keys)
        except Exception:
            continue

    if total_ref_count == 0:
        return None

    cite_count = len(_extract_inline_citation_keys(source))
    if cite_count > 0:
        return None

    return (
        f"references.json 有 {total_ref_count} 条文献但正文无 [@key] 引用，"
        f"请在 h.p() 中添加（如 '...进展[@Wang2024]'）。"
        f"若未使用学术检索，无需关注改提示。"
    )


# ── Build lifecycle markers (emitted by docx-helper.js) ──────────

_BUILD_STARTED_RE = re.compile(r"^\[docx-helpers\] build 开始执行 → (.+)$", re.MULTILINE)
_BUILD_DONE_RE = re.compile(r"^\[docx-helpers\] 文档已生成: (.+)$", re.MULTILINE)


def _parse_build_lifecycle(stdout: str) -> dict[str, list[str]]:
    """Extract started/done paths emitted by h.build()."""
    started = [m.group(1).strip() for m in _BUILD_STARTED_RE.finditer(stdout or "")]
    done = [m.group(1).strip() for m in _BUILD_DONE_RE.finditer(stdout or "")]
    return {"started": started, "done": done}


def _diagnose_missing_output(
    *,
    output_resolved: str | None,
    build_states: dict[str, list[str]],
    stderr: str,
) -> str:
    """Build a precise diagnosis for "Node returned 0 but no DOCX produced"."""
    started = build_states.get("started") or []
    done = build_states.get("done") or []

    if not started and not done:
        return (
            "h.build() 从未执行。检查脚本末尾是否有顶层 h.build({ sections: [...] }) 调用，"
            "或 build 是否在未调用的函数/不可达的分支里。"
        )

    if started and not done:
        detail = ""
        if stderr and stderr.strip():
            tail = "\n    ".join(stderr.strip().splitlines()[-10:])
            detail = f"\n  stderr 末尾:\n    {tail}"
        return (
            f"h.build() 已启动但未完成写盘（目标: {', '.join(started)}）。"
            f" 检查 stderr 中的异常信息。"
            f"{detail}"
        )

    # done 非空但目标文件路径不匹配 → 用户在 build 里传了自定义路径，与 run_node_docx(output=...) 不一致
    if output_resolved and done:
        done_resolved = [str(Path(p).expanduser()) for p in done]
        if output_resolved not in done_resolved:
            return (
                f"h.build() 写入了 {done_resolved}，但期望路径是 {output_resolved!r}。"
                " 不要在 h.build() 中手动传路径，由 run_node_docx(output=...) 统一注入。"
            )

    return "h.build() 报告完成但目标文件不存在"


def _ensure_output_written(
    *,
    script_path: str | Path,
    script: Path,
    working_dir: Path,
    output_resolved: str | None,
    build_states: dict[str, list[str]],
    stdout: str,
    stderr: str,
) -> None:
    """Authoritative post-execution check. Raises RuntimeError if no DOCX was produced.

    Replaces the old ``print("[warn] ...")`` which let "silent success" bugs
    leak through to the LLM, often costing 20-50 extra rounds before the
    model noticed via user feedback.
    """
    done_paths = [Path(p).expanduser() for p in build_states.get("done") or []]
    started_paths = [Path(p).expanduser() for p in build_states.get("started") or []]

    if output_resolved:
        out_p = Path(output_resolved)
        if out_p.is_file() and out_p.stat().st_size > 0:
            return
        diag = _diagnose_missing_output(
            output_resolved=output_resolved,
            build_states=build_states,
            stderr=stderr,
        )
        raise RuntimeError(f"DOCX 未生成: {output_resolved}\n{diag}")

    # output 未指定 → 至少要有一次 build done 标记，并且对应文件确实在
    if done_paths:
        existing = [p for p in done_paths if p.is_file() and p.stat().st_size > 0]
        if existing:
            return
        raise RuntimeError(
            "h.build() 报告写入 " + ", ".join(str(p) for p in done_paths)
            + " 但磁盘上找不到这些文件"
        )

    if started_paths:
        raise RuntimeError(_diagnose_missing_output(
            output_resolved=None,
            build_states=build_states,
            stderr=stderr,
        ))

    raise RuntimeError(_diagnose_missing_output(
        output_resolved=None,
        build_states=build_states,
        stderr=stderr,
    ))


# ── Main entry point ─────────────────────────────────────────────

def run_node_docx(
    script_path: str | Path,
    output: str | Path | None = None,
    cwd: str | Path | None = None,
) -> subprocess.CompletedProcess:
    """Execute a docx-generating JS script with comprehensive pre-flight checks and auto-backup.

    Args:
        script_path: JS 脚本路径。
        output: DOCX 输出路径。传入后作为 process.argv[2] 传给 node，
                JS 侧 h.build() 无需再传 outputPath。
        cwd: 工作目录，默认为脚本所在目录。

    Pre-flight (before execution):
      1. Syntax check (node --check)
      2. Module resolution (require.resolve)
      3. File path verification (readFileSync targets)
      4. Common mistake detection (ShadingType.SOLID, wrong enums, etc.)
      5. Table width consistency

    Runtime:
      - Auto-backup on first run, update backup on each success
      - Rich error context with source line annotations on failure
    """
    script = Path(script_path).expanduser()
    if not script.exists():
        raise FileNotFoundError(f"JS 文件不存在：{script}")

    # 最早一步：核心 npm 模块预检，缺失则立即抛错（带一次性安装命令）
    _ensure_core_npm_modules()

    output_resolved = str(Path(output).expanduser()) if output else None
    if output_resolved and os.path.isdir(output_resolved):
        raise ValueError(
            f"output 是目录而非文件路径: {output_resolved}，应传完整路径如 {output_resolved}/文档.docx"
        )
    # h.build() inside JS does writeFileSync(outputPath, buf) — if the
    # parent dir is missing it dies with a raw ENOENT and the LLM has to
    # re-issue the call just to insert one `mkdir`. Do it for them.
    if output_resolved:
        out_parent = Path(output_resolved).parent
        if out_parent and not out_parent.exists():
            try:
                out_parent.mkdir(parents=True, exist_ok=True)
            except OSError as e:
                raise RuntimeError(
                    f"无法创建 output 父目录 {out_parent}: {e}\n"
                    f"  → 请检查路径是否合法或是否有写入权限。"
                ) from e

    # ① Auto-backup (MUST happen before any auto-fix writes).
    # This guarantees the first .js.bak captures the true user input, not
    # a potentially mutated script produced by fixers.
    backup = _backup_path(script)
    if not backup.exists():
        shutil.copy2(script, backup)

    # ⓪ Auto-fix pipeline (3 stages)
    env = _with_global_node_path()
    fixes = auto_fix_pipeline(script, env)

    # ⓪.1 Citation cleanup: remove hallucinated [@key] not present in references.json
    cleanup_base = Path(cwd).expanduser().resolve() if cwd else script.parent
    citation_cleanup_notes = _clean_missing_citation_keys(script, cleanup_base)

    # ⓪.2 Citation zero-check: error if autoBibliography has entries but JS has 0 [@key]
    zero_cite_error = _check_zero_inline_citations(script, cleanup_base)
    if zero_cite_error:
        raise ValueError(f"[citation-check] {zero_cite_error}")

    # ② Pre-flight: catch everything we can before executing
    errors = preflight(script)
    if errors:
        sep = "\n  • "
        raise ValueError(
            f"预检发现 {len(errors)} 个问题（未执行脚本，请修复后重试）:{sep}{sep.join(errors)}"
        )

    working_dir = Path(cwd).expanduser().resolve() if cwd else script.parent

    # ③ Execute — output path passed as argv[2] so h.build() can read it
    cmd = [_get_node_exe(), str(script)]
    if output_resolved:
        cmd.append(output_resolved)

    result = subprocess.run(
        cmd,
        capture_output=True, text=True,
        cwd=str(working_dir), env=env,
        encoding="utf-8",
    )

    if result.returncode != 0:
        runtime_error_text = result.stderr.strip() or result.stdout.strip()
        error_detail = _extract_error_context(script, runtime_error_text)
        missing_modules = _extract_missing_modules(runtime_error_text)
        missing_module_hint = ""
        if missing_modules:
            missing_module_hint = "\n\n" + _missing_modules_hint(missing_modules, env)
        hint = ""
        if backup.exists():
            hint = f"\n\n备份: restore_backup(\"{script_path}\") 可恢复上次成功版本"
        raise RuntimeError(f"运行时错误:\n{error_detail}{missing_module_hint}{hint}")

    # ④ 解析 JS 侧 build 生命周期标记
    build_states = _parse_build_lifecycle(result.stdout)

    # ⑤ Verify output actually written
    _ensure_output_written(
        script_path=script_path,
        script=script,
        working_dir=working_dir,
        output_resolved=output_resolved,
        build_states=build_states,
        stdout=result.stdout,
        stderr=result.stderr,
    )

    # ⑥ Success → update backup
    shutil.copy2(script, backup)

    # ⑦ Post-process: fix docx.js bugs in generated files
    _post_fix_generated_docx(script, working_dir, Path(output_resolved) if output_resolved else None)
    if result.stderr.strip():
        print(result.stderr.strip())
    if result.stdout.strip():
        print(result.stdout.strip())

    print(
        "文档已生成成功，接下来直接交付给用户即可"
    )

    return result


def restore_backup(script_path: str | Path) -> str:
    """Restore the JS file from its last-known-good backup (.js.bak)."""
    script = Path(script_path).expanduser()
    backup = _backup_path(script)
    if not backup.exists():
        raise FileNotFoundError(f"没有可用的备份: {backup}")
    shutil.copy2(backup, script)
    print(f"已恢复: {backup} → {script}")
    return str(script)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("用法: python run_node_docx.py <script.js>")
    run_node_docx(sys.argv[1])


if __name__ == "__main__":
    main()
