//! 数据出境闸门（纯函数，可单测）。
//!
//! 语义：**团队数据可发送至云端模型**（组织级总闸，默认不允许）。
//! - `Internal`（内网 / 本机模型）：数据不出内网 → 允许注入，无需开关；
//! - `Cloud`（云端 Provider）：必须显式开启 `allow_cloud_model` 且策略已加载；
//! - 无法确认（策略缺失 / 未知信任域 / 离线）→ **最严档：拒绝**（fail-safe）。

use serde::{Deserialize, Serialize};

/// Provider 信任域。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustDomain {
    /// 云端服务商
    Cloud,
    /// 内网 / 本机（Ollama、TEI、自建 OpenAI 兼容服务等）
    Internal,
}

impl TrustDomain {
    pub fn parse(raw: &str) -> Option<TrustDomain> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cloud" => Some(TrustDomain::Cloud),
            "internal" => Some(TrustDomain::Internal),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TrustDomain::Cloud => "cloud",
            TrustDomain::Internal => "internal",
        }
    }

    /// 按 `baseUrl` 主机做启发式判断：回环 / 私网地址视为内网。
    pub fn heuristic(base_url: &str) -> TrustDomain {
        let host = base_url
            .split("://")
            .nth(1)
            .unwrap_or(base_url)
            .split('/')
            .next()
            .unwrap_or("")
            .rsplit('@')
            .next()
            .unwrap_or("");
        // 去掉端口
        let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        let host = host.trim_start_matches('[').trim_end_matches(']');

        let internal = host == "localhost"
            || host.ends_with(".local")
            || host == "::1"
            || host.starts_with("127.")
            || host.starts_with("10.")
            || host.starts_with("192.168.")
            || is_private_172(host)
            || host.ends_with(".internal");
        if internal {
            TrustDomain::Internal
        } else {
            TrustDomain::Cloud
        }
    }
}

/// 172.16.0.0/12
fn is_private_172(host: &str) -> bool {
    let mut parts = host.split('.');
    if parts.next() != Some("172") {
        return false;
    }
    matches!(parts.next().and_then(|s| s.parse::<u8>().ok()), Some(16..=31))
}

/// 团队内容能否注入到该会话（唯一判定口径）。
///
/// `policy_loaded = false`（本地还没有该组织的策略行）时，云端一律不可注入。
pub fn team_content_injectable(
    trust: TrustDomain,
    allow_cloud_model: bool,
    policy_loaded: bool,
) -> bool {
    match trust {
        TrustDomain::Internal => true,
        TrustDomain::Cloud => policy_loaded && allow_cloud_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真值表：内网永远允许；云端必须「策略已加载 + 显式开启」。
    #[test]
    fn inject_truth_table() {
        assert!(team_content_injectable(TrustDomain::Internal, false, false));
        assert!(team_content_injectable(TrustDomain::Internal, true, true));
        assert!(!team_content_injectable(TrustDomain::Cloud, false, true));
        assert!(!team_content_injectable(TrustDomain::Cloud, true, false));
        assert!(team_content_injectable(TrustDomain::Cloud, true, true));
    }

    /// 启发式：回环 / 私网 / .local 视为内网，公网域名为云端。
    #[test]
    fn heuristic_hosts() {
        for u in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:8000",
            "https://10.0.0.5/v1",
            "https://192.168.1.2:1234/v1",
            "https://172.20.3.4/v1",
            "http://ollama.local/v1",
        ] {
            assert_eq!(TrustDomain::heuristic(u), TrustDomain::Internal, "{u}");
        }
        for u in [
            "https://api.openai.com/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://172.32.0.1/v1",
        ] {
            assert_eq!(TrustDomain::heuristic(u), TrustDomain::Cloud, "{u}");
        }
    }
}
