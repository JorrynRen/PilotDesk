//! 媒体节点执行器：图片生成 / 视频生成 / 语音合成 / 语音转写。
//!
//! 节点 config（存于 `WorkflowNode.params`）：
//! ```json
//! { "kind": "video|image|tts|stt", "provider": "pid", "model": "m", "args": { ... } }
//! ```
//! - `args` 的键与对应工具 `parameters()` 同名；未知键直接报错，绝不静默丢弃；
//! - 动态值先在节点 `input_mapping` 中声明，引擎解析后得到 `resolved_input`，
//!   本执行器再用 [`TemplateEngine::resolve_value`] 递归解析 `args` 中的占位符
//!   （单个占位符保留原始类型，文本内嵌则插值，未解析按空处理并记录日志）；
//! - `provider` / `model` 必填，不做隐式回退；
//! - 输出统一为 `{ content, urls, filePaths }`：`content` 与人工交互节点同名，便于复用默认输出映射；
//!   `filePaths` 为全部产物路径，`urls` 为全部远程结果 URL。

use crate::db::init::DbPool;
use crate::tools::generate_image::GenerateImageTool;
use crate::tools::generate_video::GenerateVideoTool;
use crate::tools::stt::SttTool;
use crate::tools::tts::TtsTool;
use crate::tools::{ToolArtifacts, ToolHandler};
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeExecutorTrait, NodeOutput};
use crate::workflow::template::TemplateEngine;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// 未配置 `node.timeout_ms` 时的执行器默认超时（与媒体任务的典型耗时匹配）。
const DEFAULT_IMAGE_TIMEOUT_MS: u64 = 180_000;
const DEFAULT_VIDEO_TIMEOUT_MS: u64 = 1_200_000;
const DEFAULT_TTS_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_STT_TIMEOUT_MS: u64 = 180_000;

/// 媒体节点执行器
pub struct MediaExecutor {
    pool: DbPool,
}

impl MediaExecutor {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    fn db_conn(&self) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
        self.pool
            .get()
            .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))
    }

    /// 工作根：项目主目录，未归属项目时回退全局工作区（与 Agent 节点口径一致）。
    fn resolve_work_root(&self, execution_id: &str) -> Result<String, AppError> {
        let conn = self.db_conn()?;
        let def = crate::workflow::events::derive_instance(&conn, execution_id)
            .ok()
            .flatten()
            .and_then(|inst| {
                crate::workflow::get_definition(&conn, &inst.definition_id)
                    .ok()
                    .flatten()
            });
        let workspace = if let Some(definition) = def {
            crate::commands::projects::resolve_workspace_context(
                &conn,
                crate::commands::projects::WorkspaceOwner::workflow(&definition.id),
            )?
            .workspace
        } else {
            crate::commands::projects::resolve_unowned_workspace(&conn)?
        };
        Ok(workspace.work_root.clone())
    }

    /// 跨 provider 解析闭包（key 只用于请求构造，绝不进入输出）。
    fn resolve_provider(&self) -> Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>> {
        let pool = self.pool.clone();
        Some(Arc::new(move |pid: &str| {
            let conn = pool.get().ok()?;
            let provider = crate::commands::api_provider::get_api_provider(&conn, pid)
                .ok()??;
            let key = crate::commands::api_provider::get_api_key(&conn, pid).ok()??;
            Some((provider.api_endpoint, key, provider.api_format))
        }))
    }

    /// 模型白名单闭包：provider_id → 合法模型名列表（数据源与 list_models 一致）。
    fn model_getter(&self) -> Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>> {
        let pool = self.pool.clone();
        Some(Arc::new(move |pid: String| {
            let Ok(conn) = pool.get() else {
                return Vec::new();
            };
            let mut names = Vec::new();
            for provider in crate::commands::api_provider::collect_provider_models(&conn) {
                if !pid.is_empty() && provider.provider_id != pid {
                    continue;
                }
                names.extend(provider.models.into_iter().map(|model| model.name));
            }
            names
        }))
    }

    /// 读取并校验节点配置基础字段。
    fn parse_config(node: &NodeDef) -> Result<(String, String, String, Map<String, Value>), AppError> {
        let kind = node
            .config
            .get("kind")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "media 节点缺少 kind（可选：image / video / tts / stt）".to_string(),
                )
            })?;
        if !matches!(kind, "image" | "video" | "tts" | "stt") {
            return Err(AppError::InvalidInput(format!(
                "media 节点 kind '{}' 无效（可选：image / video / tts / stt）",
                kind
            )));
        }

        let provider = node
            .config
            .get("provider")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput("media 节点缺少 provider（提供商 id）".to_string())
            })?
            .to_string();
        let model = node
            .config
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(|value| value.trim_start_matches('@').to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput("media 节点缺少 model（模型名）".to_string())
            })?;

        let args = match node.config.get("args") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(AppError::InvalidInput(
                    "media 节点的 args 必须是对象（参数名 → 值）".to_string(),
                ))
            }
        };
        for reserved in ["provider", "model"] {
            if args.contains_key(reserved) {
                return Err(AppError::InvalidInput(format!(
                    "media 节点的 args 不能包含 '{}'：provider / model 请在节点配置中单独填写",
                    reserved
                )));
            }
        }

        Ok((kind.to_string(), provider, model, args))
    }

    /// 按 kind 构造对应工具（不经过 build_registry，避免装配无关工具）。
    fn build_handler(
        &self,
        kind: &str,
        endpoint: String,
        api_key: String,
        provider: &str,
        work_root: &str,
        session_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<Arc<dyn ToolHandler>, AppError> {
        let resolve = self.resolve_provider();
        let get_models = self.model_getter();
        let handler: Arc<dyn ToolHandler> = match kind {
            "image" => Arc::new(GenerateImageTool::new(
                endpoint,
                api_key,
                resolve,
                Some(provider.to_string()),
                get_models,
                work_root.to_string(),
            )),
            "video" => Arc::new(GenerateVideoTool::new(
                endpoint,
                api_key,
                resolve,
                get_models,
                Some(emitter.clone()),
                session_id.to_string(),
                work_root.to_string(),
            )),
            "tts" => Arc::new(TtsTool::new(
                endpoint,
                api_key,
                resolve,
                get_models,
                work_root.to_string(),
                session_id.to_string(),
            )),
            "stt" => Arc::new(SttTool::new(endpoint, api_key, resolve, get_models)),
            other => {
                return Err(AppError::InvalidInput(format!(
                    "未知的媒体类型: {}",
                    other
                )))
            }
        };
        Ok(handler)
    }

    /// 默认超时（仅在节点未配置 timeout_ms 时由执行器兜底；配置了则由引擎加硬上限）。
    fn default_timeout_ms(kind: &str) -> u64 {
        match kind {
            "image" => DEFAULT_IMAGE_TIMEOUT_MS,
            "video" => DEFAULT_VIDEO_TIMEOUT_MS,
            "tts" => DEFAULT_TTS_TIMEOUT_MS,
            _ => DEFAULT_STT_TIMEOUT_MS,
        }
    }
}

#[async_trait]
impl NodeExecutorTrait for MediaExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let (kind, provider, model, raw_args) = Self::parse_config(node)?;

        // 变量解析：resolved_input 由引擎按 input_mapping 解析产出；args 递归解析其中的占位符。
        let context: HashMap<String, Value> = match &resolved_input {
            Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => HashMap::new(),
        };
        let mut resolved_args = match TemplateEngine::resolve_value(&Value::Object(raw_args), &context) {
            Value::Object(map) => map,
            _ => Map::new(),
        };

        // 读取提供商配置（数据库取 endpoint / key），并拒绝 Anthropic 协议。
        let (endpoint, api_key) = {
            let conn = self.db_conn()?;
            let provider_row = crate::commands::api_provider::get_api_provider(&conn, &provider)?
                .ok_or_else(|| {
                    AppError::NotFound(format!("提供商 [@{}] 不存在或未配置", provider))
                })?;
            if provider_row.api_format.eq_ignore_ascii_case("anthropic") {
                return Err(AppError::InvalidInput(format!(
                    "提供商 [@{}] 使用 Anthropic 协议，媒体节点仅支持 OpenAI 兼容协议",
                    provider
                )));
            }
            let api_key = crate::commands::api_provider::get_api_key(&conn, &provider)?
                .ok_or_else(|| {
                    AppError::Config(format!("提供商 [@{}] 未配置 API Key", provider))
                })?;
            (provider_row.api_endpoint, api_key)
        };

        let work_root = self.resolve_work_root(execution_id)?;
        let session_id = format!("wf_{}_{}", execution_id, node.id);
        let handler = self.build_handler(
            &kind,
            endpoint,
            api_key,
            &provider,
            &work_root,
            &session_id,
            emitter,
        )?;

        // 未知参数直接报错：args 键必须存在于该工具 parameters() 中。
        let allowed: Option<Vec<String>> = handler
            .parameters()
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect());
        if let Some(allowed) = &allowed {
            for key in resolved_args.keys() {
                if !allowed.iter().any(|name| name == key) {
                    return Err(AppError::InvalidInput(format!(
                        "media 节点 kind={} 不支持参数 '{}'（支持：{}）",
                        kind,
                        key,
                        allowed.join("、")
                    )));
                }
            }
        }

        resolved_args.insert("provider".to_string(), json!(provider));
        resolved_args.insert("model".to_string(), json!(model));

        // 节点配置了 timeout_ms 时由引擎加硬上限，执行器不再叠加默认超时。
        let call = handler.execute_structured(Value::Object(resolved_args));
        let (text, artifacts): (String, ToolArtifacts) =
            if node.timeout_ms.filter(|ms| *ms > 0).is_some() {
                call.await
            } else {
                let default_ms = Self::default_timeout_ms(&kind);
                tokio::time::timeout(Duration::from_millis(default_ms), call)
                    .await
                    .map_err(|_| {
                        AppError::External(format!(
                            "媒体节点执行超时（默认 {}ms）",
                            default_ms
                        ))
                    })?
            }
            .map_err(|error| AppError::External(format!("媒体任务失败: {}", error)))?;

        // 产物目录：首个本地文件的父目录。
        let artifacts_path = artifacts
            .local_paths
            .first()
            .and_then(|path| Path::new(path).parent())
            .map(|parent| parent.to_string_lossy().to_string());

        let file_paths: Vec<String> = artifacts.files.iter().map(|f| f.path.clone()).collect();
        let output = json!({
            "content": text,
            "urls": artifacts.urls,
            "filePaths": file_paths,
        });

        Ok(NodeOutput {
            output,
            session_id: None,
            input_data: None,
            artifacts_path,
        })
    }
}
