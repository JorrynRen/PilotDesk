use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ── 权限系统 ──

/// 已定义的合法权限列表
pub const ALL_PERMISSIONS: &[&str] = &[
    "ui:panel",
    "ui:toast",
    "ui:modal",
    "session:read",
    "session:write",
    "data:invoke",
    "storage:*",
    "fs:read",
    "fs:write",
];

/// 默认授予的权限（无需声明）
pub const DEFAULT_PERMISSIONS: &[&str] = &["ui:toast", "storage:*"];

/// 高风险权限（需额外确认）
pub const HIGH_RISK_PERMISSIONS: &[&str] = &["fs:read", "fs:write", "data:invoke"];

/// 权限验证结果
#[derive(Debug, Clone, Serialize)]
pub struct PermissionCheck {
    pub permission: String,
    pub allowed: bool,
    pub reason: Option<String>,
}

/// 沙箱信息
#[derive(Debug, Clone, Serialize)]
pub struct SandboxInfo {
    pub plugins_dir: String,
    pub sandbox_enabled: bool,
    pub max_manifest_size: usize,
    pub allowed_permissions: Vec<String>,
    pub high_risk_permissions: Vec<String>,
}

// ── 数据模型 ──

/// 插件清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(rename = "minAppVersion")]
    pub min_app_version: String,
    pub permissions: Vec<String>,
    pub entry: PluginEntry,
    pub contributes: Option<PluginContributes>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub main: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContributes {
    pub panels: Option<Vec<PanelContribution>>,
    pub commands: Option<Vec<CommandContribution>>,
    pub hooks: Option<Vec<HookContribution>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelContribution {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandContribution {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContribution {
    pub event: String,
    pub handler: String,
}

/// 插件运行时实例
#[derive(Debug, Clone, Serialize)]
pub struct PluginInstance {
    pub manifest: PluginManifest,
    pub enabled: bool,
    pub loaded: bool,
    pub path: String,
    pub error: Option<String>,
    /// 权限检查结果
    pub permission_checks: Vec<PermissionCheck>,
    /// 是否有未授权的权限
    pub has_unauthorized_permissions: bool,
}

/// 清单验证错误
#[derive(Debug, Clone, Serialize)]
pub struct ManifestValidationError {
    pub field: String,
    pub message: String,
}

/// PluginHost — 插件生命周期管理器（含安全沙箱）
pub struct PluginHost {
    plugins_dir: PathBuf,
    plugins: HashMap<String, PluginInstance>,
    /// 最大 manifest.json 文件大小（字节）
    max_manifest_size: usize,
    /// 是否启用沙箱
    sandbox_enabled: bool,
}

impl PluginHost {
    pub fn new() -> Self {
        let plugins_dir = dirs_next::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PilotDesk")
            .join("plugins");

        Self {
            plugins_dir,
            plugins: HashMap::new(),
            max_manifest_size: 1024 * 64, // 64KB
            sandbox_enabled: true,
        }
    }

    // ── 权限验证 ──

    /// 验证单个权限是否合法
    pub fn is_valid_permission(perm: &str) -> bool {
        ALL_PERMISSIONS.contains(&perm)
    }

    /// 验证权限是否为高风险
    pub fn is_high_risk_permission(perm: &str) -> bool {
        HIGH_RISK_PERMISSIONS.contains(&perm)
    }

    /// 检查权限是否为默认授权
    pub fn is_default_permission(perm: &str) -> bool {
        DEFAULT_PERMISSIONS.contains(&perm)
    }

    /// 对插件的所有权限执行检查
    pub fn check_permissions(permissions: &[String]) -> Vec<PermissionCheck> {
        permissions.iter().map(|perm| {
            if !Self::is_valid_permission(perm) {
                PermissionCheck {
                    permission: perm.clone(),
                    allowed: false,
                    reason: Some(format!("未知权限 '{}'，合法权限列表: {}", perm, ALL_PERMISSIONS.join(", "))),
                }
            } else if Self::is_high_risk_permission(perm) {
                PermissionCheck {
                    permission: perm.clone(),
                    allowed: true,
                    reason: Some("高风险权限：请确认插件来源可信".to_string()),
                }
            } else {
                PermissionCheck {
                    permission: perm.clone(),
                    allowed: true,
                    reason: None,
                }
            }
        }).collect()
    }

    /// 检查插件是否拥有指定权限
    pub fn has_permission(instance: &PluginInstance, permission: &str) -> bool {
        if Self::is_default_permission(permission) {
            return true; // 默认权限始终可用
        }
        instance.manifest.permissions.iter().any(|p| p == permission)
            && instance.permission_checks.iter().any(|c| c.permission == permission && c.allowed)
    }

    // ── 清单验证 ──

    /// 验证 manifest.json 的完整性和安全性
    pub fn validate_manifest(manifest: &PluginManifest, plugin_path: &Path) -> Result<Vec<ManifestValidationError>, Vec<ManifestValidationError>> {
        let mut errors: Vec<ManifestValidationError> = Vec::new();

        // 1. id 验证
        if manifest.id.is_empty() {
            errors.push(ManifestValidationError {
                field: "id".to_string(),
                message: "插件 ID 不能为空".to_string(),
            });
        } else if manifest.id.contains("..") || manifest.id.contains('/') || manifest.id.contains('\\') {
            errors.push(ManifestValidationError {
                field: "id".to_string(),
                message: "插件 ID 不能包含路径分隔符".to_string(),
            });
        } else if manifest.id.len() > 128 {
            errors.push(ManifestValidationError {
                field: "id".to_string(),
                message: "插件 ID 长度不能超过 128 个字符".to_string(),
            });
        }

        // 2. 名称验证
        if manifest.name.is_empty() {
            errors.push(ManifestValidationError {
                field: "name".to_string(),
                message: "插件名称不能为空".to_string(),
            });
        } else if manifest.name.len() > 64 {
            errors.push(ManifestValidationError {
                field: "name".to_string(),
                message: "插件名称长度不能超过 64 个字符".to_string(),
            });
        }

        // 3. 版本号格式验证（semver 基本检查）
        if manifest.version.is_empty() {
            errors.push(ManifestValidationError {
                field: "version".to_string(),
                message: "版本号不能为空".to_string(),
            });
        } else {
            let parts: Vec<&str> = manifest.version.split('.').collect();
            if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
                errors.push(ManifestValidationError {
                    field: "version".to_string(),
                    message: "版本号格式无效，应为 semver 格式（如 1.0.0）".to_string(),
                });
            }
        }

        // 4. 权限验证
        for perm in &manifest.permissions {
            if !Self::is_valid_permission(perm) {
                errors.push(ManifestValidationError {
                    field: "permissions".to_string(),
                    message: format!("未知权限 '{}'", perm),
                });
            }
        }

        // 5. 入口文件路径验证（防止路径遍历）
        if manifest.entry.main.contains("..") {
            errors.push(ManifestValidationError {
                field: "entry.main".to_string(),
                message: "入口文件路径不能包含 '..'".to_string(),
            });
        }
        if let Some(ref styles) = manifest.entry.styles {
            if styles.contains("..") {
                errors.push(ManifestValidationError {
                    field: "entry.styles".to_string(),
                    message: "样式文件路径不能包含 '..'".to_string(),
                });
            }
        }

        // 6. 检查入口文件是否存在
        let main_path = plugin_path.join(&manifest.entry.main);
        if !main_path.exists() {
            errors.push(ManifestValidationError {
                field: "entry.main".to_string(),
                message: format!("入口文件不存在: {}", manifest.entry.main),
            });
        }

        if errors.is_empty() {
            Ok(errors)
        } else {
            Err(errors)
        }
    }

    /// 验证文件路径是否在插件目录内（防止路径遍历攻击）
    pub fn is_path_safe(plugin_dir: &Path, relative_path: &str) -> bool {
        let target = plugin_dir.join(relative_path);
        // 规范化路径
        match target.canonicalize() {
            Ok(canonical) => canonical.starts_with(plugin_dir),
            Err(_) => false,
        }
    }

    // ── 插件发现与加载 ──

    /// 扫描插件目录，发现所有插件
    pub fn discover(&mut self) -> Vec<PluginInstance> {
        if !self.plugins_dir.exists() {
            let _ = fs::create_dir_all(&self.plugins_dir);
            return Vec::new();
        }

        let mut instances = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let manifest_path = path.join("manifest.json");
                if !manifest_path.exists() {
                    continue;
                }

                match self.load_and_validate_plugin(&path, &manifest_path) {
                    Ok(instance) => {
                        let key = instance.path.clone();
                        self.plugins.insert(key, instance.clone());
                        instances.push(instance);
                    }
                    Err(e) => {
                        log::warn!("[PluginSandbox] 插件加载失败 {:?}: {}", manifest_path, e);
                    }
                }
            }
        }

        instances
    }

    /// 加载并验证单个插件
    fn load_and_validate_plugin(&self, plugin_path: &Path, manifest_path: &Path) -> Result<PluginInstance, String> {
        // 1. 文件大小检查（防止大文件攻击）
        let metadata = fs::metadata(manifest_path)
            .map_err(|e| format!("读取清单元数据失败: {}", e))?;
        if metadata.len() > self.max_manifest_size as u64 {
            return Err(format!(
                "manifest.json 文件过大: {} 字节（最大允许: {} 字节）",
                metadata.len(), self.max_manifest_size
            ));
        }

        // 2. 读取清单
        let content = fs::read_to_string(manifest_path)
            .map_err(|e| format!("读取清单失败: {}", e))?;

        // 3. JSON 大小检查（防止深度嵌套攻击）
        if content.len() > self.max_manifest_size {
            return Err("manifest.json 内容超过最大允许大小".to_string());
        }

        let manifest: PluginManifest = serde_json::from_str(&content)
            .map_err(|e| format!("解析清单失败: {}", e))?;

        // 4. 路径遍历检查
        let plugin_dir_str = plugin_path.to_string_lossy().to_string();
        if plugin_dir_str.contains("..") {
            return Err("插件目录路径包含 '..'，已拒绝加载".to_string());
        }

        // 5. 清单字段验证
        match Self::validate_manifest(&manifest, plugin_path) {
            Ok(_) => {}
            Err(errors) => {
                let error_msg = errors.iter()
                    .map(|e| format!("[{}] {}", e.field, e.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(format!("清单验证失败: {}", error_msg));
            }
        }

        // 6. 权限检查（沙箱禁用时放行所有权限）
        let permission_checks = if self.sandbox_enabled {
            Self::check_permissions(&manifest.permissions)
        } else {
            // 沙箱禁用时，所有权限标记为允许
            manifest.permissions.iter().map(|perm| PermissionCheck {
                permission: perm.clone(),
                allowed: true,
                reason: Some("沙箱已禁用，权限检查已跳过".to_string()),
            }).collect::<Vec<_>>()
        };
        let has_unauthorized = permission_checks.iter().any(|c| !c.allowed);

        // 7. 构建实例
        let instance = PluginInstance {
            enabled: true,
            loaded: false,
            path: plugin_dir_str,
            error: if has_unauthorized {
                Some("包含未授权的权限声明".to_string())
            } else {
                None
            },
            manifest: manifest.clone(),
            permission_checks,
            has_unauthorized_permissions: has_unauthorized,
        };

        Ok(instance)
    }

    // ── 查询 ──

    /// 获取所有已发现的插件
    pub fn list_plugins(&self) -> Vec<PluginInstance> {
        self.plugins.values().cloned().collect()
    }

    /// 设置沙箱启用/禁用状态
    pub fn set_sandbox_enabled(&mut self, enabled: bool) {
        self.sandbox_enabled = enabled;
        log::info!("[PluginSandbox] 沙箱状态已切换: {}", if enabled { "启用" } else { "禁用" });
    }

    /// 获取沙箱信息
    pub fn get_sandbox_info(&self) -> SandboxInfo {
        SandboxInfo {
            plugins_dir: self.plugins_dir.to_string_lossy().to_string(),
            sandbox_enabled: self.sandbox_enabled,
            max_manifest_size: self.max_manifest_size,
            allowed_permissions: ALL_PERMISSIONS.iter().map(|s| s.to_string()).collect(),
            high_risk_permissions: HIGH_RISK_PERMISSIONS.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ── 启用/禁用 ──

    /// 通过路径或ID查找插件
    fn find_plugin_key(&self, path_or_id: &str) -> Option<String> {
        if self.plugins.contains_key(path_or_id) {
            return Some(path_or_id.to_string());
        }
        self.plugins.iter()
            .find(|(_, p)| p.manifest.id == path_or_id)
            .map(|(k, _)| k.clone())
    }

    /// 启用插件（含权限校验）
    pub fn enable_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self.find_plugin_key(path_or_id).ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let plugin = self.plugins.get_mut(&key).ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        if plugin.has_unauthorized_permissions {
            return Err(format!(
                "插件 '{}' 包含未授权的权限，无法启用。请检查 manifest.json 中的 permissions 字段。",
                plugin.manifest.name
            ));
        }
        plugin.enabled = true;
        Ok(())
    }

    /// 禁用插件
    pub fn disable_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self.find_plugin_key(path_or_id).ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let plugin = self.plugins.get_mut(&key).ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        plugin.enabled = false;
        Ok(())
    }

    /// 获取单个插件详情
    pub fn get_plugin(&self, path_or_id: &str) -> Option<&PluginInstance> {
        let key = self.find_plugin_key(path_or_id)?;
        self.plugins.get(&key)
    }

    // ── 安装与卸载 ──

    /// 从 zip 文件安装插件
    pub fn install_from_zip(&mut self, zip_path: &str) -> Result<PluginInstance, String> {
        let zip_path = PathBuf::from(zip_path);
        if !zip_path.exists() {
            return Err(format!("文件不存在: {}", zip_path.display()));
        }

        // 确保插件目录存在
        fs::create_dir_all(&self.plugins_dir)
            .map_err(|e| format!("创建插件目录失败: {}", e))?;

        // 打开 zip 文件
        let file = fs::File::open(&zip_path)
            .map_err(|e| format!("打开文件失败: {}", e))?;

        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("读取压缩包失败: {}", e))?;

        // 查找 manifest.json 确定插件 ID
        let mut manifest_content: Option<String> = None;
        let mut plugin_dir_name: Option<String> = None;

        // 第一遍扫描：找到 manifest.json
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)
                .map_err(|e| format!("读取压缩包条目失败: {}", e))?;

            let entry_name = entry.name().to_string();

            // 支持两种结构: manifest.json (根目录) 或 plugin-name/manifest.json (有父目录)
            if entry_name == "manifest.json" || entry_name.ends_with("/manifest.json") {
                let mut content = String::new();
                use std::io::Read;
                entry.read_to_string(&mut content)
                    .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
                manifest_content = Some(content);

                if entry_name == "manifest.json" {
                    plugin_dir_name = Some(".".to_string());
                } else {
                    plugin_dir_name = Some(entry_name.trim_end_matches("/manifest.json").to_string());
                }
                break;
            }
        }

        let manifest_content = manifest_content.ok_or_else(|| "压缩包中未找到 manifest.json".to_string())?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_content)
            .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

        // 验证插件 ID
        if manifest.id.contains("..") || manifest.id.contains('/') || manifest.id.contains('\\') {
            return Err("插件 ID 包含非法字符".to_string());
        }

        // 检查是否已存在（按路径而非ID）
        let target_dir = self.plugins_dir.join(&manifest.id);
        if target_dir.exists() {
            return Err(format!("插件目录 '{}' 已存在", target_dir.display()));
        }

        // 目标目录: plugins/<plugin-id>/
        let target_dir = self.plugins_dir.join(&manifest.id);

        // 如果目标目录已存在，先删除
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)
                .map_err(|e| format!("清理旧目录失败: {}", e))?;
        }

        // 第二遍：解压文件
        let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path)
            .map_err(|e| format!("重新打开文件失败: {}", e))?)
            .map_err(|e| format!("读取压缩包失败: {}", e))?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)
                .map_err(|e| format!("读取条目失败: {}", e))?;

            let entry_name = entry.name().to_string();

            // 跳过目录条目
            if entry_name.ends_with('/') {
                continue;
            }

            // 计算相对路径（去除插件目录前缀）
            let relative_path = if let Some(ref dir_name) = plugin_dir_name {
                if dir_name == "." {
                    entry_name.clone()
                } else if entry_name.starts_with(dir_name) {
                    entry_name[dir_name.len() + 1..].to_string()
                } else {
                    entry_name.clone()
                }
            } else {
                entry_name.clone()
            };

            // 路径遍历检查
            if relative_path.contains("..") {
                log::warn!("[PluginInstall] 跳过路径遍历: {}", relative_path);
                continue;
            }

            let target_path = target_dir.join(&relative_path);

            // 创建父目录
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录失败: {}", e))?;
            }

            // 写入文件
            let mut output = fs::File::create(&target_path)
                .map_err(|e| format!("创建文件失败: {}", e))?;
            use std::io::copy;
            copy(&mut entry, &mut output)
                .map_err(|e| format!("写入文件失败: {}", e))?;
        }

        // 加载并验证安装的插件
        let manifest_path = target_dir.join("manifest.json");
        let instance = self.load_and_validate_plugin(&target_dir, &manifest_path)?;

        let id = instance.manifest.id.clone();
        self.plugins.insert(id.clone(), instance.clone());

        log::info!("[PluginInstall] 插件 '{}' 安装成功", manifest.name);

        Ok(instance)
    }

    /// 卸载插件
    pub fn uninstall_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self.find_plugin_key(path_or_id).ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let plugin = self.plugins.get(&key)
            .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;

        let plugin_path = PathBuf::from(&plugin.path);

        // 从 HashMap 移除
        self.plugins.remove(&key);

        // 删除目录
        if plugin_path.exists() {
            fs::remove_dir_all(&plugin_path)
                .map_err(|e| format!("删除插件目录失败: {}", e))?;
        }

        log::info!("[PluginUninstall] 插件 '{}' 已卸载", path_or_id);
        Ok(())
    }
}

// ── Tauri Commands ──

#[tauri::command]
pub fn plugin_discover(host: tauri::State<'_, std::sync::Mutex<PluginHost>>) -> Result<Vec<PluginInstance>, String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    Ok(host.discover())
}

#[tauri::command]
pub fn plugin_list(host: tauri::State<'_, std::sync::Mutex<PluginHost>>) -> Result<Vec<PluginInstance>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    Ok(host.list_plugins())
}

#[tauri::command]
pub fn plugin_enable(host: tauri::State<'_, std::sync::Mutex<PluginHost>>, id: String) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.enable_plugin(&id)
}

#[tauri::command]
pub fn plugin_disable(host: tauri::State<'_, std::sync::Mutex<PluginHost>>, id: String) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.disable_plugin(&id)
}

#[tauri::command]
pub fn plugin_get_sandbox_info(host: tauri::State<'_, std::sync::Mutex<PluginHost>>) -> Result<SandboxInfo, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    Ok(host.get_sandbox_info())
}

#[tauri::command]
pub fn plugin_set_sandbox_enabled(host: tauri::State<'_, std::sync::Mutex<PluginHost>>, enabled: bool) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.set_sandbox_enabled(enabled);
    Ok(())
}

/// 从 zip 文件安装插件
#[tauri::command]
pub fn plugin_install_zip(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    zip_path: String,
) -> Result<PluginInstance, String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.install_from_zip(&zip_path)
}

/// 删除插件
#[tauri::command]
pub fn plugin_uninstall(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    id: String,
) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.uninstall_plugin(&id)
}

/// 读取插件入口文件内容
#[tauri::command]
pub fn plugin_read_entry(host: tauri::State<'_, std::sync::Mutex<PluginHost>>, plugin_id: String) -> Result<String, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    let entry_path = std::path::Path::new(&plugin.path).join(&plugin.manifest.entry.main);
    let content = std::fs::read_to_string(&entry_path)
        .map_err(|e| format!("读取入口文件失败: {}", e))?;

    Ok(content)
}

/// 读取插件图标文件，返回 base64 data URL
/// 支持图片格式：png, jpg, jpeg, gif, svg, webp, ico
#[tauri::command]
pub fn plugin_read_icon_file(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    plugin_id: String,
    icon_path: String,
) -> Result<String, String> {
    use std::io::Read;

    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    // 路径遍历检查
    if icon_path.contains("..") {
        return Err("图标路径不能包含 '..'".to_string());
    }

    let full_path = std::path::Path::new(&plugin.path).join(&icon_path);
    if !full_path.exists() {
        return Err(format!("图标文件不存在: {}", icon_path));
    }

    // 读取文件
    let mut file = std::fs::File::open(&full_path)
        .map_err(|e| format!("读取图标文件失败: {}", e))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取图标文件失败: {}", e))?;

    // 根据扩展名推断 MIME 类型
    let ext = full_path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:{};base64,{}", mime, b64))
}

/// 获取插件面板内容
#[tauri::command]
pub fn plugin_get_panel_content(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    plugin_id: String,
    panel_id: String,
) -> Result<String, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    // 构建面板内容：显示插件基本信息
    let mut content = String::new();
    content.push_str(&format!("插件: {}\n", plugin.manifest.name));
    content.push_str(&format!("版本: {}\n", plugin.manifest.version));
    content.push_str(&format!("作者: {}\n", plugin.manifest.author));
    content.push_str(&format!("描述: {}\n", plugin.manifest.description));
    content.push_str(&format!("路径: {}\n", plugin.path));
    content.push_str(&format!("沙箱: {}\n", if host.get_sandbox_info().sandbox_enabled { "已启用" } else { "已禁用" }));
    content.push_str(&format!("面板ID: {}\n", panel_id));

    Ok(content)
}
