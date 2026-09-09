//! 文本向量化工具（embed）：调用 OpenAI 兼容 `POST /embeddings`。
//!
//! 模型选择走 list_models（LLM 决策）+ resolve_provider 跨 provider 解析；key 只进请求构造。
//! 支持批量与维度透传（dimensions，依赖模型支持 Matryoshka 截断）。
//! BERT/Sentence-BERT 等经 TEI/Ollama 等 OpenAI 兼容服务接入（在 API Provider 配置登记 + 备注）。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

pub struct EmbedTool {
    api_endpoint: String,
    api_key: String,
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
}

impl EmbedTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    ) -> Self {
        Self { api_endpoint, api_key, resolve, get_models }
    }

    /// 校验 model 是否在当前 provider 的合法模型列表中，不在则返回引导错误。
    fn validate_model(&self, model: &str, provider_id: &str) -> Result<(), String> {
        if let Some(get_models) = &self.get_models {
            let valid: Vec<String> = get_models(provider_id.to_string());
            if !valid.is_empty() && !valid.iter().any(|m| m == model) {
                return Err(format!(
                    "模型 '{}' 不是提供商 '@{}' 的有效模型。\
                     有效模型：{}\n请重新调用 list_models，从返回结果中完整复制模型名（含 namespace 前缀如 TeleAI/xxx）后重试。",
                    model, provider_id, valid.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// 向量输出文本化（避免超长 JSON）：每个向量按维度截断展示为 `[0.1, 0.2, ... ]（N 维）`。
fn format_vectors(vectors: &[Vec<f64>], max_dims: usize) -> String {
    let mut out = String::new();
    for (i, v) in vectors.iter().enumerate() {
        let shown: Vec<String> = v.iter().take(max_dims).map(|x| format!("{:.4}", x)).collect();
        out.push_str(&format!(
            "[{}] 向量{}: [{}]（{} 维{}）\n",
            i + 1,
            i + 1,
            shown.join(", "),
            v.len(),
            if v.len() > max_dims { format!("，前 {} 维示例", max_dims) } else { String::new() }
        ));
    }
    out
}

#[async_trait]
impl ToolHandler for EmbedTool {
    fn name(&self) -> &str {
        "embed"
    }

    fn description(&self) -> &str {
        "文本向量化（embedding）：将文本转换为数值向量，供相似度检索/聚类/语义分析使用。\
         使用 OpenAI 兼容的 /embeddings 接口。调用前请先调用 list_models 查看可用模型，\
         选择支持向量化的模型名与提供商（model 为纯模型名，不带 @ 前缀，勿编造；\
         如 bge-large-zh、text-embedding-3-small 等，可在模型备注中标注向量化用途）。\
         input 支持单条或数组批量；dimensions 可指定输出维度（依赖模型支持）。\
         若连续 2 次调用失败（如 HTTP 4xx/5xx、模型不支持向量化），请停止自动重试，向用户确认三选一：\
         继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "要向量化的文本；支持 JSON 数组形式传入多条（如 [\"a\",\"b\"]，最多 32 条）"
                },
                "provider": {
                    "type": "string",
                    "description": "可选：目标提供商 id（先用 list_models 查看可用提供商）；省略时使用当前会话提供商"
                },
                "model": {
                    "type": "string",
                    "description": "向量化模型名（必填，纯模型名不带 @ 前缀）：先调用 list_models 从清单选择支持向量化的模型"
                },
                "dimensions": {
                    "type": "integer",
                    "description": "可选：输出向量维度（如 512/1024/3072）；依赖模型支持维度截断，不支持时接口将报错"
                },
                "batch_size": {
                    "type": "integer",
                    "description": "可选：批量并发上限，默认 16，最大 32"
                }
            },
            "required": ["input", "model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Network, ToolTag::Read, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        // input：字符串或数组
        let texts: Vec<String> = if let Some(arr) = arguments["input"].as_array() {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        } else if let Some(s) = arguments["input"].as_str() {
            vec![s.to_string()]
        } else {
            return Err("缺少 input 参数".to_string());
        };
        if texts.is_empty() {
            return Err("input 不能为空".to_string());
        }
        if texts.len() > 32 {
            return Err(format!("批量输入超过上限（32 条，当前 {} 条）", texts.len()));
        }

        let provider_arg = arguments["provider"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (endpoint_base, api_key) = match (&provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, _fmt)) => (ep, key),
                None => return Err(format!("提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商", pid)),
            },
            _ => (self.api_endpoint.clone(), self.api_key.clone()),
        };

        // 校验 model 是否在合法清单内
        let provider_id = provider_arg.as_deref().unwrap_or("__default__");
        self.validate_model(&model, provider_id)?;

        let endpoint = format!("{}/embeddings", endpoint_base.trim_end_matches('/'));
        let mut body = serde_json::json!({
            "model": model,
            "input": texts,
        });
        if let Some(dim) = arguments["dimensions"].as_u64() {
            body["dimensions"] = serde_json::json!(dim);
        }
        let _ = arguments["batch_size"].as_u64(); // 输入量小（≤32），单请求即可，batch 由服务端处理

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("向量化请求失败: {}", e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "向量化失败 (HTTP {}): {}. 可能原因：模型不支持向量化、指定的 dimensions 不被支持。",
                status.as_u16(),
                &text[..text.len().min(300)]
            ));
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析向量结果失败: {}", e))?;
        let data = json["data"]
            .as_array()
            .ok_or_else(|| format!("响应缺少 data 数组: {}", &text[..text.len().min(200)]))?;

        let mut vectors: Vec<Vec<f64>> = Vec::new();
        for item in data {
            if let Some(embedding) = item["embedding"].as_array() {
                let v: Vec<f64> = embedding
                    .iter()
                    .filter_map(|x| x.as_f64())
                    .collect();
                if !v.is_empty() {
                    vectors.push(v);
                }
            }
        }
        if vectors.is_empty() {
            return Err("响应中没有有效向量".to_string());
        }

        let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
        let rendered = format_vectors(&vectors, 12);
        Ok(format!(
            "已生成 {} 条向量（维度 {}，模型见上）：\n{}",
            vectors.len(),
            dims,
            rendered
        ))
    }
}
