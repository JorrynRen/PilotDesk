use serde::{Deserialize, Serialize};

/// 控制台类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConsoleType {
    Windows,
    MacOS,
    Linux,
}

/// 控制台配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleConfig {
    /// 控制台类型
    pub console_type: Option<ConsoleType>,
    /// 缓冲区大小
    pub buffer_size: usize,
    /// 超时时间（毫秒）
    pub timeout_ms: u64,
    /// 是否启用调试模式
    pub enable_debug: bool,
    /// 是否启用事件驱动模式
    pub event_driven: bool,
    /// 输出编码
    pub output_encoding: String,
    /// 是否合并stdout和stderr
    pub merge_output: bool,
    /// 最大输出行数
    pub max_lines: Option<usize>,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            console_type: None,
            buffer_size: 1024 * 1024, // 1MB
            timeout_ms: 30000, // 30秒
            enable_debug: false,
            event_driven: true,
            output_encoding: "utf-8".to_string(),
            merge_output: false,
            max_lines: Some(10000),
        }
    }
}

/// 控制台构建器
pub struct ConsoleBuilder {
    config: ConsoleConfig,
}

impl ConsoleBuilder {
    pub fn new() -> Self {
        Self {
            config: ConsoleConfig::default(),
        }
    }
    
    pub fn console_type(mut self, console_type: ConsoleType) -> Self {
        self.config.console_type = Some(console_type);
        self
    }
    
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }
    
    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.config.timeout_ms = timeout;
        self
    }
    
    pub fn enable_debug(mut self, enable: bool) -> Self {
        self.config.enable_debug = enable;
        self
    }
    
    pub fn event_driven(mut self, enable: bool) -> Self {
        self.config.event_driven = enable;
        self
    }
    
    pub fn build(self) -> ConsoleConfig {
        self.config
    }
}

/// 操作系统检测
pub fn detect_os_type() -> ConsoleType {
    #[cfg(target_os = "windows")]
    return ConsoleType::Windows;
    
    #[cfg(target_os = "macos")]
    return ConsoleType::MacOS;
    
    #[cfg(target_os = "linux")]
    return ConsoleType::Linux;
    
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    panic!("Unsupported operating system");
}