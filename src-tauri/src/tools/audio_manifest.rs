//! 语音模型映射清单：`tool_manifest` 通用外壳在 `tts` / `stt` 上的实例。
//!
//! 语音合成与语音转写是两个独立工具，因此各有一份清单与槽位集合；
//! 二者共用同一套结构、校验与市场下发机制，仅 `kind` 不同。

use crate::db::init::DbPool;

pub use super::tool_manifest::Effective;

pub const KIND_TTS: &str = "tts";
pub const KIND_STT: &str = "stt";

const BUILTIN_TTS: &str = include_str!("../../resources/tts-models.json");
const BUILTIN_STT: &str = include_str!("../../resources/stt-models.json");

const SLOTS_TTS: [&str; 5] = ["text", "voice", "language", "format", "speed"];
const SLOTS_STT: [&str; 4] = ["file", "language", "format", "temperature"];

pub static TTS_SPEC: super::tool_manifest::ManifestSpec = super::tool_manifest::ManifestSpec {
    kind: KIND_TTS,
    builtin: BUILTIN_TTS,
    market_path: crate::utils::market::TTS_MODELS_PATH,
    schema_version: 1,
    slots: &SLOTS_TTS,
};

pub static STT_SPEC: super::tool_manifest::ManifestSpec = super::tool_manifest::ManifestSpec {
    kind: KIND_STT,
    builtin: BUILTIN_STT,
    market_path: crate::utils::market::STT_MODELS_PATH,
    schema_version: 1,
    slots: &SLOTS_STT,
};

/// 按 `kind` 取清单描述
pub fn spec_of(kind: &str) -> Option<&'static super::tool_manifest::ManifestSpec> {
    match kind {
        KIND_TTS => Some(&TTS_SPEC),
        KIND_STT => Some(&STT_SPEC),
        _ => None,
    }
}

pub fn ensure_seeded(conn: &rusqlite::Connection) -> Result<(), String> {
    super::tool_manifest::ensure_seeded(conn, &TTS_SPEC)?;
    super::tool_manifest::ensure_seeded(conn, &STT_SPEC)
}

pub fn spawn_background_refresh(pool: DbPool) {
    super::tool_manifest::spawn_background_refresh(pool.clone(), &TTS_SPEC);
    super::tool_manifest::spawn_background_refresh(pool, &STT_SPEC);
}

pub fn snapshot_tts() -> Result<Effective, String> {
    super::tool_manifest::snapshot(&TTS_SPEC)
}

pub fn snapshot_stt() -> Result<Effective, String> {
    super::tool_manifest::snapshot(&STT_SPEC)
}

/// 某模型在指定语音清单下的能力摘要
pub fn capability(kind: &str, model: &str) -> Result<serde_json::Value, String> {
    let spec = spec_of(kind)
        .ok_or_else(|| format!("未知的语音清单 kind: {}（可选 tts / stt）", kind))?;
    super::tool_manifest::capability(spec, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool_manifest::ToolManifest;

    #[test]
    fn builtin_tts_and_stt_manifests_parse() {
        ToolManifest::builtin(&TTS_SPEC).expect("tts 内置清单应可解析");
        ToolManifest::builtin(&STT_SPEC).expect("stt 内置清单应可解析");
    }

    #[test]
    fn deepseek_rules_select_and_omit_model() {
        let tts = ToolManifest::builtin(&TTS_SPEC).unwrap();
        let (rule, verified) = tts.select("deepseek-tts").unwrap();
        assert!(verified, "deepseek 应命中专属规则而不是兜底");
        assert_eq!(rule.match_pattern, "*deepseek*");
        assert!(rule.omit_model, "DeepSeek 端点没有 model 参数");
        assert_eq!(rule.transport.create, "/tools/tts");
        assert_eq!(rule.mapping["voice"].field.as_deref(), Some("voice_id"));
        assert!(rule.mapping.get("speed").is_none());

        let stt = ToolManifest::builtin(&STT_SPEC).unwrap();
        let (rule, verified) = stt.select("deepseek-asr").unwrap();
        assert!(verified);
        assert!(rule.omit_model);
        assert_eq!(rule.transport.create, "/tools/asr");
        assert!(rule.mapping.get("language").is_none());
        assert_eq!(rule.limits.max_bytes, Some(8 * 1024 * 1024));
    }
}
