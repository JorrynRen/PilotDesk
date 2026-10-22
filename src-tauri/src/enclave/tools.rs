//! 注入 Agent 的**只读**团队知识库工具。
//!
//! 三层防线中的第三层：每个工具 `execute` 首行**再查一次**出境闸门——
//! 即使装配时允许，会话中途策略被改成「不允许」也会立刻失败，不读任何数据。
//!
//! 只读白名单：仅 `search_org_knowledge` / `read_org_knowledge_entry`，
//! 写动作在工具层不可达（Agent 不提权）。
//!
//! 主机 / 成员两种运行形态：
//! - 主机本机：直接读 `enclave.db`，策略读本地 `org_policy`；
//! - 成员机：不缓存团队数据，检索与读取经局域网 WS 转发到主机，策略也以主机为准。

use std::sync::Arc;

use serde_json::json;

use crate::enclave::db::EnclaveStore;
use crate::enclave::gate::{self, TrustDomain};
use crate::enclave::{host, knowledge, org, EnclaveState};
use crate::tools::{RiskLevel, ToolHandler};

/// 工具体共用的闸门复核（主机本机）。
fn allowed_now(
    store: &EnclaveStore,
    org_id: &str,
    trust: TrustDomain,
) -> Result<bool, String> {
    store.with_conn(|c| {
        let policy = crate::enclave::org::get_allow_cloud_model(c, org_id)?;
        Ok(gate::team_content_injectable(
            trust,
            policy.unwrap_or(false),
            policy.is_some(),
        ))
    })
}

fn is_local_host(store: &EnclaveStore, org_id: &str) -> bool {
    store
        .with_conn(|c| org::host_org_ids(c, &super::host::device_id()))
        .map(|ids| ids.iter().any(|o| o == org_id))
        .unwrap_or(false)
}

/// 执行前复核：主机读本地策略；成员向主机查询策略（`policy.get`）。
async fn ensure_injectable(
    store: &EnclaveStore,
    state: Option<&Arc<EnclaveState>>,
    org_id: &str,
    trust: TrustDomain,
) -> Result<(), String> {
    if is_local_host(store, org_id) {
        return if allowed_now(store, org_id, trust)? {
            Ok(())
        } else {
            Err(DENIED.to_string())
        };
    }
    let Some(state) = state else {
        return Err(DENIED.to_string());
    };
    let data = host::call_host_without_token(state, org_id, "policy.get", json!({})).await?;
    let allow = data["allowCloudModel"].as_bool().unwrap_or(false);
    let explicit = data["explicit"].as_bool().unwrap_or(false);
    if gate::team_content_injectable(trust, allow, explicit) {
        Ok(())
    } else {
        Err(DENIED.to_string())
    }
}

const DENIED: &str = "团队数据出境未授权：当前会话可能发往云端模型，而组织未开启「团队数据可发送至云端模型」。请在飞地「组织与权限」页确认策略。";

/// 检索组织知识库（只读）。
pub struct SearchOrgKnowledgeTool {
    store: Arc<EnclaveStore>,
    org_id: String,
    trust: TrustDomain,
    state: Option<Arc<EnclaveState>>,
}

impl SearchOrgKnowledgeTool {
    pub fn new(
        store: Arc<EnclaveStore>,
        org_id: String,
        trust: TrustDomain,
        state: Option<Arc<EnclaveState>>,
    ) -> Self {
        Self {
            store,
            org_id,
            trust,
            state,
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for SearchOrgKnowledgeTool {
    fn name(&self) -> &str {
        "search_org_knowledge"
    }

    fn description(&self) -> &str {
        "检索当前组织的团队知识库（任务产出自动沉淀的内容）。只读；数据始终留在内网/本机。\
         当问题涉及团队任务、项目交付、内部规范或历史产出时优先使用。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "检索关键词（支持中文子串）" },
                "limit": { "type": "integer", "description": "返回条数上限，默认 10，最大 50" }
            },
            "required": ["query"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        ensure_injectable(&self.store, self.state.as_ref(), &self.org_id, self.trust).await?;
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(10)
            .clamp(1, 50);

        // 成员机：转发到主机检索（主机按成员身份与策略执行）
        if let (Some(state), false) = (&self.state, is_local_host(&self.store, &self.org_id)) {
            let data = host::call_host_without_token(
                state,
                &self.org_id,
                "kb.search",
                json!({ "query": query, "limit": limit }),
            )
            .await?;
            let entries: Vec<serde_json::Value> = serde_json::from_value(data)
                .map_err(|e| format!("解析主机知识库结果失败：{}", e))?;
            let brief: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "memoryId": e["memoryId"],
                        "title": e["title"],
                        "snippet": truncate_chars(e["content"].as_str().unwrap_or(""), 400),
                        "tags": e["tags"],
                        "addedAt": e["addedAt"],
                    })
                })
                .collect();
            return Ok(json!({ "entries": brief, "count": brief.len() }).to_string());
        }

        let entries = self.store.with_conn(|c| {
            knowledge::search_entries(c, &self.org_id, None, &query, limit, 0)
        })?;
        let brief: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "memoryId": e.memory_id,
                    "title": e.title,
                    "snippet": truncate_chars(&e.content, 400),
                    "tags": e.tags,
                    "addedAt": e.added_at,
                })
            })
            .collect();
        Ok(json!({ "entries": brief, "count": brief.len() }).to_string())
    }
}

/// 读取组织知识库条目的完整正文（只读）。
pub struct ReadOrgKnowledgeEntryTool {
    store: Arc<EnclaveStore>,
    org_id: String,
    trust: TrustDomain,
    state: Option<Arc<EnclaveState>>,
}

impl ReadOrgKnowledgeEntryTool {
    pub fn new(
        store: Arc<EnclaveStore>,
        org_id: String,
        trust: TrustDomain,
        state: Option<Arc<EnclaveState>>,
    ) -> Self {
        Self {
            store,
            org_id,
            trust,
            state,
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ReadOrgKnowledgeEntryTool {
    fn name(&self) -> &str {
        "read_org_knowledge_entry"
    }

    fn description(&self) -> &str {
        "读取团队知识库中某条目的完整正文（先用 search_org_knowledge 拿到 memoryId）。只读。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "memoryId": { "type": "string", "description": "条目 id（search 结果中的 memoryId）" },
                "kbId": { "type": "string", "description": "知识库 id；缺省用组织的隐式库" }
            },
            "required": ["memoryId"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        ensure_injectable(&self.store, self.state.as_ref(), &self.org_id, self.trust).await?;
        let memory_id = arguments
            .get("memoryId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "缺少 memoryId".to_string())?;
        let kb_id = arguments
            .get("kbId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| knowledge::implicit_kb_id(&self.org_id));

        // 成员机：转发到主机读取
        if let (Some(state), false) = (&self.state, is_local_host(&self.store, &self.org_id)) {
            let data = host::call_host_without_token(
                state,
                &self.org_id,
                "kb.entry_get",
                json!({ "kbId": kb_id, "memoryId": memory_id }),
            )
            .await?;
            let entry: Option<serde_json::Value> = serde_json::from_value(data)
                .map_err(|e| format!("解析主机知识条目失败：{}", e))?;
            return match entry {
                Some(e) => Ok(json!({
                    "memoryId": e["memoryId"],
                    "title": e["title"],
                    "content": e["content"],
                    "tags": e["tags"],
                    "sourceRef": e["sourceRef"],
                    "addedAt": e["addedAt"],
                })
                .to_string()),
                None => Err("条目不存在或不属于该组织".to_string()),
            };
        }

        let entry = self.store.with_conn(|c| {
            knowledge::get_entry(c, &self.org_id, &kb_id, memory_id)
        })?;
        match entry {
            Some(e) => Ok(json!({
                "memoryId": e.memory_id,
                "title": e.title,
                "content": e.content,
                "tags": e.tags,
                "sourceRef": e.source_ref,
                "addedAt": e.added_at,
            })
            .to_string()),
            None => Err("条目不存在或不属于该组织".to_string()),
        }
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}
