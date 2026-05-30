use std::process::Child;

pub struct SidecarManager {
    node_process: Option<Child>,
}

impl SidecarManager {
    pub fn new() -> Self {
        Self { node_process: None }
    }
    
    pub fn start_sidecar(&mut self) -> Result<(), String> {
        // Will be implemented in Task 5
        println!("SidecarManager: start_sidecar (placeholder)");
        Ok(())
    }
    
    pub fn stop_sidecar(&mut self) {
        if let Some(mut child) = self.node_process.take() {
            let _ = child.kill();
            println!("Sidecar stopped.");
        }
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.stop_sidecar();
    }
}
