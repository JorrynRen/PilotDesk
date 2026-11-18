//! 视频模型清单的管理命令（设置 › 运行与策略 › 模型清单）。
//!
//! 清单本身的三层解析、校验与刷新都在 `tools::video_manifest`；
//! 这里只负责取数据库连接、把入参转发过去。

use crate::tools::video_manifest::{self, ManifestInfo, ManifestSourceText};
use crate::DbState;

/// 当前生效清单状态（含规则摘要）
#[tauri::command]
pub fn video_manifest_info() -> Result<ManifestInfo, String> {
    video_manifest::info()
}

/// 编辑器初始内容：本地编辑优先，否则当前基线原文
#[tauri::command]
pub fn video_manifest_source() -> Result<ManifestSourceText, String> {
    video_manifest::source_text()
}

/// 保存本地编辑：整份校验通过才落库并立即生效
#[tauri::command]
pub fn save_video_manifest(
    state: tauri::State<'_, DbState>,
    text: String,
) -> Result<ManifestInfo, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    video_manifest::save_local(&conn, &text)
}

/// 强制从市场拉取（多源取最高版本）
#[tauri::command]
pub async fn refresh_video_manifest() -> Result<ManifestInfo, String> {
    video_manifest::refresh().await
}

/// 回退：`local` 仅放弃本地编辑；`all` 再删除市场缓存、回落内置基线
#[tauri::command]
pub fn reset_video_manifest(
    state: tauri::State<'_, DbState>,
    scope: String,
) -> Result<ManifestInfo, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    match scope.as_str() {
        "local" => video_manifest::clear_local(&conn),
        "all" => video_manifest::reset_all(&conn),
        other => Err(format!("未知的重置范围: {}（可选 local / all）", other)),
    }
}
