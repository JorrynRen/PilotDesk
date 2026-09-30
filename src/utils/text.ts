/**
 * 文本截断的统一入口（按 Unicode **码点**计数）。
 *
 * 为什么不能直接用 `s.slice(0, n)`：JS 的 slice 按 UTF-16 码元计数，切点落在 emoji
 * （代理对）中间会留下半个字符，界面上显示成乱码（�）。这里按码点切，不会切断字符。
 * 已知边界：不处理 ZWJ 组合 emoji（如"一家人"由多个码点拼成），展示场景足够。
 *
 * 这里只影响显示、不会崩溃；Rust 后端"按字节切片"的同类问题（会 panic）见
 * src-tauri/src/utils/text.rs。
 */

/** 取开头最多 `max` 个码点（不加省略号，调用方自行追加标记） */
export function headChars(s: string, max: number): string {
  if (max <= 0) return '';
  const chars = Array.from(s);
  return chars.length <= max ? s : chars.slice(0, max).join('');
}

/** 取结尾最多 `max` 个码点 */
export function tailChars(s: string, max: number): string {
  if (max <= 0) return '';
  const chars = Array.from(s);
  return chars.length <= max ? s : chars.slice(chars.length - max).join('');
}

/** 超长时截断并追加标记（默认省略号）；未超长原样返回 */
export function elide(s: string, max: number, marker = '…'): string {
  const chars = Array.from(s);
  return chars.length <= max ? s : chars.slice(0, max).join('') + marker;
}
