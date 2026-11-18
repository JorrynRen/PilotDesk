//! 图片模型映射清单：`tool_manifest` 通用外壳在 `kind='image'` 上的实例。

use crate::db::init::DbPool;

pub use super::tool_manifest::{BodyKind, Effective};

/// `tool_manifests.kind`：图片清单
pub const KIND_IMAGE: &str = "image";

/// 随包内置的基线清单（离线与首次启动可用）
pub const BUILTIN_MANIFEST: &str = include_str!("../../resources/image-models.json");

/// 客户端支持的图片清单格式版本
pub const SUPPORTED_SCHEMA_VERSION: u64 = 1;

/// 图片工具的第一层槽位
pub const SLOTS: [&str; 5] = ["prompt", "size", "n", "referenceImages", "mask"];

pub static SPEC: super::tool_manifest::ManifestSpec = super::tool_manifest::ManifestSpec {
    kind: KIND_IMAGE,
    builtin: BUILTIN_MANIFEST,
    market_path: crate::utils::market::IMAGE_MODELS_PATH,
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
