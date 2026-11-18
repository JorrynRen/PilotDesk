//! 工具模型清单的管理命令（设置 › 运行与策略 › 视频模型适配 / 图片模型适配）。
//!
//! 清单的校验、版本规则、写库与内存投影都在 `tools::tool_manifest`；
//! 这里只负责按 kind 选清单、取数据库连接 / 连接池、把入参转发过去。

use crate::tools::tool_manifest::{ManifestInfo, ManifestSourceText, ManifestSpec};
use crate::tools::{image_manifest, video_manifest};
use crate::DbState;

fn spec_of(kind: &str) -> Result<&'static ManifestSpec, String> {
    match kind {
        video_manifest::KIND_VIDEO => Ok(&video_manifest::SPEC),
        image_manifest::KIND_IMAGE => Ok(&image_manifest::SPEC),
        other => Err(format!("未知的清单类型: {}", other)),
    }
}

/// 当前生效清单状态（含规则摘要）
#[tauri::command]
pub fn tool_manifest_info(kind: String) -> Result<ManifestInfo, String> {
    crate::tools::tool_manifest::info(spec_of(&kind)?)
}

/// 编辑器初始内容：库行当前生效原文
#[tauri::command]
pub fn tool_manifest_source(kind: String) -> Result<ManifestSourceText, String> {
    crate::tools::tool_manifest::source_text(spec_of(&kind)?)
}

/// 保存本地编辑：整份校验通过才写库并立即生效
#[tauri::command]
pub fn save_tool_manifest(
    state: tauri::State<'_, DbState>,
    kind: String,
    text: String,
) -> Result<ManifestInfo, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::tools::tool_manifest::save_local(&conn, spec_of(&kind)?, &text)
}

/// 立即刷新：拉取市场最高版本，按版本规则写库或只记录市场版本
#[tauri::command]
pub async fn refresh_tool_manifest(
    state: tauri::State<'_, DbState>,
    kind: String,
) -> Result<ManifestInfo, String> {
    let pool = state.pool.clone();
    crate::tools::tool_manifest::refresh(&pool, spec_of(&kind)?).await
}

/// 应用市场版本：重新拉取并强制写入库（丢弃本地修改）
#[tauri::command]
pub async fn apply_tool_manifest(
    state: tauri::State<'_, DbState>,
    kind: String,
) -> Result<ManifestInfo, String> {
    let pool = state.pool.clone();
    crate::tools::tool_manifest::apply_market(&pool, spec_of(&kind)?).await
}

/// 恢复出厂清单：用内置覆盖库行
#[tauri::command]
pub fn reset_tool_manifest(
    state: tauri::State<'_, DbState>,
    kind: String,
) -> Result<ManifestInfo, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::tools::tool_manifest::reset_builtin(&conn, spec_of(&kind)?)
}
