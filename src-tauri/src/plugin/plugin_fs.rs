use serde::Serialize;
use std::path::{Path, PathBuf};
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

/// 解析插件 fs 操作的目标绝对路径，并强制它落在插件目录内。
///
/// 目标文件可能尚不存在（写新文件），此时直接 canonicalize 会失败，
/// 因此改为「向上找到第一个存在的祖先目录 → 规范化 → 拼回缺失部分」。
fn resolve_plugin_path(plugin_path: &str, path: &str) -> Result<PathBuf, String> {
    let base = PathBuf::from(plugin_path);
    let raw = Path::new(path);
    let target = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };

    let base_canonical = base
        .canonicalize()
        .map_err(|e| format!("插件目录不可用: {}", e))?;

    let mut probe = target;
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let resolved = loop {
        if let Ok(canonical) = probe.canonicalize() {
            let mut result = canonical;
            for name in missing.iter().rev() {
                result.push(name);
            }
            break result;
        }
        match (
            probe.file_name().map(|n| n.to_os_string()),
            probe.parent().map(|p| p.to_path_buf()),
        ) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                probe = parent;
            }
            _ => return Err("路径不在插件目录内，已拒绝访问".to_string()),
        }
    };

    if resolved.starts_with(&base_canonical) {
        Ok(resolved)
    } else {
        Err("路径不在插件目录内，已拒绝访问".to_string())
    }
}

/// 统一的 fs 前置校验：先按 manifest 声明的权限判定（与沙箱解耦），
/// 再叠加沙箱开关（沙箱启用时拒绝一切 fs 操作）。
fn guard_fs(
    host: &PluginHost,
    plugin_id: &str,
    permission: &str,
    action: &str,
) -> Result<super::PluginInstance, String> {
    let plugin = host.require_permission(plugin_id, permission)?;
    if host.sandbox_enabled() {
        return Err(format!(
            "沙箱已启用，文件系统{}被拒绝（需先在设置中禁用沙箱）",
            action
        ));
    }
    Ok(plugin)
}

// ── Tauri Commands ──

#[tauri::command]
pub fn plugin_fs_read_text(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<String, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugin = guard_fs(&host, &plugin_id, "fs:read", "读取")?;

    let full_path = resolve_plugin_path(&plugin.path, &path)?;

    std::fs::read_to_string(&full_path).map_err(|e| format!("读取文件失败: {}", e))
}

#[tauri::command]
pub fn plugin_fs_write_text(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugin = guard_fs(&host, &plugin_id, "fs:write", "写入")?;

    let full_path = resolve_plugin_path(&plugin.path, &path)?;

    // 创建父目录
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }

    std::fs::write(&full_path, &content).map_err(|e| format!("写入文件失败: {}", e))
}

#[tauri::command]
pub fn plugin_fs_delete(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<(), String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugin = guard_fs(&host, &plugin_id, "fs:write", "删除")?;

    let full_path = resolve_plugin_path(&plugin.path, &path)?;

    if full_path.is_dir() {
        std::fs::remove_dir_all(&full_path).map_err(|e| format!("删除目录失败: {}", e))
    } else {
        std::fs::remove_file(&full_path).map_err(|e| format!("删除文件失败: {}", e))
    }
}

#[tauri::command]
pub fn plugin_fs_exists(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<bool, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugin = guard_fs(&host, &plugin_id, "fs:read", "读取")?;

    let full_path = resolve_plugin_path(&plugin.path, &path)?;
    Ok(full_path.exists())
}

#[tauri::command]
pub fn plugin_fs_read_dir(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    path: String,
) -> Result<Vec<FileEntry>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugin = guard_fs(&host, &plugin_id, "fs:read", "读取")?;

    let full_path = resolve_plugin_path(&plugin.path, &path)?;

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&full_path).map_err(|e| format!("读取目录失败: {}", e))? {
        let entry = entry.map_err(|e| format!("读取条目失败: {}", e))?;
        let metadata = entry
            .metadata()
            .map_err(|e| format!("读取元数据失败: {}", e))?;
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
        });
    }

    Ok(entries)
}
