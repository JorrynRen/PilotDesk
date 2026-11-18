//! 视频模型清单的管理命令（设置 › 运行与策略 › 模型清单）。
//!
//! 清单的校验、版本规则、写库与内存投影都在 `tools::video_manifest`；
//! 这里只负责取数据库连接 / 连接池、把入参转发过去。

use crate::tools::video_manifest::{self, ManifestInfo, ManifestSourceText};
use crate::DbState;

/// 当前生效清单状态（含规则摘要）
#[tauri::command]
pub fn video_manifest_info() -> Result<ManifestInfo, String> {
    video_manifest::info()
}

/// 编辑器初始内容：库行当前生效原文
#[tauri::command]
pub fn video_manifest_source() -> Result<ManifestSourceText, String> {
    video_manifest::source_text()
}

/// 保存本地编辑：整份校验通过才写库并立即生效
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

/// 立即刷新：拉取市场最高版本，按版本规则写库或只记录市场版本
#[tauri::command]
pub async fn refresh_video_manifest(
    state: tauri::State<'_, DbState>,
) -> Result<ManifestInfo, String> {
    let pool = state.pool.clone();
    video_manifest::refresh(&pool).await
}

/// 应用市场版本：重新拉取并强制写入库（丢弃本地修改）
#[tauri::command]
pub async fn apply_video_manifest(
    state: tauri::State<'_, DbState>,
) -> Result<ManifestInfo, String> {
    let pool = state.pool.clone();
    video_manifest::apply_market(&pool).await
}

/// 恢复出厂清单：用内置覆盖库行
#[tauri::command]
pub fn reset_video_manifest(state: tauri::State<'_, DbState>) -> Result<ManifestInfo, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    video_manifest::reset_builtin(&conn)
}
