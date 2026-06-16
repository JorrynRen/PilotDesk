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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), match self {
            AppError::Db(msg)
            | AppError::Io(msg)
            | AppError::Lock(msg)
            | AppError::NotFound(msg)
            | AppError::InvalidInput(msg)
            | AppError::External(msg)
            | AppError::Config(msg)
            | AppError::Network(msg)
            | AppError::Json(msg) => msg,
        })
    }
}

impl std::error::Error for AppError {}

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
