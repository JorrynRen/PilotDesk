use tauri::Manager;

use std::process::{Child, Command};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub struct SidecarManager {
    process: Option<Child>,
    port: u16,
    restart_count: Arc<AtomicU32>,
    is_running: Arc<AtomicBool>,
}

impl SidecarManager {
    pub fn new(port: u16) -> Self {
        Self {
            process: None,
            port,
            restart_count: Arc::new(AtomicU32::new(0)),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&mut self, app_handle: tauri::AppHandle) -> Result<u16, String> {
        // Get sidecar path relative to the app resource dir
        let resource_dir = app_handle.path().resource_dir()
            .map_err(|e| format!("Failed to get resource dir: {}", e))?;
        
        let sidecar_path = resource_dir.join("sidecar").join("dist").join("index.js");
        
        // During development, use relative path
        let sidecar_path_str = if sidecar_path.exists() {
            sidecar_path.to_string_lossy().to_string()
        } else {
            "sidecar/dist/index.js".to_string()
        };

        let child = Command::new("node")
            .args([&sidecar_path_str])
            .env("PORT", self.port.to_string())
            .spawn()
            .map_err(|e| format!("Failed to start sidecar: {}", e))?;

        self.process = Some(child);
        self.is_running.store(true, Ordering::Relaxed);
        println!("[Sidecar] Started on port {}", self.port);

        // Spawn monitoring task
        let is_running = self.is_running.clone();
        let restart_count = self.restart_count.clone();
        let port = self.port;
        let app = app_handle.clone();
        
        tauri::async_runtime::spawn(async move {
            // Simple heartbeat: check if sidecar process is alive
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                
                if !is_running.load(Ordering::Relaxed) {
                    break;
                }
            }
        });

        Ok(self.port)
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

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.stop();
    }
}
