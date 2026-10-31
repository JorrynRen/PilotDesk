//! 应用运行时的本机状态查询（与「Agent 环境探测」无关，故独立成模块）。
//!
//! 目前只有一项：进程是否以管理员/高完整性级别运行。
//! Windows 的 UIPI 会拦掉「中完整性进程 → 高完整性进程」的 OLE 拖放，
//! 以管理员权限启动后「把文件从资源管理器拖进窗口」必然失效且没有任何报错，
//! 前端据此在启动时给出提示，避免用户以为是功能坏了。

/// 进程是否以管理员权限（高完整性级别）运行；非 Windows 恒为 `false`。
#[tauri::command]
pub fn is_process_elevated() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_is_elevated()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 查询当前进程令牌的 `TokenElevation`。
///
/// 不用「能否写系统目录」之类的探测：那种写法会被 UAC 虚拟化与目录 ACL 误导，
/// 直接读令牌是最准确的。查询失败一律按「未提权」处理 —— 提示宁可少弹。
#[cfg(target_os = "windows")]
fn windows_is_elevated() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: 只查询当前进程自己的令牌；token 句柄在同一作用域内关闭。
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}
