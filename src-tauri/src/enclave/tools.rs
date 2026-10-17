//! 注入 Agent 的**只读**团队知识库工具。
//!
//! 三层防线中的第三层：每个工具 `execute` 首行**再查一次**出境闸门——
//! 即使装配时允许，会话中途策略被改成「不允许」也会立刻失败，不读任何数据。
//!
//! 只读白名单：仅 `search_org_knowledge` / `read_org_knowledge_entry`，
//! 写动作在工具层不可达（Agent 不提权）。

use std::sync::Arc;

use serde_json::json;

use crate::enclave::db::EnclaveStore;
use crate::enclave::gate::{self, TrustDomain};
use crate::enclave::knowledge;
use crate::tools::{RiskLevel, ToolHandler};

/// 工具体共用的闸门复核。
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

const DENIED: &str = "团队数据出境未授权：当前会话可能发往云端模型，而组织未开启「团队数据可发送至云端模型」。请在飞地「组织与权限」页确认策略。";

/// 检索组织知识库（只读）。
pub struct SearchOrgKnowledgeTool {
    store: Arc<EnclaveStore>,
    org_id: String,
    trust: TrustDomain,
}

impl SearchOrgKnowledgeTool {
    pub fn new(store: Arc<EnclaveStore>, org_id: String, trust: TrustDomain) -> Self {
        Self {
            store,
            org_id,
            trust,
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for SearchOrgKnowledgeTool {
    fn name(&self) -> &str {
        "search_org_knowledge"
    }

    fn description(&self) -> &str {
        "检索当前组织的团队知识库（任务产出自动沉淀的内容）。只读；数据始终留在本机。\
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
        if !allowed_now(&self.store, &self.org_id, self.trust)? {
            return Err(DENIED.to_string());
        }
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
        let entries = self.store.with_conn(|c| {
            knowledge::search_entries(c, &self.org_id, None, &query, limit)
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
}

impl ReadOrgKnowledgeEntryTool {
    pub fn new(store: Arc<EnclaveStore>, org_id: String, trust: TrustDomain) -> Self {
        Self {
            store,
            org_id,
            trust,
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
        if !allowed_now(&self.store, &self.org_id, self.trust)? {
            return Err(DENIED.to_string());
        }
        let memory_id = arguments
            .get("memoryId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "缺少 memoryId".to_string())?;
        let kb_id = arguments
            .get("kbId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| knowledge::implicit_kb_id(&self.org_id));
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
