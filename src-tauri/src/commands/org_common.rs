//! 组织类平台调用共享辅助：组织共享空间（`org_share`）/ 组织共享凭据（`org_credentials`）
//! / 飞地治理快照（`enclave::snapshot`）共用同一套令牌获取与错误映射口径。
//!
//! 平台侧接口（全部 `requireAuth`，产品端用 Bearer）令牌均复用 [`current_access_token`]
//! （读 `account_tokens` + 自动续期）；未登录时返回明确中文提示。

use crate::commands::account::current_access_token;
use crate::utils::platform;

/// 取一个可用的 Bearer 令牌；未登录 / 续期失败给出明确中文提示。
/// 供组织共享空间与组织共享凭据（`org_credentials`）复用。
pub(crate) async fn require_token(state: &crate::DbState) -> Result<String, String> {
    match current_access_token(state).await {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err("请先登录平台账号".to_string()),
        Err(e) => Err(format!("获取登录状态失败：{}", e)),
    }
}

/// 平台错误 → 面向用户的中文文案（对共享空间的 403 / 409 做友好映射）。
pub(crate) fn map_platform_error(err: platform::PlatformError) -> String {
    match err.status {
        // 403：组织档位不含 team.shared-space（free / pro 组织）
        403 => "当前组织档位不支持共享空间（需要团队版）".to_string(),
        // 409：同稳定 ID 内容有改动、需确认覆盖（正常路径由 org_share_workflow 解析 existing 后处理）
        409 => "共享内容与云端不一致，需确认后才能覆盖".to_string(),
        // 401：令牌失效 / 未登录
        401 => "请先登录平台账号".to_string(),
        // 其余（含 status=0 的网络 / 解析错误）直接透传（正文已带中文前缀）
        _ => err.message,
    }
}
