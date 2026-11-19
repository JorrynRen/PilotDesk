//! 语音模型能力查询：只读、无网络、无副作用。
//!
//! 数据完全来自当前生效的语音清单（tts / stt），不内联任何厂商能力。
//! `kind` 省略时同时返回两份结果，方便模型先确认一个模型到底是合成还是转写。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::audio_manifest;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 语音模型能力查询工具
pub struct ListAudioCapabilitiesTool;

#[async_trait]
impl ToolHandler for ListAudioCapabilitiesTool {
    fn name(&self) -> &str {
        "list_audio_capabilities"
    }

    fn description(&self) -> &str {
        "查询某个语音模型支持的参数槽位与限制（音色、格式、语言、字符/体积上限）。\
         只读、无网络。kind=tts 查语音合成，kind=stt 查语音转写；省略则两份都查。\
         在不确定模型是否支持某参数时先调用本工具，再调用 tts / stt；\
         返回的 unsupported 列表即该模型会被本地拒绝的参数。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "语音模型名，原样取自 list_models（含命名空间前缀时需完整保留）"
                },
                "kind": {
                    "type": "string",
                    "enum": ["tts", "stt"],
                    "description": "可选：tts（语音合成）或 stt（语音转写）；省略则两份都查"
                }
            },
            "required": ["model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Audio, ToolTag::Read]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let model = arguments
            .get("model")
            .and_then(|value| value.as_str())
            .map(|text| text.trim().trim_start_matches('@').to_string())
            .filter(|text| !text.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models，从清单中复制完整的语音模型名")?;
        let kind = arguments
            .get("kind")
            .and_then(|value| value.as_str())
            .map(|text| text.trim().to_ascii_lowercase())
            .filter(|text| !text.is_empty());

        let capability = match kind.as_deref() {
            Some(kind) => audio_manifest::capability(kind, &model)?,
            None => {
                let mut results = serde_json::Map::new();
                for (kind, result) in [
                    (audio_manifest::KIND_TTS, audio_manifest::capability(audio_manifest::KIND_TTS, &model)),
                    (audio_manifest::KIND_STT, audio_manifest::capability(audio_manifest::KIND_STT, &model)),
                ] {
                    if let Ok(value) = result {
                        results.insert(kind.to_string(), value);
                    }
                }
                if results.is_empty() {
                    return Err(format!(
                        "模型 '{}' 在语音合成与语音转写清单中都没有匹配规则。请用 list_models 确认模型名",
                        model
                    ));
                }
                Value::Object(results)
            }
        };
        serde_json::to_string_pretty(&capability)
            .map_err(|e| format!("序列化能力摘要失败: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_tts_slots_and_limits() {
        let out = ListAudioCapabilitiesTool
            .execute(json!({ "model": "tts-1", "kind": "tts" }))
            .await
            .expect("查询应成功");
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["matchedRule"], "tts-1");
        assert_eq!(value["output"], "file");
        assert!(value["supported"]["text"]["field"] == "input");
        assert_eq!(value["limits"]["maxChars"], 4096);
        assert!(value["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .all(|slot| slot != "voice"));
    }

    #[tokio::test]
    async fn reports_stt_slots() {
        let out = ListAudioCapabilitiesTool
            .execute(json!({ "model": "whisper-1", "kind": "stt" }))
            .await
            .expect("查询应成功");
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["output"], "text");
        assert_eq!(value["supported"]["file"]["transform"], "file_part_binary");
        assert_eq!(value["limits"]["maxBytes"], 26214400);
    }
}
