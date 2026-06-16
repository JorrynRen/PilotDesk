pub mod paths;
pub mod errors;
pub mod crypto;

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
