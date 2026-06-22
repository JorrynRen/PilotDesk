use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

use super::PluginHost;

/// 文件条目
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// 文件状态
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct FileStat {
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub created: String,
    pub modified: String,
}

/// 检查路径是否在插件目录内（安全限制）
fn is_path_in_plugin_dir(plugin_path: &str, target_path: &str) -> bool {
    let plugin_dir = PathBuf::from(plugin_path).canonicalize().ok();
    let target = PathBuf::from(target_path).canonicalize().ok();
    
    match (plugin_dir, target) {
        (Some(pd), Some(t)) => t.starts_with(&pd),
        _ => false,
    }
}

// ── Tauri Commands ──

#[tauri::command]
pub fn plugin_fs_read_text(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<String, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();
    
    // 沙箱启用时拒绝
    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，文件系统读取被拒绝".to_string());
    }
    
    // 查找插件路径
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    let full_path = PathBuf::from(&plugin.path).join(&path);
    
    // 路径遍历检查
    if !is_path_in_plugin_dir(&plugin.path, &full_path.to_string_lossy()) {
        return Err("路径不在插件目录内，已拒绝访问".to_string());
    }
    
    std::fs::read_to_string(&full_path)
        .map_err(|e| format!("读取文件失败: {}", e))
}

#[tauri::command]
pub fn plugin_fs_write_text(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();
    
    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，文件系统写入被拒绝".to_string());
    }
    
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    let full_path = PathBuf::from(&plugin.path).join(&path);
    
    if !is_path_in_plugin_dir(&plugin.path, &full_path.to_string_lossy()) {
        return Err("路径不在插件目录内，已拒绝访问".to_string());
    }
    
    // 创建父目录
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }
    
    std::fs::write(&full_path, &content)
        .map_err(|e| format!("写入文件失败: {}", e))
}

#[tauri::command]
pub fn plugin_fs_delete(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<(), String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();
    
    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，文件系统删除被拒绝".to_string());
    }
    
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    let full_path = PathBuf::from(&plugin.path).join(&path);
    
    if !is_path_in_plugin_dir(&plugin.path, &full_path.to_string_lossy()) {
        return Err("路径不在插件目录内，已拒绝访问".to_string());
    }
    
    if full_path.is_dir() {
        std::fs::remove_dir_all(&full_path)
            .map_err(|e| format!("删除目录失败: {}", e))
    } else {
        std::fs::remove_file(&full_path)
            .map_err(|e| format!("删除文件失败: {}", e))
    }
}

#[tauri::command]
pub fn plugin_fs_exists(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<bool, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    let full_path = PathBuf::from(&plugin.path).join(&path);
    Ok(full_path.exists())
}

#[tauri::command]
pub fn plugin_fs_read_dir(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<Vec<FileEntry>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();
    
    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，文件系统读取被拒绝".to_string());
    }
    
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    let full_path = PathBuf::from(&plugin.path).join(&path);
    
    if !is_path_in_plugin_dir(&plugin.path, &full_path.to_string_lossy()) {
        return Err("路径不在插件目录内，已拒绝访问".to_string());
    }
    
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&full_path).map_err(|e| format!("读取目录失败: {}", e))? {
        let entry = entry.map_err(|e| format!("读取条目失败: {}", e))?;
        let metadata = entry.metadata().map_err(|e| format!("读取元数据失败: {}", e))?;
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
        });
    }
    
    Ok(entries)
}
