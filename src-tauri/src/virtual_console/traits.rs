pub use std::io::Result;
pub use std::process::ExitStatus;
pub use std::time::Duration;
/// 虚拟控制台统一接口
pub trait VirtualConsole: Send + Sync {
    /// 启动进程
    fn spawn(&mut self, command: &str, args: &[&str], cwd: &str) -> Result<ConsoleHandle>;
    
    /// 向控制台输入数据
    fn write(&mut self, data: &[u8]) -> Result<()>;
    
    /// 读取控制台输出
    fn read(&mut self) -> Result<Vec<u8>>;
    
    /// 读取一行输出
    fn read_line(&mut self) -> Result<String>;
    
    /// 发送终止信号
    fn terminate(&mut self) -> Result<()>;
    
    /// 强制终止进程
    fn kill(&mut self) -> Result<()>;
    
    /// 等待进程结束
    fn wait(&mut self, timeout: Option<Duration>) -> Result<ExitStatus>;
    
    /// 获取进程ID
    fn pid(&self) -> u32;
    
    /// 检查进程是否仍在运行
    fn is_running(&self) -> bool;
    
    /// 获取进程状态
    fn status(&self) -> ProcessStatus;
    
    /// 关闭控制台资源
    fn close(&mut self) -> Result<()>;
}

/// 控制台句柄
pub struct ConsoleHandle {
    pub pid: u32,
    pub stdin: Box<dyn ConsoleWriter>,
    pub stdout: Box<dyn ConsoleReader>,
    pub stderr: Box<dyn ConsoleReader>,
    pub is_conpty: bool, // 是否为ConPTY模式
}

/// 进程状态
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    /// 正在运行
    Running,
    /// 已正常退出
    Exited(ExitStatus),
    /// 被信号终止
    Terminated(i32),
    /// 未知状态
    Unknown,
}

/// 输入输出trait
pub trait ConsoleWriter: Send + Sync {
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

pub trait ConsoleReader: Send + Sync {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;
    fn read_line(&mut self) -> Result<String>;
    fn has_data(&self) -> bool;
    fn close(&mut self) -> Result<()>;
}

/// 输出流类型
#[derive(Debug, Clone, Copy)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// 控制台事件
#[derive(Debug, Clone)]
pub enum ConsoleEvent {
    /// 输出数据
    Output { data: Vec<u8>, stream: OutputStream },
    /// 进程状态变化
    Status { status: ProcessStatus },
    /// 错误事件
    Error { error: String },
    /// 会话结束
    SessionEnded,
}

/// 事件监听器trait
pub trait ConsoleEventListener: Send + Sync {
    fn on_event(&mut self, event: ConsoleEvent) -> std::io::Result<()>;
}

/// 事件驱动的控制台接口
pub trait EventDrivenConsole: Send + Sync {
    fn add_listener(&mut self, listener: Box<dyn ConsoleEventListener>) -> Result<()>;
    fn remove_listener(&mut self, listener_id: usize) -> Result<()>;
    fn start_event_loop(&mut self) -> Result<()>;
    fn stop_event_loop(&mut self) -> Result<()>;
}

/// 异步控制台接口
pub trait AsyncVirtualConsole: Send + Sync {
    /// 异步启动进程
    async fn spawn(&mut self, command: &str, args: &[&str], cwd: &str) -> Result<ConsoleHandle>;
    
    /// 异步写入数据
    async fn write(&mut self, data: &[u8]) -> Result<()>;
    
    /// 异步读取数据
    async fn read(&mut self) -> Result<Vec<u8>>;
    
    /// 异步读取一行
    async fn read_line(&mut self) -> Result<String>;
    
    /// 异步终止进程
    async fn terminate(&mut self) -> Result<()>;
    
    /// 异步等待进程结束
    async fn wait(&mut self, timeout: Option<Duration>) -> Result<ExitStatus>;
}