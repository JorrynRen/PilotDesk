/**
 * 统一错误归一化：把 Promise / Tauri 命令抛出的各种错误形状收敛成一段可直接展示给用户的文本。
 *
 * 为什么需要它：后端 `#[tauri::command]` 的 `Err` 有两种形状——
 *  - `Result<_, String>`：Tauri 把 `Err` 序列化成**字符串**，前端 catch 到 string；
 *  - `Result<_, AppError>`：序列化成**对象** `{code, message, details}`（见
 *    `src-tauri/src/utils/errors.rs` 的 `impl Serialize for AppError`）。
 * 前端若直接 `${err}` 插值，遇到对象形状就会显示 `[object Object]`。
 *
 * message 与 details 的优先级依据 `errors.rs` 的序列化事实：
 *  - `message` 是写死的**分类标题**（如「数据库操作失败」「外部服务错误」）；
 *  - `details` 才是构造时传入的**具体正文**（如「请求失败: 400 invalid model」）。
 * 同一份错误里，`Display`/`From<AppError> for String` 也输出这段正文。
 * 因此这里**优先取 details**（信息量最大），仅在 details 缺失或为空时退回 message 分类标题。
 */

/** 无法提取任何信息时的固定兜底文案（不产出空串，也不产出 `[object Object]`） */
const FALLBACK = '未知错误';

export function errorMessage(err: unknown): string {
  if (err == null) return FALLBACK;

  if (typeof err === 'string') {
    return err.trim() ? err : FALLBACK;
  }

  if (err instanceof Error) {
    return err.message?.trim() ? err.message : FALLBACK;
  }

  if (typeof err === 'object') {
    const e = err as { details?: unknown; message?: unknown };
    // details 优先：errors.rs 中它是具体正文，message 只是分类标题
    if (typeof e.details === 'string' && e.details.trim()) return e.details;
    if (typeof e.message === 'string' && e.message.trim()) return e.message;
    try {
      return JSON.stringify(err) ?? FALLBACK;
    } catch {
      // 循环引用等无法序列化的对象，回退到固定文案
      return FALLBACK;
    }
  }

  // number / boolean / bigint / symbol 等
  return String(err) || FALLBACK;
}
