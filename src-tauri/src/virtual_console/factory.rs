//! 虚拟控制台工厂模块
//!
//! 提供工厂模式创建跨平台虚拟控制台实例，
//! 根据 ConsoleType 自动选择对应平台实现。

use std::io;
use crate::virtual_console::traits::VirtualConsole;
use crate::virtual_console::config::{ConsoleType, ConsoleConfig, detect_os_type};

#[cfg(target_os = "windows")]
use crate::virtual_console::windows::WindowsConpty;

#[cfg(target_os = "macos")]
use crate::virtual_console::macos::MacVirtualConsole;

#[cfg(target_os = "linux")]
use crate::virtual_console::linux::LinuxVirtualConsole;

/// 虚拟控制台工厂
///
/// 根据配置或自动检测的操作系统类型，创建对应平台的虚拟控制台实例。
pub struct ConsoleFactory;

impl ConsoleFactory {
    /// 自动检测操作系统并创建对应的虚拟控制台
    pub fn auto_create() -> io::Result<Box<dyn VirtualConsole>> {
        let os_type = detect_os_type();
        Self::create(os_type)
    }

    /// 自动检测操作系统并创建对应的虚拟控制台（带配置，当前仅做日志）
    pub fn auto_create_with_config(_config: &ConsoleConfig) -> io::Result<Box<dyn VirtualConsole>> {
        let os_type = _config.console_type.unwrap_or_else(detect_os_type);
        Self::create(os_type)
    }

    /// 根据指定的控制台类型创建虚拟控制台
    pub fn create(console_type: ConsoleType) -> io::Result<Box<dyn VirtualConsole>> {
        match console_type {
            #[cfg(target_os = "windows")]
            ConsoleType::Windows => {
                let console = WindowsConpty::new()?;
                Ok(Box::new(console))
            }

            #[cfg(target_os = "macos")]
            ConsoleType::MacOS => {
                let console = MacVirtualConsole::new()?;
                Ok(Box::new(console))
            }

            #[cfg(target_os = "linux")]
            ConsoleType::Linux => {
                let console = LinuxVirtualConsole::new()?;
                Ok(Box::new(console))
            }

            #[cfg(not(target_os = "windows"))]
            ConsoleType::Windows => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "Windows ConPTY not available on this platform"))
            }

            #[cfg(not(target_os = "macos"))]
            ConsoleType::MacOS => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "macOS PTY not available on this platform"))
            }

            #[cfg(not(target_os = "linux"))]
            ConsoleType::Linux => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "Linux PTY not available on this platform"))
            }
        }
    }

    /// 获取当前平台支持的控制系统列表
    pub fn supported_types() -> Vec<ConsoleType> {
        let mut types = Vec::new();
        #[cfg(target_os = "windows")]
        types.push(ConsoleType::Windows);
        #[cfg(target_os = "macos")]
        types.push(ConsoleType::MacOS);
        #[cfg(target_os = "linux")]
        types.push(ConsoleType::Linux);
        types
    }
}
