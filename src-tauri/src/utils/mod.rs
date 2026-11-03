pub mod crypto;
pub mod errors;
pub mod paths;
pub mod text;

/// 生成 UUID v4 字符串 ID
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 获取当前 Unix 时间戳（秒）
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 获取当前 Unix 时间戳（毫秒）
pub fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 当前真实时钟锚点（动态获取，以用户设备时区为准）：拼接在用户消息进入 LLM 上下文时，
/// 供 LLM 正确处理"今天/最近N天/上周"等相对时间与"现在/当前时间"等准确时间表达
/// （避免使用模型内部时间作参照）。格式为 ISO 风格（含 UTC 偏移 + 星期 + IANA 时区），便于计算。
pub fn current_clock_cn() -> String {
    use chrono::{Datelike, Offset, Timelike};
    let now = chrono::Local::now();
    let (_, weekday_en) = match now.weekday() {
        chrono::Weekday::Mon => ("星期一", "Mon"),
        chrono::Weekday::Tue => ("星期二", "Tue"),
        chrono::Weekday::Wed => ("星期三", "Wed"),
        chrono::Weekday::Thu => ("星期四", "Thu"),
        chrono::Weekday::Fri => ("星期五", "Fri"),
        chrono::Weekday::Sat => ("星期六", "Sat"),
        chrono::Weekday::Sun => ("星期日", "Sun"),
    };
    // UTC 偏移（如 +08:00）与 UTC 名称（如 UTC+8 / UTC+5:30）：按设备时区动态计算。
    let off_secs = now.offset().fix().local_minus_utc();
    let sign = if off_secs < 0 { '-' } else { '+' };
    let abs = off_secs.abs();
    let off_str = format!("{}{:02}:{:02}", sign, abs / 3600, (abs % 3600) / 60);
    let utc_str = if abs % 3600 == 0 {
        format!("UTC{}{}", sign, abs / 3600)
    } else {
        format!("UTC{}{:02}:{:02}", sign, abs / 3600, (abs % 3600) / 60)
    };
    // 系统时区名（IANA，如 Asia/Shanghai）；获取失败回退"Local"（用户设备设置），不硬编码。
    let tz_name = iana_time_zone::get_timezone().unwrap_or_else(|_| "Local".to_string());
    format!(
        "【当前真实时钟】{:04}-{:02}-{:02} {:02}:{:02}:{:02}（{}），{}（{}，{}）",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        off_str,
        weekday_en,
        tz_name,
        utc_str,
    )
}

/// 给**本轮用户输入**前置真实时钟锚点（可选），并标出本轮指令边界。
///
/// 用于 CLI / ACP 会话：宿主按自身回合结构区分用户消息，`<user_request>` 在链路里没有
/// 边界语义；关闭时间感知时**整体原样返回**（不追加时钟、也不追加标签），避免多轮对话里
/// 往宿主历史累积大量无用标签。
///
/// 只用于送入模型的本轮输入，**不得写库、也不得回放历史**：历史里带实时时钟会让同一条
/// 逻辑消息随秒漂移、破坏前缀缓存（群聊记忆层已因同一原因撤掉渲染期注入）。
pub fn with_clock_anchor(body: &str, include_clock: bool) -> String {
    if include_clock {
        format!(
            "{}\n\n<user_request>\n{}\n</user_request>",
            current_clock_cn(),
            body
        )
    } else {
        body.to_string()
    }
}

/// API 会话的本轮用户输入：`<user_request>` 边界**恒有**，时钟按开关追加。
///
/// API 链路的历史消息以原文回放、不带任何标记，唯一带标签的就是本轮请求；标签因此不可
/// 随开关移除，否则模型会把历史里已处理过的指令当成新一轮请求。
pub fn wrap_user_request(body: &str, include_clock: bool) -> String {
    let envelope = format!("<user_request>\n{}\n</user_request>", body);
    if include_clock {
        format!("{}\n\n{}", current_clock_cn(), envelope)
    } else {
        envelope
    }
}
pub mod market;
pub mod process;
pub mod platform;

#[cfg(test)]
mod tests {
    use super::*;

    /// API 会话：`<user_request>` 边界恒有，时钟按开关追加。
    #[test]
    fn wrap_user_request_keeps_envelope_when_clock_off() {
        let body = "修复登录问题";
        let without_clock = wrap_user_request(body, false);
        assert!(!without_clock.contains("【当前真实时钟】"));
        assert_eq!(without_clock, "<user_request>\n修复登录问题\n</user_request>");

        let with_clock = wrap_user_request(body, true);
        assert!(with_clock.contains("【当前真实时钟】"));
        assert!(with_clock.contains("<user_request>\n修复登录问题\n</user_request>"));
    }

    /// CLI / ACP 会话：关闭时整体原样（不带标签）；开启时维持「时钟 + 标签」。
    #[test]
    fn with_clock_anchor_returns_body_verbatim_when_clock_off() {
        let body = "修复登录问题";
        assert_eq!(with_clock_anchor(body, false), body);

        let with_clock = with_clock_anchor(body, true);
        assert!(with_clock.contains("【当前真实时钟】"));
        assert!(with_clock.contains("<user_request>\n修复登录问题\n</user_request>"));
    }
}
