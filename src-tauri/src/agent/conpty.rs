// ──────────────────────────────────────────────
//  Windows ConPTY (Pseudo Console) Spawn
// ──────────────────────────────────────────────
// 在 Windows 上，部分 CLI 工具（如 hermes + prompt_toolkit）要求 stdout 是真实
// 控制台句柄，通过 Stdio::piped() 管道化会导致
//   NoConsoleScreenBufferError: No Windows console found
// ConPTY 为子进程提供虚拟控制台环境，同时允许宿主通过管道读取合并的输出。

use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;

use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_INFORMATION,
    STARTUPINFOEXW,
    InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute,
    DeleteProcThreadAttributeList,
};
use windows_sys::Win32::System::Console::{
    CreatePseudoConsole,
    HPCON,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::Foundation::{
    CloseHandle,
    HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    SECURITY_ATTRIBUTES,
};

// PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = 0x20016 (22)
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x20016;

/// 通过 ConPTY 创建子进程，返回 (ConptyProcess, ChildStdout, ChildStderr)
///
/// - `cmdline`: 完整命令行字符串（如 "hermes chat --query=hello -Q"）
/// - `cwd`: 工作目录
///
/// ConPTY 合并了 stdout 和 stderr 到同一个输出管道。
/// 返回的 ChildStderr 是一个空管道（ConPTY 模式下不使用）。
pub fn spawn_with_conpty(
    cmdline: &str,
    cwd: &str,
) -> Result<(ConptyProcess, tokio::fs::File, tokio::fs::File, bool), String> {
    log::info!("[ConPTY] spawn_with_conpty: cmdline={} cwd={}", cmdline, cwd);
    unsafe {
        // 1. 创建 ConPTY 通信管道
        // pipe_server: ConPTY 写入端（子进程的 console 输出）
        // pipe_client: 宿主读取端（我们从中读取子进程输出）
        let mut h_pipe_server_out: HANDLE = INVALID_HANDLE_VALUE;
        let mut h_pipe_client_out: HANDLE = INVALID_HANDLE_VALUE;
        let mut h_pipe_server_in: HANDLE = INVALID_HANDLE_VALUE;
        let mut h_pipe_client_in: HANDLE = INVALID_HANDLE_VALUE;

        let sa = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1, // TRUE — 子进程继承句柄
        };

        // 输出管道：ConPTY → 宿主
        // CreatePipe(hRead, hWrite): h_pipe_client_out=read(宿主读), h_pipe_server_out=write(ConPTY写)
        if CreatePipe(
            &mut h_pipe_client_out,
            &mut h_pipe_server_out,
            &sa,
            0,
        ) == 0 {
            return Err("CreatePipe(output) 失败".into());
        }

        // 输入管道：宿主 → ConPTY（提供 stdin 数据）
        if CreatePipe(
            &mut h_pipe_server_in,
            &mut h_pipe_client_in,
            &sa,
            0,
        ) == 0 {
            CloseHandle(h_pipe_server_out);
            CloseHandle(h_pipe_client_out);
            return Err("CreatePipe(input) 失败".into());
        }

        // 2. 创建伪控制台
        let mut h_pc: HPCON = 0;
        use windows_sys::Win32::System::Console::COORD;
        let coord = COORD { X: 80, Y: 30 };
        let result = CreatePseudoConsole(
            coord,
            h_pipe_server_in,   // ConPTY 读 stdin 从这个管道
            h_pipe_server_out,  // ConPTY 写 stdout/stderr 到这个管道
            0,    // flags
            &mut h_pc,
        );

        if result != 0 {
            CloseHandle(h_pipe_server_in);
            CloseHandle(h_pipe_client_in);
            CloseHandle(h_pipe_server_out);
            CloseHandle(h_pipe_client_out);
            return Err(format!("CreatePseudoConsole 失败: HRESULT 0x{:X}", result as u32));
        }

        // 3. 准备 STARTUPINFOEXW（含 ConPTY 属性）
        // 计算属性列表所需大小
        let mut attr_list_size: usize = 0;
        InitializeProcThreadAttributeList(
            std::ptr::null_mut(),
            1,
            0,
            &mut attr_list_size,
        );

        // 分配属性列表
        let mut startup_info_ex: STARTUPINFOEXW = std::mem::zeroed();
        startup_info_ex.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        startup_info_ex.lpAttributeList = allocate_attr_list(attr_list_size);

        if startup_info_ex.lpAttributeList.is_null() {
            CloseHandle(h_pipe_server_in);
            CloseHandle(h_pipe_client_in);
            CloseHandle(h_pipe_server_out);
            CloseHandle(h_pipe_client_out);
            // ConPTY handle cleanup - use raw value since we don't have proper destructor
            // The ConPTY is cleaned up when all handles are closed
            return Err("分配属性列表内存失败".into());
        }

        if InitializeProcThreadAttributeList(
            startup_info_ex.lpAttributeList,
            1,
            0,
            &mut attr_list_size,
        ) == 0 {
            free_attr_list(startup_info_ex.lpAttributeList);
            CloseHandle(h_pipe_server_in);
            CloseHandle(h_pipe_client_in);
            CloseHandle(h_pipe_server_out);
            CloseHandle(h_pipe_client_out);
            return Err("InitializeProcThreadAttributeList 失败".into());
        }

        // 关联 ConPTY 到属性列表
        if UpdateProcThreadAttribute(
            startup_info_ex.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            h_pc as *const _,
            mem::size_of::<HPCON>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0 {
            DeleteProcThreadAttributeList(startup_info_ex.lpAttributeList);
            free_attr_list(startup_info_ex.lpAttributeList);
            CloseHandle(h_pipe_server_in);
            CloseHandle(h_pipe_client_in);
            CloseHandle(h_pipe_server_out);
            CloseHandle(h_pipe_client_out);
            return Err("UpdateProcThreadAttribute 失败".into());
        }

        // 4. 构建命令行和环境
        let cmdline_w: Vec<u16> = encode_wide(cmdline);
        let mut cmdline_mut = cmdline_w.clone();
        // cmdline_w 需要 null-terminated 并且可修改（CreateProcessW 要求）
        cmdline_mut.push(0);

        let cwd_w = encode_wide(cwd);
        let mut cwd_buf: Vec<u16> = cwd_w.clone();
        cwd_buf.push(0);

        let mut proc_info: PROCESS_INFORMATION = std::mem::zeroed();

        // 直接传递给 CreateProcessW，不使用 cmd /C
        // 避免 cmd.exe 将参数中的换行符解释为命令分隔符
        let exe_cmdline = encode_wide(&cmdline);
        let mut cmd_buf = exe_cmdline;
        cmd_buf.push(0);

        let creation_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;

        let result = windows_sys::Win32::System::Threading::CreateProcessW(
            cmd_buf.as_ptr(),  // lpApplicationName: 直接使用可执行文件路径
            cmd_buf.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0, // 不继承句柄（ConPTY 管道已通过属性传递）
            creation_flags,
            std::ptr::null(), // 使用父进程环境
            cwd_buf.as_ptr(),
            &startup_info_ex.StartupInfo,
            &mut proc_info,
        );

        // 5. 清理 ConPTY 相关资源
        DeleteProcThreadAttributeList(startup_info_ex.lpAttributeList);
        free_attr_list(startup_info_ex.lpAttributeList);

        // 关闭 ConPTY 输入管道的服务端（不再需要，ConPTY 已连接）
        CloseHandle(h_pipe_server_in);
        // 关闭 ConPTY 输出管道的服务端（ConPTY 已连接，不需要服务端句柄）
        CloseHandle(h_pipe_server_out);

        log::info!("[ConPTY] CreateProcessW result={}, pid={}", result, proc_info.dwProcessId);
        if result == 0 {
            let err_code = windows_sys::Win32::Foundation::GetLastError();
            CloseHandle(h_pipe_client_in);
            CloseHandle(h_pipe_client_out);
            return Err(format!("CreateProcessW 失败: 系统错误 {}", err_code));
        }

        // 关闭主线程句柄（不需要）
        CloseHandle(proc_info.hThread);

        // 6. 将 stdout 管道 HANDLE 转换为 tokio::fs::File（实现 AsyncRead）
        let stdout = tokio::fs::File::from_std(
            std::fs::File::from_raw_handle(h_pipe_client_out)
        );

        // stderr: ConPTY 合并了 stdout 和 stderr 到同一个管道，
        // 此处返回一个空管道（已关闭写端的读端），读取立即返回 EOF
        let mut h_null_read: HANDLE = INVALID_HANDLE_VALUE;
        let mut h_null_write: HANDLE = INVALID_HANDLE_VALUE;
        let null_sa = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 0,
        };
        CreatePipe(&mut h_null_read, &mut h_null_write, &null_sa, 0);
        CloseHandle(h_null_write);
        let stderr = tokio::fs::File::from_std(
            std::fs::File::from_raw_handle(h_null_read)
        );

        // 用 ConptyProcess 封装进程句柄
        // stdin_pipe 必须在进程生命周期内保持打开，否则 ConPTY 会发送 Ctrl+C 终止子进程
        let child = ConptyProcess {
            handle: SendHandle(proc_info.hProcess),
            pid: proc_info.dwProcessId,
            stdin_pipe: SendHandle(h_pipe_client_in),
        };

        log::info!("[ConPTY] process started successfully, pid={}", proc_info.dwProcessId);
        Ok((child, stdout, stderr, true)) // true = conpty_mode
    }
}

fn encode_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().collect()
}

/// 分配属性列表内存
unsafe fn allocate_attr_list(size: usize) -> *mut std::ffi::c_void {
    windows_sys::Win32::System::Memory::HeapAlloc(
        windows_sys::Win32::System::Memory::GetProcessHeap(),
        0, // HEAP_ZERO_MEMORY
        size,
    )
}

/// HANDLE 不自动实现 Send，需要 newtype 包装以在 spawn_blocking 中跨线程传递
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    fn wait_for_exit(self) -> i32 {
        unsafe {
            use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE, GetExitCodeProcess};
            WaitForSingleObject(self.0, INFINITE);
            let mut exit_code: u32 = 0;
            if GetExitCodeProcess(self.0, &mut exit_code) == 0 {
                -1
            } else {
                exit_code as i32
            }
        }
    }
}

/// ConPTY 模式下的进程封装
/// 由于 std::process::Child 无法从原始 HANDLE 构造，
/// 使用 Windows API 直接管理进程生命周期。
pub struct ConptyProcess {
    handle: SendHandle,
    pid: u32,
    /// stdin 写入端，必须在进程生命周期内保持打开
    /// ConPTY 在 stdin 关闭时会向子进程发送 Ctrl+C
    stdin_pipe: SendHandle,
}

impl ConptyProcess {
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// 等待进程退出，返回退出码
    pub async fn wait(&self) -> Result<i32, String> {
        let send_handle = SendHandle(self.handle.0);
        tokio::task::spawn_blocking(move || send_handle.wait_for_exit())
            .await.map_err(|e| format!("等待进程失败: {}", e))
    }

    /// 通过 taskkill 终止进程
    pub fn kill(&self) {
        let _ = std::process::Command::new("taskkill")
            .args(&["/PID", &self.pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

impl Drop for ConptyProcess {
    fn drop(&mut self) {
        unsafe {
            // 先关闭 stdin 管道（进程已结束，安全关闭）
            CloseHandle(self.stdin_pipe.0);
            // 再关闭进程句柄
            CloseHandle(self.handle.0);
        }
    }
}

/// 释放属性列表内存
unsafe fn free_attr_list(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        windows_sys::Win32::System::Memory::HeapFree(
            windows_sys::Win32::System::Memory::GetProcessHeap(),
            0,
            ptr,
        );
    }
}
