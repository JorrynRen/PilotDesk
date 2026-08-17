//! 通用文件系统工具命令。

use std::path::PathBuf;

/// 将文本内容写入指定文件（保存对话框选定的任意路径）。
#[tauri::command]
pub fn write_text_file(path: String, content: String) -> Result<(), String> {
    let file_path = PathBuf::from(&path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败 ({}): {}", parent.display(), e))?;
    }
    std::fs::write(&file_path, content)
        .map_err(|e| format!("写入文件失败 ({}): {}", file_path.display(), e))
}
