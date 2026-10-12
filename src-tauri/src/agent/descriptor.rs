//! Agent 描述符：把两张表（`agents` / `api_providers`）统一成**同一种内存模型**。
//!
//! 对应设计文档 §4.1 的第三个契约、实施计划 §七 的「**步骤 A：逻辑统一**」——
//! **零 DDL**：不新增列、不迁移，只在读取时把两种行映射成同一个 `AgentDescriptor`。
//! 上层（会话 / 工作流 / 圆桌 / 插件）据此**只认一种模型**，为步骤 B 的物理合并铺路。
//!
//! 两条硬约束（设计文档 §4.3）：
//! 1. **密钥不进 `conn`** —— `api_providers` 的 key 由调用方在运行时经
//!    `AgentConn.secret` 注入，描述符本身可安全序列化/下发。
//! 2. **模型清单只在 `http` 描述符里** —— 一个供应商下可多模型，
//!    具体用哪个由会话选定（`model` 参数），不在描述符里定死。

// 本模块尚未接线（传输路由在下一步落地），故暂整体 `allow(dead_code)`；
// 接线完成后删除该 allow。单元测试已覆盖映射口径。
#![allow(dead_code)]

use serde_json::json;

use crate::agent::config::AgentConfig;
use crate::agent::transport::TransportKind;
use crate::commands::api_provider::ApiProvider;

/// 一个可调用对象的统一描述（**不含密钥**）。
#[derive(Debug, Clone)]
pub struct AgentDescriptor {
    /// 统一标识：CLI 为 `agents.agent_type`；在线版为 `api:<provider_id>`
    pub agent_id: String,
    /// 展示名
    pub name: String,
    /// 接入方式
    pub transport: TransportKind,
    /// transport 专属参数（JSON）；**不含密钥**
    pub conn: serde_json::Value,
}

impl AgentDescriptor {
    /// CLI Agent 行 → 描述符。
    ///
    /// P1 阶段一律视为 `subprocess`：`agents.transport` 列要到**步骤 B** 才加，
    /// 届时此处改为"读列 + 缺省 subprocess"。
    pub fn from_agent_config(cfg: &AgentConfig) -> Self {
        Self {
            agent_id: cfg.agent_type.clone(),
            name: cfg.display_name.clone(),
            transport: TransportKind::Subprocess,
            conn: json!({ "cliCommand": cfg.cli_command }),
        }
    }

    /// API 供应商行 → 描述符（`transport = http`）。
    ///
    /// `model` 为会话选定的模型名；`None` 表示尚未选定（此时描述符仍合法，
    /// 但 `http` 传输在 `connect` 时会要求 `conn.model` 非空）。
    pub fn from_api_provider(p: &ApiProvider, model: Option<&str>) -> Self {
        let mut conn = json!({
            "baseUrl": p.api_endpoint,
            "apiFormat": p.api_format,
            "models": p.models,
        });
        if let Some(m) = model {
            conn["model"] = json!(m);
        }
        Self {
            agent_id: format!("api:{}", p.id),
            name: p.name.clone(),
            transport: TransportKind::Http,
            conn,
        }
    }

    /// 是否具备自动调用能力。
    ///
    /// `webview`（内嵌承载，对应设计文档 §1.1 的 L2）返回 `false`——
    /// UI 据此把它排除在会话选择器之外或置灰。
    pub fn is_automatable(&self) -> bool {
        !matches!(self.transport, TransportKind::Webview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider() -> ApiProvider {
        ApiProvider {
            id: "p1".into(),
            name: "DeepSeek".into(),
            api_endpoint: "https://api.deepseek.com/v1".into(),
            api_key_masked: "sk-***".into(),
            api_key_set: true,
            models: vec!["deepseek-chat".into(), "deepseek-reasoner".into()],
            api_format: "openai".into(),
            sort_order: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// CLI 行缺省映射为 subprocess（步骤 B 加列前的口径）。
    #[test]
    fn descriptor_from_cli_agent_defaults_to_subprocess() {
        let cfg = AgentConfig {
            agent_type: "claude".into(),
            display_name: "Claude Code".into(),
            cli_command: "claude".into(),
            ..Default::default()
        };
        let d = AgentDescriptor::from_agent_config(&cfg);
        assert_eq!(d.agent_id, "claude");
        assert_eq!(d.name, "Claude Code");
        assert_eq!(d.transport, TransportKind::Subprocess);
        assert!(d.is_automatable());
    }

    /// 供应商行映射为 http，且**密钥绝不进 conn**（只有 baseUrl / apiFormat / models / model）。
    #[test]
    fn descriptor_from_provider_is_http_without_secret() {
        let p = sample_provider();
        let d = AgentDescriptor::from_api_provider(&p, Some("deepseek-chat"));
        assert_eq!(d.agent_id, "api:p1");
        assert_eq!(d.transport, TransportKind::Http);
        assert_eq!(d.conn["baseUrl"], "https://api.deepseek.com/v1");
        assert_eq!(d.conn["apiFormat"], "openai");
        assert_eq!(d.conn["model"], "deepseek-chat");
        assert!(d.conn.get("apiKey").is_none());
        assert!(d.conn.get("secret").is_none());
    }

    /// 未选定模型时描述符仍合法，只是不带 model 键。
    #[test]
    fn descriptor_without_model_omits_model_key() {
        let p = sample_provider();
        let d = AgentDescriptor::from_api_provider(&p, None);
        assert!(d.conn.get("model").is_none());
    }

    /// `webview` 属承载类，不可自动调用（L2）。
    #[test]
    fn descriptor_webview_is_not_automatable() {
        let d = AgentDescriptor {
            agent_id: "web".into(),
            name: "Web".into(),
            transport: TransportKind::Webview,
            conn: json!({}),
        };
        assert!(!d.is_automatable());
    }
}
