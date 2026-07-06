//! 虚拟控制台事件系统

use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;

use crate::virtual_console::traits::{ConsoleEvent, ConsoleEventListener, OutputStream};

/// 事件系统
pub struct EventSystem {
    event_sender: mpsc::Sender<ConsoleEvent>,
    listeners: Arc<Mutex<Vec<Box<dyn ConsoleEventListener>>>>,
    is_running: Arc<Mutex<bool>>,
}

impl EventSystem {
    pub fn new() -> Self {
        let (event_sender, _event_receiver) = mpsc::channel();
        let listeners = Arc::new(Mutex::new(Vec::new()));
        let is_running = Arc::new(Mutex::new(false));

        Self {
            event_sender,
            listeners,
            is_running,
        }
    }

    /// 添加事件监听器
    pub fn add_listener(&mut self, listener: Box<dyn ConsoleEventListener>) -> Result<(), String> {
        self.listeners.lock().unwrap().push(listener);
        Ok(())
    }

    /// 移除事件监听器
    pub fn remove_listener(&mut self, _listener_id: usize) -> Result<(), String> {
        // 简化实现：当前不维护 listener_id 索引
        Ok(())
    }

    /// 发送事件到所有监听器
    pub fn emit(&self, event: ConsoleEvent) {
        let mut listeners = self.listeners.lock().unwrap();
        for listener in listeners.iter_mut() {
            let _ = listener.on_event(event.clone());
        }
    }

    /// 启动事件系统
    pub fn start(&mut self) -> Result<(), String> {
        *self.is_running.lock().unwrap() = true;
        Ok(())
    }

    /// 停止事件系统
    pub fn stop(&mut self) -> Result<(), String> {
        *self.is_running.lock().unwrap() = false;
        Ok(())
    }

    /// 检查是否运行中
    pub fn is_running(&self) -> bool {
        *self.is_running.lock().unwrap()
    }
}

impl Default for EventSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// 日志事件监听器
pub struct LogEventListener;

impl LogEventListener {
    pub fn new() -> Self {
        Self
    }
}

impl ConsoleEventListener for LogEventListener {
    fn on_event(&mut self, event: ConsoleEvent) -> std::io::Result<()> {
        match event {
            ConsoleEvent::Output { data, stream } => {
                let text = String::from_utf8_lossy(&data);
                match stream {
                    OutputStream::Stdout => log::debug!("[Console/Event] stdout: {}", text.trim()),
                    OutputStream::Stderr => log::warn!("[Console/Event] stderr: {}", text.trim()),
                }
            }
            ConsoleEvent::Status { status } => {
                log::info!("[Console/Event] status: {:?}", status);
            }
            ConsoleEvent::Error { error } => {
                log::error!("[Console/Event] error: {}", error);
            }
            ConsoleEvent::SessionEnded => {
                log::info!("[Console/Event] session ended");
            }
        }
        Ok(())
    }
}

/// 统计事件监听器
pub struct StatsEventListener {
    pub event_count: usize,
}

impl StatsEventListener {
    pub fn new() -> Self {
        Self { event_count: 0 }
    }
}

impl ConsoleEventListener for StatsEventListener {
    fn on_event(&mut self, _event: ConsoleEvent) -> std::io::Result<()> {
        self.event_count += 1;
        Ok(())
    }
}
