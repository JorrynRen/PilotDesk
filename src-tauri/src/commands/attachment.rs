use serde::Deserialize;
use tauri::State;

use crate::db::models::Attachment;
use crate::utils::errors::AppError;
use crate::utils::paths::resolve_attachments_dir;
use crate::DbState;

/// 单个待落盘的附件输入
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    /// "image" 或 "file"
    pub kind: String,
    /// 原始文件名
    pub name: String,
    /// MIME 类型（可为空，后端按扩展名兜底）
    pub mime: Option<String>,
    /// 源文件绝对路径（选择/拖拽走路径复制）
    pub path: Option<String>,
    /// 文件字节（base64，粘贴板兜底）
    pub data: Option<String>,
}

/// 将一批附件落盘到会话附件目录，返回落盘后的附件元数据。
///
/// 落盘目录：`<工作目录>/attachments/<session_id>/<时间戳>-<清洗后文件名>`
/// - 选择/拖拽：传入 `path`，后端直接 `fs::copy`；
/// - 粘贴：传入 `data`（base64 字节），后端写盘兜底。
#[tauri::command]
pub fn save_attachments(
    state: State<'_, DbState>,
    session_id: String,
    items: Vec<AttachmentInput>,
) -> Result<Vec<Attachment>, AppError> {
    let conn = state.get_conn()?;

    let cwd: Option<String> = conn
        .query_row(
            "SELECT cwd FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    let dir = resolve_attachments_dir(&conn, cwd.as_deref(), &session_id);
    save_attachments_to_dir(&dir, &items)
}

/// 群聊附件落盘：目录为 `<工作目录>/attachments/<room_id>`。
/// 群聊没有 sessions 记录，故 cwd 回退全局工作区。
#[tauri::command]
pub fn groupchat_save_attachments(
    state: State<'_, DbState>,
    room_id: String,
    items: Vec<AttachmentInput>,
) -> Result<Vec<Attachment>, AppError> {
    let conn = state.get_conn()?;
    let dir = resolve_attachments_dir(&conn, None, &room_id);
    save_attachments_to_dir(&dir, &items)
}

fn save_attachments_to_dir(
    dir: &std::path::Path,
    items: &[AttachmentInput],
) -> Result<Vec<Attachment>, AppError> {
    std::fs::create_dir_all(dir)?;

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let clean_name = sanitize_name(&item.name);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let unique_name = format!("{}-{}", ts, clean_name);
        let dest = dir.join(&unique_name);

        if let Some(src) = item.path.as_deref().filter(|s| !s.is_empty()) {
            std::fs::copy(src, &dest)
                .map_err(|e| AppError::Io(format!("复制附件失败 ({}): {}", src, e)))?;
        } else if let Some(data) = item.data.as_deref().filter(|s| !s.is_empty()) {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| AppError::InvalidInput(format!("附件数据解码失败: {}", e)))?;
            std::fs::write(&dest, bytes)?;
        } else {
            return Err(AppError::InvalidInput(
                "附件缺少 path 或 data 来源".to_string(),
            ));
        }

        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let mime = item
            .mime
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| guess_mime(&unique_name))
            .to_string();

        out.push(Attachment {
            kind: item.kind.clone(),
            name: item.name.clone(),
            path: dest.to_string_lossy().to_string(),
            mime,
            size,
        });
    }

    Ok(out)
}

/// 删除单个已落盘附件文件（用户在发送前移除附件时调用）。
///
/// 仅允许删除会话附件目录内的文件，避免误删任意路径。
#[tauri::command]
pub fn delete_attachment(
    state: State<'_, DbState>,
    session_id: String,
    path: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;

    let cwd: Option<String> = conn
        .query_row(
            "SELECT cwd FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    let dir = resolve_attachments_dir(&conn, cwd.as_deref(), &session_id);
    let target = std::path::PathBuf::from(&path);

    // 安全校验：目标必须直接位于该会话的附件目录内
    let parent = target.parent().map(|p| p.to_path_buf());
    if parent.as_deref() != Some(dir.as_path()) {
        return Err(AppError::InvalidInput(
            "拒绝删除附件目录之外的文件".to_string(),
        ));
    }

    match std::fs::remove_file(&target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Io(format!("删除附件失败 ({}): {}", path, e))),
    }
}

/// 清洗文件名：只保留安全的文件名字符，移除路径分隔符与控制字符。
fn sanitize_name(name: &str) -> String {
    let base = std::path::Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();

    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 按扩展名推断 MIME 类型（用于粘贴板来源缺失 mime 的场景）。
fn guess_mime(name: &str) -> &'static str {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "java" | "c" | "cpp" | "h" | "go" | "cs"
        | "rb" | "php" | "css" | "html" | "toml" | "yaml" | "yml" | "sh" | "bat" | "ps1" => {
            "text/plain"
        }
        _ => "application/octet-stream",
    }
}

/// 用系统默认方式打开本地路径：文件用默认程序打开，目录用资源管理器打开，URL 交给默认浏览器。
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    // http(s) 地址交给系统默认浏览器，不该按本地路径校验存在性
    let is_url = path.starts_with("http://") || path.starts_with("https://");
    if !is_url && !p.exists() {
        return Err(AppError::NotFound(format!("路径不存在: {}", path)).into());
    }

    #[cfg(target_os = "windows")]
    {
        // 走 ShellExecuteW（系统"打开"动词），**不要用 `explorer.exe <路径>`**：
        // explorer 会对参数做二次解析，路径里带逗号（文件名常见）或其它特殊字符时会解析失败，
        // 失败后它退化成打开默认窗口（表现为"点了却打开了文档目录"）。
        // ShellExecuteW 不做二次解析，文件名里有空格/逗号/中文都安全。
        use std::os::windows::ffi::OsStrExt;
        let wide = |s: &std::ffi::OsStr| -> Vec<u16> {
            s.encode_wide().chain(std::iter::once(0)).collect()
        };
        let operation = wide(std::ffi::OsStr::new("open"));
        let file = wide(p.as_os_str());
        // SAFETY: 全部指针都指向本调用内有效的 NUL 结尾宽字符串；ShellExecuteW 不保留这些指针。
        let rc = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };
        // 返回值 ≤ 32 表示失败（约定：大于 32 才是成功句柄）
        if (rc as isize) <= 32 {
            return Err(AppError::External(format!(
                "打开路径失败（ShellExecute 返回 {}）",
                rc as isize
            ))
            .into());
        }
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| AppError::External(format!("打开路径失败: {}", e)))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| AppError::External(format!("打开路径失败: {}", e)))?;
    }

    Ok(())
}
