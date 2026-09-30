pub mod agent;
pub mod plugin_fs;
pub mod shell;
pub mod store;

use crate::workflow::registry::{NodeCategory, NodeTypeRegistration};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── 权限系统 ──

/// 已定义的合法权限列表
pub const ALL_PERMISSIONS: &[&str] = &[
    "ui:panel",
    "ui:toast",
    "ui:modal",
    "session:read",
    "session:write",
    "session:execute",
    "data:invoke",
    "storage:*",
    "fs:read",
    "fs:write",
    "shell:exec",
    "plugin:call",
    "plugin:events",
    "workflow:read",
    "workflow:write",
    "workflow:trigger",
];

/// 默认授予的权限（无需声明）
pub const DEFAULT_PERMISSIONS: &[&str] = &["ui:toast", "storage:*"];

/// 高风险权限（需额外确认）
pub const HIGH_RISK_PERMISSIONS: &[&str] = &[
    "fs:read",
    "fs:write",
    "data:invoke",
    "shell:exec",
    "session:execute",
];

/// data:invoke 允许插件透传的命令白名单。
///
/// 只放行「只读、无副作用、且不返回明文密钥」的命令；`get_api_key`、
/// `fs_util::write_text_file`、`plugin_install_zip`、`terminal_*`、`set_app_setting`
/// 等高风险命令必须排除在外。新增条目需逐条评估后再加入。
pub const PLUGIN_DATA_INVOKE_ALLOWLIST: &[&str] = &["list_api_providers", "get_api_provider"];

/// 命令 input 模式允许的属性类型（平台据它生成参数表单）
const COMMAND_INPUT_PROPERTY_TYPES: &[&str] =
    &["string", "number", "integer", "boolean", "array", "object"];

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
    pub icon: Option<String>,
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
    #[serde(default)]
    pub node_types: Option<Vec<NodeTypeContribution>>,
    #[serde(default)]
    pub workflow_config: Option<WorkflowConfigContribution>,
}

/// Plugin-provided config component rendered inside workflow plugin node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfigContribution {
    /// Name of the exported component in plugin entry JS (e.g. "PluginNodeConfig")
    /// Used as the default parameter form for every command of the plugin.
    #[serde(default)]
    pub component: Option<String>,
    /// Per-command parameter forms; keys are `contributes.commands[].id`, values are
    /// exported component names. Take precedence over `component`.
    #[serde(default)]
    pub components: Option<HashMap<String, String>>,
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
    #[serde(default)]
    #[serde(rename = "input")]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default)]
    #[serde(rename = "output")]
    pub output_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContribution {
    pub event: String,
    pub handler: String,
}

/// 工作流节点类型贡献点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTypeContribution {
    pub type_id: String,
    pub name: String,
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub permissions: Vec<String>,
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
    /// 被停用的插件 id 集合（持久化到 plugins_dir 下的 `.disabled.json`）
    ///
    /// 停用状态不能只靠内存里的 `PluginInstance.enabled`：`discover()` 会从磁盘
    /// 重新扫描并重建实例，若不持久化，"插件停用后一刷新就变回启用"。
    ///
    /// **键是 `manifest.id` 而不是 `path`**（与 `self.plugins` 用 path 作 key 不同，是有意的）：
    /// 停用是**用户视角的开关**，用户认的是插件身份；插件目录被移动/改名不该让停用失效。
    /// 而"同一 id 出现在两个目录"本身就是非法状态（节点类型等也会撞车），不需要为它设计语义。
    disabled_ids: HashSet<String>,
    /// 最大 manifest.json 文件大小（字节）
    max_manifest_size: usize,
    /// 是否启用沙箱
    sandbox_enabled: bool,
    /// 工作流节点类型注册回调（由 NodeExecutor 在初始化时设置）
    register_node_type: Option<Box<dyn Fn(NodeTypeRegistration) + Send + Sync>>,
    /// 工作流节点类型注销回调（按插件 id 批量注销，由 NodeExecutor 在初始化时设置）
    unregister_node_types: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// "列出已注册节点类型的插件 id"回调（由 NodeExecutor 在初始化时设置）。
    /// discover() 借它收敛那些目录已被手动删除、但节点类型仍残留在注册表里的插件。
    list_registered_plugin_ids: Option<Box<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// 插件节点执行通道管理器（由 NodeExecutor 在初始化时设置，与 respond_plugin_execute 命令共享）
    plugin_execute_manager:
        Option<std::sync::Arc<crate::workflow::executors::plugin_executor::PluginExecuteManager>>,
}

impl PluginHost {
    pub fn new() -> Self {
        let plugins_dir = crate::utils::paths::plugins_dir();
        // 启动时读回停用集合，保证应用重启后停用状态依然生效
        let disabled_ids = Self::load_disabled_ids_from(&plugins_dir);

        Self {
            plugins_dir,
            plugins: HashMap::new(),
            disabled_ids,
            max_manifest_size: 1024 * 64, // 64KB
            sandbox_enabled: true,
            register_node_type: None,
            unregister_node_types: None,
            list_registered_plugin_ids: None,
            plugin_execute_manager: None,
        }
    }

    // ── 停用状态持久化 ──

    /// 停用记录文件路径：`<plugins_dir>/.disabled.json`，内容是插件 id 的 JSON 数组。
    ///
    /// 放在 plugins_dir 下是安全的：`discover()` 扫描时用
    /// `if !path.is_dir() { continue; }` 跳过所有非目录项，
    /// 因此这个普通文件不会被误当成插件目录。
    /// 若将来修改扫描逻辑（例如改为递归/按扩展名匹配），务必保留"只认目录"的判断。
    fn disabled_file_path(&self) -> PathBuf {
        self.plugins_dir.join(".disabled.json")
    }

    /// 从指定目录读取停用集合。
    ///
    /// 语义：文件不存在 ⇒ 没有任何插件被停用（正常情况，不是错误）；
    /// 文件损坏/非法 JSON ⇒ 记一条 warn 并按空集合处理，避免插件系统整体不可用。
    fn load_disabled_ids_from(plugins_dir: &Path) -> HashSet<String> {
        let path = plugins_dir.join(".disabled.json");
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("[PluginHost] 读取停用记录失败 {:?}: {}", path, e);
                }
                return HashSet::new();
            }
        };
        match serde_json::from_str::<Vec<String>>(&content) {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                log::warn!(
                    "[PluginHost] 停用记录 {:?} 不是合法的 JSON 字符串数组，已按空集合处理: {}",
                    path,
                    e
                );
                HashSet::new()
            }
        }
    }

    /// 把当前停用集合落盘（写失败返回明确错误，不静默）。
    fn persist_disabled_ids(&self) -> Result<(), String> {
        let path = self.disabled_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建插件目录失败: {}", e))?;
        }
        // 排序后再写，保证文件内容稳定、便于人工查看 diff
        let mut ids: Vec<&String> = self.disabled_ids.iter().collect();
        ids.sort();
        let content =
            serde_json::to_string_pretty(&ids).map_err(|e| format!("序列化停用记录失败: {}", e))?;
        fs::write(&path, content).map_err(|e| format!("写入停用记录失败 {:?}: {}", path, e))
    }

    /// 设置插件节点执行通道管理器（由 NodeExecutor 在初始化时调用）
    pub fn set_plugin_execute_manager(
        &mut self,
        manager: std::sync::Arc<crate::workflow::executors::plugin_executor::PluginExecuteManager>,
    ) {
        self.plugin_execute_manager = Some(manager);
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
        permissions
            .iter()
            .map(|perm| {
                if !Self::is_valid_permission(perm) {
                    PermissionCheck {
                        permission: perm.clone(),
                        allowed: false,
                        reason: Some(format!(
                            "未知权限 '{}'，合法权限列表: {}",
                            perm,
                            ALL_PERMISSIONS.join(", ")
                        )),
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
            })
            .collect()
    }

    /// 检查插件是否拥有指定权限
    pub fn has_permission(instance: &PluginInstance, permission: &str) -> bool {
        if Self::is_default_permission(permission) {
            return true; // 默认权限始终可用
        }
        instance
            .manifest
            .permissions
            .iter()
            .any(|p| p == permission)
            && instance
                .permission_checks
                .iter()
                .any(|c| c.permission == permission && c.allowed)
    }

    /// 权限校验的唯一入口：插件必须存在，且在其 manifest 中显式声明了该权限。
    ///
    /// 与 `sandbox_enabled` 解耦 —— 关闭沙箱只是额外放宽（见各命令），
    /// 不再等价于「授予全部权限」，任何高风险权限都必须命中 manifest 声明。
    pub fn require_permission(
        &self,
        plugin_id: &str,
        permission: &str,
    ) -> Result<PluginInstance, String> {
        let key = self
            .find_plugin_key(plugin_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
        let plugin = self
            .plugins
            .get(&key)
            .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

        // 已停用的插件一律拒绝：停用必须是真正的闸门，而不只是 UI 可见性开关。
        // 这条让 fs / shell / agent / data:invoke 等能力在停用后立即失效。
        if !plugin.enabled {
            return Err(format!(
                "插件 '{}' 已停用，调用被拒绝",
                plugin.manifest.name
            ));
        }

        if !Self::has_permission(plugin, permission) {
            return Err(format!(
                "插件 '{}' 未声明权限 '{}'，调用被拒绝",
                plugin.manifest.name, permission
            ));
        }

        Ok(plugin.clone())
    }

    /// 沙箱是否启用（供命令决定是否额外收紧）
    pub fn sandbox_enabled(&self) -> bool {
        self.sandbox_enabled
    }

    // ── 清单验证 ──

    /// 验证 manifest.json 的完整性和安全性
    pub fn validate_manifest(
        manifest: &PluginManifest,
        plugin_path: &Path,
    ) -> Result<Vec<ManifestValidationError>, Vec<ManifestValidationError>> {
        let mut errors: Vec<ManifestValidationError> = Vec::new();

        // 1. id 验证
        if manifest.id.is_empty() {
            errors.push(ManifestValidationError {
                field: "id".to_string(),
                message: "插件 ID 不能为空".to_string(),
            });
        } else if manifest.id.contains("..")
            || manifest.id.contains('/')
            || manifest.id.contains('\\')
        {
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

        // 6. 检查入口文件是否存在
        let main_path = plugin_path.join(&manifest.entry.main);
        if !main_path.exists() {
            errors.push(ManifestValidationError {
                field: "entry.main".to_string(),
                message: format!("入口文件不存在: {}", manifest.entry.main),
            });
        }

        // 7. 工作流贡献点验证
        if let Some(contributes) = &manifest.contributes {
            Self::validate_workflow_contributions(contributes, &mut errors);
        }

        if errors.is_empty() {
            Ok(errors)
        } else {
            Err(errors)
        }
    }

    /// 校验工作流贡献点：命令 id 唯一、命令级配置组件的键是已声明的命令、命令 input 模式可被平台渲染
    fn validate_workflow_contributions(
        contributes: &PluginContributes,
        errors: &mut Vec<ManifestValidationError>,
    ) {
        let commands = contributes.commands.as_deref().unwrap_or(&[]);

        let mut seen_command_ids: HashMap<&str, ()> = HashMap::new();
        for command in commands {
            if seen_command_ids.insert(command.id.as_str(), ()).is_some() {
                errors.push(ManifestValidationError {
                    field: format!("contributes.commands[{}]", command.id),
                    message: "命令 id 重复，工作流节点无法区分这两个命令".to_string(),
                });
            }
        }

        if let Some(config) = &contributes.workflow_config {
            let has_component =
                matches!(config.component.as_deref(), Some(name) if !name.is_empty());
            let components = config.components.as_ref();
            if !has_component && components.map_or(true, |map| map.is_empty()) {
                errors.push(ManifestValidationError {
                    field: "contributes.workflow_config".to_string(),
                    message: "必须声明 component 或 components 之一，否则节点没有参数表单"
                        .to_string(),
                });
            }
            for (command_id, component) in components.into_iter().flatten() {
                if !commands.iter().any(|command| &command.id == command_id) {
                    errors.push(ManifestValidationError {
                        field: format!("contributes.workflow_config.components.{}", command_id),
                        message: "命令 id 未在 contributes.commands 中声明，该组件不会被渲染"
                            .to_string(),
                    });
                }
                if component.is_empty() {
                    errors.push(ManifestValidationError {
                        field: format!("contributes.workflow_config.components.{}", command_id),
                        message: "组件名不能为空".to_string(),
                    });
                }
            }
        }

        for command in commands {
            let Some(schema) = &command.input_schema else {
                continue;
            };
            Self::validate_command_input_schema(&command.id, schema, errors);
        }
    }

    /// 校验命令 input 模式：平台按它生成参数表单，越界写法会在界面上静默丢失字段
    fn validate_command_input_schema(
        command_id: &str,
        schema: &serde_json::Value,
        errors: &mut Vec<ManifestValidationError>,
    ) {
        let field_prefix = format!("contributes.commands[{}].input", command_id);
        let Some(object) = schema.as_object() else {
            errors.push(ManifestValidationError {
                field: field_prefix,
                message: "input 必须是 JSON 对象".to_string(),
            });
            return;
        };

        // input.type 可省略（省略即 object），写了就必须是 "object"
        if let Some(kind) = object.get("type").and_then(|value| value.as_str()) {
            if kind != "object" {
                errors.push(ManifestValidationError {
                    field: format!("{}.type", field_prefix),
                    message: "input.type 必须是 \"object\"（省略即视为 object）".to_string(),
                });
            }
        }

        let properties = object
            .get("properties")
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default();
        for (name, property) in &properties {
            let property_type = property.get("type").and_then(|value| value.as_str());
            match property_type {
                Some(kind) if COMMAND_INPUT_PROPERTY_TYPES.contains(&kind) => {}
                Some(kind) => errors.push(ManifestValidationError {
                    field: format!("{}.properties.{}.type", field_prefix, name),
                    message: format!(
                        "不支持的类型 '{}'，可选: {}",
                        kind,
                        COMMAND_INPUT_PROPERTY_TYPES.join(", ")
                    ),
                }),
                None => errors.push(ManifestValidationError {
                    field: format!("{}.properties.{}.type", field_prefix, name),
                    message: "每个属性都必须声明 type".to_string(),
                }),
            }
            if let Some(enum_values) = property.get("enum") {
                match enum_values.as_array() {
                    Some(values) if !values.is_empty() => {}
                    _ => errors.push(ManifestValidationError {
                        field: format!("{}.properties.{}.enum", field_prefix, name),
                        message: "enum 必须是非空数组".to_string(),
                    }),
                }
            }
        }

        if let Some(required) = object.get("required") {
            let names = required
                .as_array()
                .filter(|names| names.iter().all(|name| name.as_str().is_some()));
            match names {
                Some(names) => {
                    for name in names {
                        let name = name.as_str().unwrap_or_default();
                        if !properties.contains_key(name) {
                            errors.push(ManifestValidationError {
                                field: format!("{}.required", field_prefix),
                                message: format!("必填项 '{}' 未在 properties 中声明", name),
                            });
                        }
                    }
                }
                None => errors.push(ManifestValidationError {
                    field: format!("{}.required", field_prefix),
                    message: "required 必须是字符串数组".to_string(),
                }),
            }
        }
    }

    /// 验证文件路径是否在插件目录内（防止路径遍历攻击）
    #[allow(dead_code)]
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
            // 目录不存在等价于"一个插件都没有"：内存表也要同步清空，
            // 否则手动删掉整个 plugins 目录后，旧条目会一直留在 list 里（要到下次 discover 才自愈）。
            self.plugins.clear();
            return Vec::new();
        }

        let mut instances = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.plugins_dir) {
            // 重建而非追加：self.plugins 必须与磁盘一致。若只覆盖同名 key，手动删除
            // 的插件会以旧 key 残留成幽灵条目（list 重复、且下面第 B 步收敛失效）。
            self.plugins.clear();
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
                        // 节点类型注册已在 load_and_validate_plugin 第 8 步按 enabled 完成，
                        // 此处不再重复注册（重复注册会在停用插件被重扫时把节点类型又塞回注册表）。
                        self.plugins.insert(key, instance.clone());
                        instances.push(instance);
                    }
                    Err(e) => {
                        log::warn!("[PluginSandbox] 插件加载失败 {:?}: {}", manifest_path, e);
                    }
                }
            }
        }

        // ── 收敛：注册表里仍有节点类型、但磁盘（self.plugins）里已消失的插件 ──
        //
        // 典型场景：插件目录被手动删除、没走 uninstall，此时没有代码替它注销节点类型，
        // 其类型会永久残留在节点面板里。停用插件不会被误伤：它仍在 self.plugins 中
        // （enabled=false），且其节点类型在停用时已被注销，不会出现在"已注册"集合里。
        if let Some(ref callback) = self.list_registered_plugin_ids {
            let present: HashSet<&str> = self
                .plugins
                .values()
                .map(|p| p.manifest.id.as_str())
                .collect();
            for plugin_id in callback() {
                if !present.contains(plugin_id.as_str()) {
                    log::info!("[PluginSandbox] 收敛已消失插件的节点类型: {}", plugin_id);
                    self.unregister_plugin_node_types(&plugin_id);
                }
            }
        }

        instances
    }

    /// 加载并验证单个插件
    pub(crate) fn load_and_validate_plugin(
        &self,
        plugin_path: &Path,
        manifest_path: &Path,
    ) -> Result<PluginInstance, String> {
        // 1. 文件大小检查（防止大文件攻击）
        let metadata =
            fs::metadata(manifest_path).map_err(|e| format!("读取清单元数据失败: {}", e))?;
        if metadata.len() > self.max_manifest_size as u64 {
            return Err(format!(
                "manifest.json 文件过大: {} 字节（最大允许: {} 字节）",
                metadata.len(),
                self.max_manifest_size
            ));
        }

        // 2. 读取清单
        let content =
            fs::read_to_string(manifest_path).map_err(|e| format!("读取清单失败: {}", e))?;

        // 3. JSON 大小检查（防止深度嵌套攻击）
        if content.len() > self.max_manifest_size {
            return Err("manifest.json 内容超过最大允许大小".to_string());
        }

        let manifest: PluginManifest =
            serde_json::from_str(&content).map_err(|e| format!("解析清单失败: {}", e))?;

        // 4. 路径遍历检查
        let plugin_dir_str = plugin_path.to_string_lossy().to_string();
        if plugin_dir_str.contains("..") {
            return Err("插件目录路径包含 '..'，已拒绝加载".to_string());
        }

        // 5. 清单字段验证（沙箱禁用时跳过）
        if self.sandbox_enabled {
            match Self::validate_manifest(&manifest, plugin_path) {
                Ok(_) => {}
                Err(errors) => {
                    let error_msg = errors
                        .iter()
                        .map(|e| format!("[{}] {}", e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(format!("清单验证失败: {}", error_msg));
                }
            }
        } else {
            log::info!("[PluginSandbox] 沙箱已禁用，跳过清单验证");
        }

        // 6. 权限检查（沙箱禁用时放行所有权限）
        let permission_checks = if self.sandbox_enabled {
            Self::check_permissions(&manifest.permissions)
        } else {
            // 沙箱禁用时，所有权限标记为允许
            manifest
                .permissions
                .iter()
                .map(|perm| PermissionCheck {
                    permission: perm.clone(),
                    allowed: true,
                    reason: Some("沙箱已禁用，权限检查已跳过".to_string()),
                })
                .collect::<Vec<_>>()
        };
        let has_unauthorized = permission_checks.iter().any(|c| !c.allowed);

        // 7. 构造实例的 enabled 标志（由持久化的停用集合决定：仅凭磁盘重扫时不能硬编码
        //    为 true，否则停用过的插件在 discover()/重启后会被重新启用）。
        let enabled = !self.disabled_ids.contains(&manifest.id);

        // 8. 注册插件声明的节点类型
        // 仅启用的插件才注册；停用插件的节点类型不应留在注册表里（否则仍可被拖出）。
        if enabled {
            self.register_plugin_node_types(&manifest);
        }

        let instance = PluginInstance {
            enabled,
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

    /// 获取所有插件贡献的工作流节点类型
    #[allow(dead_code)]
    pub fn get_contributed_node_types(&self) -> Vec<(String, NodeTypeContribution)> {
        let mut result = Vec::new();
        for (_, instance) in &self.plugins {
            if !instance.enabled {
                continue;
            }
            if let Some(node_types) = instance
                .manifest
                .contributes
                .as_ref()
                .and_then(|c| c.node_types.as_ref())
            {
                for nt in node_types {
                    result.push((instance.manifest.id.clone(), nt.clone()));
                }
            }
        }
        result
    }

    /// 设置沙箱启用/禁用状态
    pub fn set_sandbox_enabled(&mut self, enabled: bool) {
        self.sandbox_enabled = enabled;
        log::info!(
            "[PluginSandbox] 沙箱状态已切换: {}",
            if enabled { "启用" } else { "禁用" }
        );
    }

    /// 设置工作流节点类型注册回调（由 NodeExecutor 在初始化时调用）
    pub fn set_register_node_type(
        &mut self,
        callback: Box<dyn Fn(NodeTypeRegistration) + Send + Sync>,
    ) {
        self.register_node_type = Some(callback);
    }

    /// 设置工作流节点类型注销回调（由 NodeExecutor 在初始化时调用）。
    /// 回调接收插件 id，批量注销该插件贡献的全部节点类型。
    pub fn set_unregister_node_types(&mut self, callback: Box<dyn Fn(&str) + Send + Sync>) {
        self.unregister_node_types = Some(callback);
    }

    /// 设置"列出已注册节点类型的插件 id"回调（由 NodeExecutor 在初始化时调用）。
    /// 与 register / unregister 两个回调成一套，供 discover() 收敛已消失的插件。
    pub fn set_list_registered_plugin_ids(
        &mut self,
        callback: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    ) {
        self.list_registered_plugin_ids = Some(callback);
    }

    /// 为插件 manifest 中声明的节点类型构造注册信息。
    ///
    /// 注册（load_and_validate_plugin / enable_plugin）路径共用此方法，
    /// 避免同一段构造代码在多处重复。
    fn build_node_type_registrations(
        &self,
        manifest: &PluginManifest,
    ) -> Vec<NodeTypeRegistration> {
        let node_types = match manifest
            .contributes
            .as_ref()
            .and_then(|c| c.node_types.as_ref())
        {
            Some(node_types) => node_types,
            None => return Vec::new(),
        };

        // 复用一个执行通道管理器；未设置时退化为新建一个（与原逻辑一致）。
        let execute_manager = self.plugin_execute_manager.clone().unwrap_or_else(|| {
            Arc::new(crate::workflow::executors::plugin_executor::PluginExecuteManager::new())
        });

        node_types
            .iter()
            .map(|nt| NodeTypeRegistration {
                type_id: nt.type_id.clone(),
                name: nt.name.clone(),
                category: NodeCategory::Plugin(manifest.id.clone()),
                executor: Arc::new(
                    crate::workflow::executors::plugin_executor::PluginExecutor::new(
                        manifest.id.clone(),
                        nt.type_id.clone(),
                        execute_manager.clone(),
                    ),
                ),
                config_schema: nt.config_schema.clone(),
                permissions: nt.permissions.clone(),
            })
            .collect()
    }

    /// 注册插件贡献的节点类型（仅当注册回调已设置时生效）。
    fn register_plugin_node_types(&self, manifest: &PluginManifest) {
        if let Some(ref callback) = self.register_node_type {
            for registration in self.build_node_type_registrations(manifest) {
                callback(registration);
            }
        }
    }

    /// 注销插件贡献的节点类型（仅当注销回调已设置时生效）。
    fn unregister_plugin_node_types(&self, plugin_id: &str) {
        if let Some(ref callback) = self.unregister_node_types {
            callback(plugin_id);
        }
    }

    /// 获取沙箱信息
    pub fn get_sandbox_info(&self) -> SandboxInfo {
        SandboxInfo {
            plugins_dir: self.plugins_dir.to_string_lossy().to_string(),
            sandbox_enabled: self.sandbox_enabled,
            max_manifest_size: self.max_manifest_size,
            allowed_permissions: ALL_PERMISSIONS.iter().map(|s| s.to_string()).collect(),
            high_risk_permissions: HIGH_RISK_PERMISSIONS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    // ── 启用/禁用 ──

    /// 在 `self.plugins` 中定位插件，返回其 HashMap key。
    ///
    /// key 统一是 `plugin.path`（path 唯一且稳定），但对外仍接受 `manifest.id`：
    /// 前端命令（如 `plugin_disable(id)`）传进来的往往是 id。故先按原样匹配 key，
    /// 再退化到按 `manifest.id` 匹配。`manifest.id` 理论可重复，只作兜底、不作身份。
    fn find_plugin_key(&self, path_or_id: &str) -> Option<String> {
        if self.plugins.contains_key(path_or_id) {
            return Some(path_or_id.to_string());
        }
        self.plugins
            .iter()
            .find(|(_, p)| p.manifest.id == path_or_id)
            .map(|(k, _)| k.clone())
    }

    /// 启用插件（含权限校验）
    pub fn enable_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self
            .find_plugin_key(path_or_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let (plugin_id, manifest) = {
            let plugin = self
                .plugins
                .get_mut(&key)
                .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
            if plugin.has_unauthorized_permissions {
                return Err(format!(
                    "插件 '{}' 包含未授权的权限，无法启用。请检查 manifest.json 中的 permissions 字段。",
                    plugin.manifest.name
                ));
            }
            plugin.enabled = true;
            (plugin.manifest.id.clone(), plugin.manifest.clone())
        };

        // 同步持久化的停用集合；仅在集合确有变化时落盘
        if self.disabled_ids.remove(&plugin_id) {
            self.persist_disabled_ids()?;
        }

        // 重新注册该插件贡献的节点类型：停用期间它们已被注销，启用后需恢复。
        self.register_plugin_node_types(&manifest);
        Ok(())
    }

    /// 禁用插件
    pub fn disable_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self
            .find_plugin_key(path_or_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let plugin_id = {
            let plugin = self
                .plugins
                .get_mut(&key)
                .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
            plugin.enabled = false;
            plugin.manifest.id.clone()
        };

        // 同步持久化的停用集合；仅在集合确有变化时落盘
        if self.disabled_ids.insert(plugin_id.clone()) {
            self.persist_disabled_ids()?;
        }

        // 注销该插件贡献的节点类型，避免停用后仍能从节点面板拖出。
        // 锁边界：此处仅持有 PluginHost 锁，注销回调内部去 lock NodeExecutor 的
        // registry（另一把独立锁）。全仓不存在「先持 registry 锁再取 PluginHost 锁」
        // 的路径，故锁序恒为 PluginHost → registry，不会死锁。
        self.unregister_plugin_node_types(&plugin_id);
        Ok(())
    }

    /// 获取单个插件详情
    #[allow(dead_code)]
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
        fs::create_dir_all(&self.plugins_dir).map_err(|e| format!("创建插件目录失败: {}", e))?;

        // 打开 zip 文件
        let file = fs::File::open(&zip_path).map_err(|e| format!("打开文件失败: {}", e))?;

        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("读取压缩包失败: {}", e))?;

        // 查找 manifest.json 确定插件 ID
        let mut manifest_content: Option<String> = None;
        let mut plugin_dir_name: Option<String> = None;

        // 第一遍扫描：找到 manifest.json
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("读取压缩包条目失败: {}", e))?;

            let entry_name = entry.name().to_string();

            // 支持两种结构: manifest.json (根目录) 或 plugin-name/manifest.json (有父目录)
            if entry_name == "manifest.json" || entry_name.ends_with("/manifest.json") {
                let mut content = String::new();
                use std::io::Read;
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
                manifest_content = Some(content);

                if entry_name == "manifest.json" {
                    plugin_dir_name = Some(".".to_string());
                } else {
                    plugin_dir_name =
                        Some(entry_name.trim_end_matches("/manifest.json").to_string());
                }
                break;
            }
        }

        let manifest_content =
            manifest_content.ok_or_else(|| "压缩包中未找到 manifest.json".to_string())?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_content)
            .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

        // 验证插件 ID。
        //
        // 这一步是**安装期**的兜底，与沙箱开关无关：id 会参与目标目录拼接
        // （`plugins/<id>/`），放行 ".." 或路径分隔符等于把落盘位置交给插件。
        // 报错时回显违规值（截断，避免恶意 manifest 用超长 id 撑爆提示），
        // 否则用户只看到"包含非法字符"却不知道是哪个字段、哪个字符违规。
        if manifest.id.contains("..") || manifest.id.contains('/') || manifest.id.contains('\\') {
            let shown: String = manifest.id.chars().take(80).collect();
            return Err(format!(
                "插件 ID 包含非法字符: {}（id 不允许包含“..”、正斜杠或反斜杠）",
                shown
            ));
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
            fs::remove_dir_all(&target_dir).map_err(|e| format!("清理旧目录失败: {}", e))?;
        }

        // 第二遍：解压文件
        let mut archive = zip::ZipArchive::new(
            fs::File::open(&zip_path).map_err(|e| format!("重新打开文件失败: {}", e))?,
        )
        .map_err(|e| format!("读取压缩包失败: {}", e))?;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
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
                fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
            }

            // 写入文件
            let mut output =
                fs::File::create(&target_path).map_err(|e| format!("创建文件失败: {}", e))?;
            use std::io::copy;
            copy(&mut entry, &mut output).map_err(|e| format!("写入文件失败: {}", e))?;
        }

        // 加载并验证安装的插件
        let manifest_path = target_dir.join("manifest.json");
        let instance = self.load_and_validate_plugin(&target_dir, &manifest_path)?;

        // 以 path 作 key（与 discover 一致）。若此处用 manifest.id，同一插件在
        // "zip 安装过、又被 discover 重建过"时会在表里留下 id / path 两个条目：
        // list 重复列出，且 disable/enable 只命中其中一条，表现为"停用了却还生效"。
        self.plugins.insert(instance.path.clone(), instance.clone());

        log::info!("[PluginInstall] 插件 '{}' 安装成功", manifest.name);

        Ok(instance)
    }

    /// 卸载插件
    pub fn uninstall_plugin(&mut self, path_or_id: &str) -> Result<(), String> {
        let key = self
            .find_plugin_key(path_or_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;
        let plugin = self
            .plugins
            .get(&key)
            .ok_or_else(|| format!("插件 '{}' 未找到", path_or_id))?;

        let plugin_path = PathBuf::from(&plugin.path);
        let plugin_id = plugin.manifest.id.clone();

        // 从 HashMap 移除
        self.plugins.remove(&key);

        // 注销该插件贡献的节点类型：移除实例后已无人能替它注销，必须在此处理。
        self.unregister_plugin_node_types(&plugin_id);

        // 同步清理停用记录，避免插件被重新安装时「继承」旧的停用状态
        if self.disabled_ids.remove(&plugin_id) {
            self.persist_disabled_ids()?;
        }

        // 删除目录
        if plugin_path.exists() {
            fs::remove_dir_all(&plugin_path).map_err(|e| format!("删除插件目录失败: {}", e))?;
        }

        log::info!("[PluginUninstall] 插件 '{}' 已卸载", path_or_id);
        Ok(())
    }
}

// ── Tauri Commands ──

#[tauri::command]
pub fn plugin_discover(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    executor: tauri::State<'_, Arc<crate::workflow::executor::NodeExecutor>>,
) -> Result<Vec<PluginInstance>, String> {
    let plugins = {
        let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        host.discover()
    }; // host 锁在此释放，避免 sync_plugin_node_types() 再次 lock 时死锁
       // discover 后同步插件节点类型到工作流注册表（共享 PluginHost，数据已最新）
    executor.sync_plugin_node_types();
    Ok(plugins)
}

#[tauri::command]
pub fn plugin_list(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
) -> Result<Vec<PluginInstance>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    Ok(host.list_plugins())
}

#[tauri::command]
pub fn plugin_enable(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    id: String,
) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.enable_plugin(&id)
}

#[tauri::command]
pub fn plugin_disable(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    id: String,
) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.disable_plugin(&id)
}

#[tauri::command]
pub fn plugin_get_sandbox_info(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
) -> Result<SandboxInfo, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    Ok(host.get_sandbox_info())
}

#[tauri::command]
pub fn plugin_set_sandbox_enabled(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    enabled: bool,
) -> Result<(), String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    host.set_sandbox_enabled(enabled);
    Ok(())
}

/// 插件 `data:invoke` 的唯一入口。
///
/// 双重校验：① 插件必须显式声明 `data:invoke` 权限；② 命令必须命中
/// [`PLUGIN_DATA_INVOKE_ALLOWLIST`]。插件 JS 不再能直接透传任意 Tauri 命令，
/// `get_api_key` / `write_text_file` / `plugin_install_zip` / `terminal_*` 等一律不可达。
#[tauri::command]
pub fn plugin_data_invoke(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    state: tauri::State<'_, crate::DbState>,
    plugin_id: String,
    command: String,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    {
        let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        host.require_permission(&plugin_id, "data:invoke")?;
    }

    if !PLUGIN_DATA_INVOKE_ALLOWLIST.contains(&command.as_str()) {
        return Err(format!(
            "命令 '{}' 不在插件可调用白名单内（data:invoke 仅放行: {}）",
            command,
            PLUGIN_DATA_INVOKE_ALLOWLIST.join(", ")
        ));
    }

    let args = args.unwrap_or(serde_json::Value::Null);
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;

    match command.as_str() {
        "list_api_providers" => {
            let list = crate::commands::api_provider::list_api_providers(&conn)
                .map_err(|e| format!("调用 {} 失败: {}", command, e))?;
            serde_json::to_value(list).map_err(|e| format!("序列化结果失败: {}", e))
        }
        "get_api_provider" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "get_api_provider 需要字符串参数 id".to_string())?;
            let provider = crate::commands::api_provider::get_api_provider(&conn, id)
                .map_err(|e| format!("调用 {} 失败: {}", command, e))?;
            serde_json::to_value(provider).map_err(|e| format!("序列化结果失败: {}", e))
        }
        // 白名单与转发分支必须同步维护：漏写分支即视为不可达
        _ => Err(format!("命令 '{}' 已在白名单中但未实现转发", command)),
    }
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
pub fn plugin_read_entry(
    host: tauri::State<'_, std::sync::Mutex<PluginHost>>,
    plugin_id: String,
) -> Result<String, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugins = host.list_plugins();
    let plugin = plugins
        .iter()
        .find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    let entry_path = std::path::Path::new(&plugin.path).join(&plugin.manifest.entry.main);
    let content =
        std::fs::read_to_string(&entry_path).map_err(|e| format!("读取入口文件失败: {}", e))?;

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
    let plugin = plugins
        .iter()
        .find(|p| p.manifest.id == plugin_id)
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
    let mut file =
        std::fs::File::open(&full_path).map_err(|e| format!("读取图标文件失败: {}", e))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取图标文件失败: {}", e))?;

    // 根据扩展名推断 MIME 类型
    let ext = full_path
        .extension()
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
    let plugin = plugins
        .iter()
        .find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    // 构建面板内容：显示插件基本信息
    let mut content = String::new();
    content.push_str(&format!("插件: {}\n", plugin.manifest.name));
    content.push_str(&format!("版本: {}\n", plugin.manifest.version));
    content.push_str(&format!("作者: {}\n", plugin.manifest.author));
    content.push_str(&format!("描述: {}\n", plugin.manifest.description));
    content.push_str(&format!("路径: {}\n", plugin.path));
    content.push_str(&format!(
        "沙箱: {}\n",
        if host.get_sandbox_info().sandbox_enabled {
            "已启用"
        } else {
            "已禁用"
        }
    ));
    content.push_str(&format!("面板ID: {}\n", panel_id));

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(id: &str, input: Option<serde_json::Value>) -> CommandContribution {
        CommandContribution {
            id: id.to_string(),
            title: id.to_string(),
            input_schema: input,
            output_schema: None,
        }
    }

    fn contributes(
        commands: Vec<CommandContribution>,
        workflow_config: Option<WorkflowConfigContribution>,
    ) -> PluginContributes {
        PluginContributes {
            panels: None,
            commands: Some(commands),
            hooks: None,
            node_types: None,
            workflow_config,
        }
    }

    fn validate(contributes: &PluginContributes) -> Vec<ManifestValidationError> {
        let mut errors = Vec::new();
        PluginHost::validate_workflow_contributions(contributes, &mut errors);
        errors
    }

    fn has_message(errors: &[ManifestValidationError], keyword: &str) -> bool {
        errors.iter().any(|error| error.message.contains(keyword))
    }

    #[test]
    fn accepts_command_level_components_and_valid_input_schema() {
        let commands = vec![
            command(
                "demo.add",
                Some(json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "number", "description": "加数" },
                        "mode": { "type": "string", "enum": ["fast", "exact"] }
                    },
                    "required": ["a"]
                })),
            ),
            // 省略 input.type 的写法也要接受（存量 manifest 只写了 properties）
            command(
                "demo.subtract",
                Some(json!({
                    "properties": { "a": { "type": "number" } },
                    "required": ["a"]
                })),
            ),
        ];
        let config = WorkflowConfigContribution {
            component: None,
            components: Some(HashMap::from([(
                "demo.add".to_string(),
                "AddNodeConfig".to_string(),
            )])),
        };

        assert!(validate(&contributes(commands, Some(config))).is_empty());
    }

    #[test]
    fn rejects_input_type_that_is_not_object() {
        let commands = vec![command("demo.add", Some(json!({ "type": "array" })))];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "input.type 必须是 \"object\""));
    }

    #[test]
    fn rejects_duplicate_command_ids() {
        let commands = vec![command("demo.add", None), command("demo.add", None)];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "命令 id 重复"));
    }

    #[test]
    fn rejects_workflow_config_without_any_component() {
        let config = WorkflowConfigContribution {
            component: None,
            components: None,
        };
        let errors = validate(&contributes(vec![command("demo.add", None)], Some(config)));

        assert!(has_message(&errors, "必须声明 component 或 components"));
    }

    #[test]
    fn rejects_component_for_undeclared_command() {
        let config = WorkflowConfigContribution {
            component: Some("DefaultNodeConfig".to_string()),
            components: Some(HashMap::from([(
                "demo.missing".to_string(),
                "MissingNodeConfig".to_string(),
            )])),
        };
        let errors = validate(&contributes(vec![command("demo.add", None)], Some(config)));

        assert!(has_message(
            &errors,
            "命令 id 未在 contributes.commands 中声明"
        ));
    }

    #[test]
    fn rejects_unknown_input_property_type() {
        let commands = vec![command(
            "demo.add",
            Some(json!({
                "type": "object",
                "properties": { "a": { "type": "decimal" } }
            })),
        )];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "不支持的类型 'decimal'"));
    }

    #[test]
    fn rejects_input_property_without_type() {
        let commands = vec![command(
            "demo.add",
            Some(json!({
                "type": "object",
                "properties": { "a": { "description": "缺 type" } }
            })),
        )];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "每个属性都必须声明 type"));
    }

    #[test]
    fn rejects_required_key_missing_from_properties() {
        let commands = vec![command(
            "demo.add",
            Some(json!({
                "type": "object",
                "properties": { "a": { "type": "number" } },
                "required": ["a", "b"]
            })),
        )];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "必填项 'b' 未在 properties 中声明"));
    }

    #[test]
    fn rejects_input_schema_that_is_not_an_object() {
        let commands = vec![command("demo.add", Some(json!("{ \"type\": \"object\" }")))];
        let errors = validate(&contributes(commands, None));

        assert!(has_message(&errors, "input 必须是 JSON 对象"));
    }
}
