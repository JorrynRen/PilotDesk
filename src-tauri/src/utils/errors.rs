//! 统一错误类型 [`AppError`]。
//!
//! # 错误处理约定（内部 AppError、边界 String、前端契约不变）
//!
//! 1. **内部**：模块内部与辅助函数用 [`AppError`] 表达错误 —— 语义明确（`Db` / `Io` / `Lock` /
//!    `NotFound` / `InvalidInput` / `Config` / `Network` / `External` / `Json`）、可携带
//!    code/details、可用 `?` 传播，不再手写 `format!` 拼错误串。
//! 2. **边界**：`#[tauri::command]` 的**返回类型仍是 `Result<T, String>`**。Tauri 会把 `Err`
//!    序列化后交给前端 `invoke().catch`：`String` 序列化成**字符串**，而 [`AppError`] 会序列化成
//!    **对象** `{code,message,details}`。前端大量 `catch (e) { showToast(`失败: ${e}`) }` 直接做
//!    字符串插值 —— 边界若返回 [`AppError`] 会显示成 `[object Object]`。**故边界一律转成字符串。**
//! 3. **转换**：靠本模块的 `impl From<AppError> for String`（产物即 [`AppError::message`]，
//!    不含错误码前缀）。因此命令体内可以自然地 `?` 一个返回 `AppError` 的辅助函数，或在构造处
//!    写 `Err(AppError::InvalidInput("...".into()).into())`。
//!
//! 结论：**对外（前端可见）的错误形状不变（仍是字符串）**，只把内部实现统一到 [`AppError`]。

use serde::ser::SerializeStruct;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum AppError {
    /// 数据库操作失败
    Db(String),
    /// 文件/IO 操作失败
    Io(String),
    /// 资源锁定失败（如 Mutex 锁、连接池获取）
    Lock(String),
    /// 资源未找到
    NotFound(String),
    /// 输入参数无效
    InvalidInput(String),
    /// 外部服务/进程错误
    External(String),
    /// 配置错误
    Config(String),
    /// 网络请求错误
    Network(String),
    /// JSON 序列化/反序列化错误
    Json(String),
}

impl AppError {
    /// Return the error code string (e.g. "ERR_DB")
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Db(_) => "ERR_DB",
            AppError::Io(_) => "ERR_IO",
            AppError::Lock(_) => "ERR_LOCK",
            AppError::NotFound(_) => "ERR_NOT_FOUND",
            AppError::InvalidInput(_) => "ERR_INVALID_INPUT",
            AppError::External(_) => "ERR_EXTERNAL",
            AppError::Config(_) => "ERR_CONFIG",
            AppError::Network(_) => "ERR_NETWORK",
            AppError::Json(_) => "ERR_JSON",
        }
    }

    /// 错误正文（面向人的文案，**不含**错误码前缀）。
    ///
    /// 错误码是给机器读的：`code()` 与 `Serialize`（`{code, message, details}`）都会带上它，
    /// 前端据此分流。而 `Display` 的产物会直接进入**用户可见文案**——节点执行结果、执行记录
    /// 的 `errorMessage`、命令返回的错误字符串、以及由其拼出的 Toast。所以 Display 只输出正文：
    /// 否则同一次失败会出现两套文案（工作流侧 `[ERR_EXTERNAL] xxx`、会话通知侧 `xxx`）。
    pub fn message(&self) -> &str {
        match self {
            AppError::Db(msg)
            | AppError::Io(msg)
            | AppError::Lock(msg)
            | AppError::NotFound(msg)
            | AppError::InvalidInput(msg)
            | AppError::External(msg)
            | AppError::Config(msg)
            | AppError::Network(msg)
            | AppError::Json(msg) => msg.as_str(),
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (code, message) = match self {
            AppError::Db(_) => ("ERR_DB", "数据库操作失败"),
            AppError::Io(_) => ("ERR_IO", "文件操作失败"),
            AppError::Lock(_) => ("ERR_LOCK", "资源锁定失败"),
            AppError::NotFound(_) => ("ERR_NOT_FOUND", "资源未找到"),
            AppError::InvalidInput(_) => ("ERR_INVALID_INPUT", "输入参数无效"),
            AppError::External(_) => ("ERR_EXTERNAL", "外部服务错误"),
            AppError::Config(_) => ("ERR_CONFIG", "配置错误"),
            AppError::Network(_) => ("ERR_NETWORK", "网络错误"),
            AppError::Json(_) => ("ERR_JSON", "JSON 处理错误"),
        };
        let details: Option<&str> = match self {
            AppError::Db(d)
            | AppError::Io(d)
            | AppError::Lock(d)
            | AppError::NotFound(d)
            | AppError::InvalidInput(d)
            | AppError::External(d)
            | AppError::Config(d)
            | AppError::Network(d)
            | AppError::Json(d) => Some(d.as_str()),
        };
        let mut state = serializer.serialize_struct("AppError", 3)?;
        state.serialize_field("code", code)?;
        state.serialize_field("message", message)?;
        state.serialize_field("details", &details)?;
        state.end()
    }
}

impl std::fmt::Display for AppError {
    /// 只输出正文，见 [`AppError::message`]：错误码经 `code()` / `Serialize` 传递，
    /// 不能混进用户可见文案。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for AppError {}

/// 边界转换：`AppError` → 前端可直接渲染的字符串。
///
/// 见文件头「错误处理约定」：`#[tauri::command]` 的 `Err` 侧保持 `String`，
/// 本实现让命令体内可以 `?` 一个返回 `AppError` 的辅助函数（或 `.into()`）自动落到字符串，
/// 前端仍是 `catch (e) => \`${e}\``，不会退化成 `[object Object]`。
/// 产物等于 [`AppError::message`]（`Display` 正文），**不含** `[ERR_XXX]` 前缀。
impl From<AppError> for String {
    fn from(err: AppError) -> Self {
        err.to_string()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::Db(err.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err.to_string())
    }
}

impl From<r2d2::Error> for AppError {
    fn from(err: r2d2::Error) -> Self {
        AppError::Lock(format!("连接池错误: {}", err))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Json(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;

    /// 回归：`Display` 的产物会直接进入用户可见文案（节点执行结果、执行记录 errorMessage、
    /// 命令返回的错误字符串、Toast），因此**不得**携带 `[ERR_XXX]` 前缀——否则同一次失败
    /// 会在工作流侧与会话通知侧显示成两套文案。错误码仍由 `code()` / `Serialize` 传递。
    #[test]
    fn display_carries_no_error_code_prefix() {
        let err = AppError::External("请求失败: 400 invalid model".to_string());
        assert_eq!(err.to_string(), "请求失败: 400 invalid model");
        assert_eq!(err.message(), "请求失败: 400 invalid model");
        assert_eq!(err.code(), "ERR_EXTERNAL");

        // 序列化仍带 code/message/details 三件套（前端分流用）
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "ERR_EXTERNAL");
        assert_eq!(v["details"], "请求失败: 400 invalid model");
    }
}
