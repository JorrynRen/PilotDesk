use tauri::Manager;

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::net::TcpStream;

type StderrBuf = Arc<Mutex<String>>;

pub struct SidecarManager {
    process: Option<Child>,
    port: u16,
    #[allow(dead_code)]
    restart_count: Arc<AtomicU32>,
    is_running: Arc<AtomicBool>,
    #[allow(dead_code)]
    stderr_buf: StderrBuf,
}

impl SidecarManager {
    /// Resolve sidecar dist/index.js path using multi-strategy probing.
    /// Priority:
    ///   1. CARGO_MANIFEST_DIR/sidecar/dist/index.js (dev: target inside src-tauri)
    ///   2. exe's 2nd parent (target/debug or target/release) → project root → sidecar/
    ///   3. Tauri resource_dir (production bundle)
    fn resolve_sidecar_path(app_handle: &tauri::AppHandle) -> String {
        // Strategy 1: CARGO_MANIFEST_DIR (compile-time, always correct in dev)
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let p = std::path::Path::new(&manifest)
                .join("..")  // src-tauri → project root
                .join("sidecar")
                .join("dist")
                .join("index.js");
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }

        // Strategy 2: exe directory → navigate to project root → sidecar/
        // exe is in src-tauri/target/debug/ → need 3 parents to reach project root
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                // Try going up 3 levels: target/debug/ → target/ → src-tauri/ → project_root
                let mut dir = exe_dir;
                for _ in 0..4 {
                    if let Some(parent) = dir.parent() {
                        dir = parent;
                        let p = dir.join("sidecar").join("dist").join("index.js");
                        if p.exists() {
                            println!("[Sidecar] Found via exe traversal: {}", p.display());
                            return p.to_string_lossy().to_string();
                        }
                    }
                }
            }
        }

        // Strategy 3: CWD-based (works when launched from project root)
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("sidecar").join("dist").join("index.js");
            if p.exists() {
                println!("[Sidecar] Found via CWD: {}", p.display());
                return p.to_string_lossy().to_string();
            }
        }

        // Strategy 4: Tauri resource_dir (production bundle)
        if let Ok(resource_dir) = app_handle.path().resource_dir() {
            let p = resource_dir.join("sidecar").join("dist").join("index.js");
            if p.exists() {
                println!("[Sidecar] Found via resource_dir: {}", p.display());
                return p.to_string_lossy().to_string();
            }
        }

        // Fallback
        "../sidecar/dist/index.js".to_string()
    }

    pub fn new(port: u16) -> Self {
        Self {
            process: None,
            port,
            restart_count: Arc::new(AtomicU32::new(0)),
            is_running: Arc::new(AtomicBool::new(false)),
            stderr_buf: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn start(&mut self, app_handle: tauri::AppHandle) -> Result<u16, String> {
        let sidecar_path_str = Self::resolve_sidecar_path(&app_handle);
        println!("[Sidecar] Resolved sidecar path: {}", sidecar_path_str);
        let sidecar_file = std::path::Path::new(&sidecar_path_str);
        println!("[Sidecar] File exists: {}", sidecar_file.exists());
        if !sidecar_file.exists() {
            let err_msg = format!("[Sidecar] FATAL: sidecar not found at '{}'\n  CWD: {:?}\n  EXE: {:?}",
                sidecar_path_str,
                std::env::current_dir().ok(),
                std::env::current_exe().ok());
            println!("{}", err_msg);
            eprintln!("{}", err_msg);
            return Err(err_msg);
        }

        #[cfg(target_os = "windows")]
        use std::os::windows::process::CommandExt;

        #[cfg(target_os = "windows")]
        let node_cmd = "F:\\soft\\nodejs\\node.exe";

        #[cfg(not(target_os = "windows"))]
        let node_cmd = "node";

        #[cfg(target_os = "windows")]
        let child = Command::new(node_cmd)
            .args([&sidecar_path_str])
            .env("PORT", self.port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x00000200)
            .spawn()
            .map_err(|e| {
                let msg = format!("Failed to start sidecar (cmd={}, path={}): {}", node_cmd, sidecar_path_str, e);
                println!("[Sidecar] {}", msg);
                eprintln!("[Sidecar] {}", msg);
                msg
            })?;

        #[cfg(not(target_os = "windows"))]
        let child = Command::new(node_cmd)
            .args([&sidecar_path_str])
            .env("PORT", self.port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start sidecar: {}", e))?;

        self.process = Some(child);
        self.is_running.store(true, Ordering::Relaxed);
        println!("[Sidecar] Spawned {}, waiting for port {}...", node_cmd, self.port);

        // Wait for port
        let port_ready = self.wait_for_port(10);
        if !port_ready {
            let port = self.port;
            let stderr_buf = self.stderr_buf.clone();
            let _app = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                for i in 0..20u32 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                        println!(
                            "[Sidecar] Port {} ready (async, +{}ms)",
                            port,
                            2000 + i * 500
                        );
                        return;
                    }
                }
                let buf = stderr_buf.lock().unwrap();
                println!("[Sidecar] Port {} unavailable", port);
                if !buf.is_empty() {
                    println!("[Sidecar] stderr: {}", buf.trim());
                }
            });
        } else {
            println!("[Sidecar] Port {} ready", self.port);
        }

        // Start background watchdog to detect unexpected exits and auto-restart
        // Spawn BEFORE spawning watchdog (watchdog will own the child handle)
        self.spawn_watchdog(app_handle);

        Ok(self.port)
    }

    /// Static version of resolve_sidecar_path for use in watchdog thread.
    fn resolve_sidecar_path_static(app_handle: &tauri::AppHandle) -> String {
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let p = std::path::Path::new(&manifest)
                .join("..").join("sidecar").join("dist").join("index.js");
            if p.exists() { return p.to_string_lossy().to_string(); }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let mut dir = exe_dir;
                for _ in 0..4 {
                    if let Some(parent) = dir.parent() {
                        dir = parent;
                        let p = dir.join("sidecar").join("dist").join("index.js");
                        if p.exists() { return p.to_string_lossy().to_string(); }
                    }
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("sidecar").join("dist").join("index.js");
            if p.exists() { return p.to_string_lossy().to_string(); }
        }
        if let Ok(resource_dir) = app_handle.path().resource_dir() {
            let p = resource_dir.join("sidecar").join("dist").join("index.js");
            if p.exists() { return p.to_string_lossy().to_string(); }
        }
        "../sidecar/dist/index.js".to_string()
    }

    /// Background thread that monitors the sidecar process.
    /// If it exits unexpectedly, attempts auto-restart (up to 5 times).
    fn spawn_watchdog(&mut self, app_handle: tauri::AppHandle) {
        let is_running = self.is_running.clone();
        let port = self.port;
        let restart_count = self.restart_count.clone();

        // Take the process out — the watchdog owns the child handle.
        // This must happen AFTER wait_for_port and stderr capture are done.
        let mut child = match self.process.take() {
            Some(c) => c,
            None => return,
        };

        std::thread::spawn(move || {
            // Wait for the process to exit
            let status = child.wait();
            match &status {
                Ok(s) => println!("[Sidecar] Process exited with code: {:?}", s.code()),
                Err(e) => println!("[Sidecar] Process wait error: {}", e),
            }
            is_running.store(false, Ordering::Relaxed);

            let count = restart_count.fetch_add(1, Ordering::Relaxed);
            if count >= 5 {
                println!("[Sidecar] Max restart attempts (5) reached, giving up");
                return;
            }

            println!("[Sidecar] Auto-restarting (attempt {})...", count + 1);
            std::thread::sleep(Duration::from_secs(2));

            #[cfg(target_os = "windows")]
            use std::os::windows::process::CommandExt;

            let sidecar_path = Self::resolve_sidecar_path_static(&app_handle);
            let mut child = match Command::new("F:\\soft\\nodejs\\node.exe")
                .args([&sidecar_path])
                .env("PORT", port.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x00000200)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    println!("[Sidecar] Restart spawn failed: {}", e);
                    return;
                }
            };

            // Wait for port
            for i in 0..20u32 {
                std::thread::sleep(Duration::from_millis(500));
                if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                    is_running.store(true, Ordering::Relaxed);
                    println!("[Sidecar] Restarted on port {} (+{}ms)", port, (i + 1) * 500);
                    // Detach: let the restarted process run independently
                    // (Child dropped here would kill it, so we leak it)
                    std::mem::forget(child);
                    return;
                }
            }
            println!("[Sidecar] Restart failed: port {} not ready after 10s", port);
            let _ = child.kill();
        });
    }

    fn wait_for_port(&mut self, max_retries: u32) -> bool {
        for _ in 0..max_retries {
            if TcpStream::connect(format!("127.0.0.1:{}", self.port)).is_ok() {
                return true;
            }
            if let Some(ref mut proc) = self.process {
                if let Ok(Some(status)) = proc.try_wait() {
                    let buf = self.stderr_buf.lock().unwrap();
                    println!(
                        "[Sidecar] Exited code {:?}, port {} not ready",
                        status.code(),
                        self.port
                    );
                    if !buf.is_empty() {
                        println!("[Sidecar] stderr: {}", buf.trim());
                    }
                    drop(buf);
                    self.is_running.store(false, Ordering::Relaxed);
                    return false;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.is_running.store(false, Ordering::Relaxed);
        println!("[Sidecar] Stopped");
    }

    pub fn restart(&mut self, app_handle: tauri::AppHandle) -> Result<u16, String> {
        self.stop();
        std::thread::sleep(Duration::from_secs(1));
        self.restart_count.store(0, Ordering::Relaxed);
        self.start(app_handle)
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.stop();
    }
}
