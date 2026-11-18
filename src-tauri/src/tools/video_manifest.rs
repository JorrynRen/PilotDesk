//! 视频模型映射清单：`tool_manifest` 通用外壳在 `kind='video'` 上的实例。
//!
//! 槽位、随包基线与市场路径在这里声明，其余逻辑全部复用通用实现。

use crate::db::init::DbPool;

pub use super::tool_manifest::{
    apply_transform, write_mapping, AuthKind, BodyKind, Effective, ModelRule, OutputKind,
    ResponseMode, TaskSpec, TransformCtx,
};

/// `tool_manifests.kind`：视频清单
pub const KIND_VIDEO: &str = "video";

/// 随包内置的基线清单（离线与首次启动可用）
pub const BUILTIN_MANIFEST: &str = include_str!("../../resources/video-models.json");

/// 客户端支持的视频清单格式版本
pub const SUPPORTED_SCHEMA_VERSION: u64 = 2;

/// 第一层媒体槽位（顺序即回显顺序）
pub const MEDIA_SLOTS: [&str; 6] = [
    "firstFrame",
    "lastFrame",
    "referenceImages",
    "referenceVideo",
    "sourceVideo",
    "extendVideo",
];

/// 第一层标量槽位
pub const SCALAR_SLOTS: [&str; 6] = [
    "prompt",
    "duration",
    "aspectRatio",
    "resolution",
    "seed",
    "negativePrompt",
];

const SLOTS: [&str; 12] = [
    MEDIA_SLOTS[0],
    MEDIA_SLOTS[1],
    MEDIA_SLOTS[2],
    MEDIA_SLOTS[3],
    MEDIA_SLOTS[4],
    MEDIA_SLOTS[5],
    SCALAR_SLOTS[0],
    SCALAR_SLOTS[1],
    SCALAR_SLOTS[2],
    SCALAR_SLOTS[3],
    SCALAR_SLOTS[4],
    SCALAR_SLOTS[5],
];

pub static SPEC: super::tool_manifest::ManifestSpec = super::tool_manifest::ManifestSpec {
    kind: KIND_VIDEO,
    builtin: BUILTIN_MANIFEST,
    market_path: crate::utils::market::VIDEO_MODELS_PATH,
    schema_version: SUPPORTED_SCHEMA_VERSION,
    slots: &SLOTS,
};

pub fn ensure_seeded(conn: &rusqlite::Connection) -> Result<(), String> {
    super::tool_manifest::ensure_seeded(conn, &SPEC)
}

pub fn snapshot() -> Result<Effective, String> {
    super::tool_manifest::snapshot(&SPEC)
}

pub fn capability(model: &str) -> Result<serde_json::Value, String> {
    super::tool_manifest::capability(&SPEC, model)
}

pub fn spawn_background_refresh(pool: DbPool) {
    super::tool_manifest::spawn_background_refresh(pool, &SPEC)
}
