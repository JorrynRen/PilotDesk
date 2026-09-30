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
pub mod market;
pub mod process;
