/// 在线商店服务器源（按优先级排列）
/// 1. jsdelivr CDN（主源，速度快，全球加速）
/// 2. GitHub Raw（降级源，直连仓库）
pub const SERVER_SOURCES: &[&str] = &[
    "https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main",
    "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main",
];

/// 在线商店路径
#[allow(dead_code)]
pub const AGENTS_CONFIG_PATH: &str = "/server/market/agents-config/agents-config.json";
pub const PLUGINS_INDEX_PATH: &str = "/server/market/plugins/index.json";
pub const PLUGINS_DIR_PATH: &str = "/server/market/plugins";

/// 根据路径构建所有服务器源的完整 URL 列表
pub fn build_urls(path: &str) -> Vec<String> {
    SERVER_SOURCES
        .iter()
        .map(|base| format!("{}{}", base.trim_end_matches('/'), path))
        .collect()
}
