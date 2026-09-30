/**
 * 模板值判定 — 与后端模板引擎（`src-tauri/src/workflow/template.rs`）保持同一口径。
 *
 * 后端模板正则是 `\{\{(.+?)\}\}`：`{{}}` 空名不算模板，未闭合的 `{{` 也会原样保留。
 * 前端的用途只有一处判断 —— "这个值是变量引用还是字面量"：
 * 含变量引用的值一律当作合法输入，不做类型校验（真实类型由后端解析出变量值后交 handler 判定）。
 */

/** 值里是否含变量引用（宽口径：出现 `{{` 即视为引用，避免把未闭合的输入当数字校验） */
export function containsTemplate(value: unknown): boolean {
  return typeof value === 'string' && value.includes('{{');
}
