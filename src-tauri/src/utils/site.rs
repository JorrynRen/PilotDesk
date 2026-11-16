//! 官网站点信息 —— 客户端所有「官网地址」的唯一来源。
//!
//! 两处消费方：
//! 1. 资源市集的官网源：官网按 `/<slug>/server/market/**` 对外托管市场数据
//!    （契约见官网仓库 `docs/url-contract.md`）；
//! 2. 「关于」页的官方网站外链。
//!
//! 换域名只改 `OFFICIAL_SITE` 一行，其余全部派生。

/// 官网基址（不带结尾斜杠）。
pub const OFFICIAL_SITE: &str = "https://ddab4935ed78.eicp.fun";

/// 本产品在官网的 slug：市场数据位于 `/<slug>/server/market/**`。
pub const MARKET_SLUG: &str = "pilotdesk";

/// 官网基址（归一化掉结尾斜杠，便于拼接）。
pub fn official_site() -> &'static str {
    OFFICIAL_SITE.trim_end_matches('/')
}

/// 市场数据的官网源基址：`<官网>/<slug>`，其后直接接 `/server/market/**`。
pub fn official_market_base() -> String {
    format!("{}/{}", official_site(), MARKET_SLUG)
}
