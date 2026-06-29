use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};
use super::super::template::TemplateEngine;

/// HTTP API 调用执行器
pub struct ApiExecutor {
    http_client: reqwest::Client,
}

impl ApiExecutor {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl NodeExecutorTrait for ApiExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let url = node.config.get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Config("api 节点缺少 url 配置".into()))?;

        let method = node.config.get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let timeout_secs = node.config.get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        // 将 resolved_input 作为模板上下文
        let mut template_ctx = HashMap::new();
        if !resolved_input.is_null() {
            if let Some(obj) = resolved_input.as_object() {
                for (k, v) in obj {
                    template_ctx.insert(k.clone(), v.clone());
                }
            }
            template_ctx.insert("__input__".to_string(), resolved_input.clone());
        }

        let request = match method.to_uppercase().as_str() {
            "GET" => self.http_client.get(url),
            "POST" => {
                let raw_body = node.config.get("body_template")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let body = TemplateEngine::resolve(raw_body, &template_ctx)
                    .unwrap_or_else(|_| raw_body.to_string());
                self.http_client.post(url)
                    .header("Content-Type", "application/json")
                    .body(body)
            }
            "PUT" => {
                let raw_body = node.config.get("body_template")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let body = TemplateEngine::resolve(raw_body, &template_ctx)
                    .unwrap_or_else(|_| raw_body.to_string());
                self.http_client.put(url)
                    .header("Content-Type", "application/json")
                    .body(body)
            }
            "DELETE" => self.http_client.delete(url),
            _ => return Err(AppError::Config(format!("不支持的 HTTP 方法: {}", method))),
        };

        let response = request
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| AppError::Network(format!("HTTP 请求失败: {}", e)))?;

        let status = response.status();
        let body = response.text().await
            .map_err(|e| AppError::Network(format!("读取响应失败: {}", e)))?;

        if !status.is_success() {
            return Err(AppError::Network(format!(
                "HTTP {} 响应: {} - {}", status.as_u16(), status.canonical_reason().unwrap_or(""), body
            )));
        }

        // 尝试解析为 JSON
        let output = serde_json::from_str::<Value>(&body)
            .unwrap_or(Value::String(body));

        Ok(NodeOutput { output, session_id: None })
    }
}
