//! Agent 生命周期规格（安装 / 更新 / 卸载 / 版本探测 / 运行时启动）。
//!
//! 取代原先散落在 `agents` 表上的扁平命令列：一张配置只描述「装什么、怎么跑」，
//! 具体命令按 `kind` 派生，避免同一个包名在安装/卸载/更新/版本四处各写一遍、漂移。
//!
//! 结构（存 `agents.lifecycle_json`）：
//! ```json
//! {
//!   "kind": "npm",
//!   "packages": [
//!     { "role": "adapter", "package": "@agentclientprotocol/claude-agent-acp", "args": ["--acp"] }
//!   ],
//!   "runtime": { "command": "npx", "args": ["-y", "@agentclientprotocol/claude-agent-acp"] }
//! }
//! ```
//!
//! P1 支持 `npm` 与 `shell`（`shell` 用于携带显式命令的历史预置，如 hermes 的 pip 形态）；
//! P2（`uvx`）**确定不做**（官方目录仅 2 条命中，且 uvx 无「安装态」语义）；
//! P3（`binary`）支持受限子集：当前平台 + `.zip` + 带 sha256 + 启动器不是 `.cmd` 壳。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 生命周期类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleKind {
    /// npm 全局安装（P1 主力）
    Npm,
    /// uvx（**不做**：保留取值以兼容存量 JSON，安装一律拒绝）
    Uvx,
    /// 直接下载二进制（P3：受限子集）
    Binary,
    /// 显式命令（安装/卸载/更新/版本探测由规格直接给出）
    Shell,
}

impl LifecycleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleKind::Npm => "npm",
            LifecycleKind::Uvx => "uvx",
            LifecycleKind::Binary => "binary",
            LifecycleKind::Shell => "shell",
        }
    }
}

/// 包角色：adapter 指 ACP 适配器（真正面向协议），host 指被适配的宿主 CLI/Agent。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageRole {
    Adapter,
    Host,
}

impl Default for PackageRole {
    fn default() -> Self {
        PackageRole::Host
    }
}

/// 一个需要安装的包。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecyclePackage {
    #[serde(default)]
    pub role: PackageRole,
    /// npm/pip 包名（可带版本后缀，如 `pkg@1.2.3`）
    pub package: String,
    /// 可选：该包提供的可执行文件名（官方目录不提供，安装后由 `npm view bin` 解析）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    /// 启动该包时的固定参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 启动时需要注入的环境变量
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// 运行时启动规格（ACP 会话真正 spawn 的命令）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// 平台制品（仅 `binary` 用）：官方目录里按平台给出的下载归档。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// 平台键，与官方目录一致：`windows-x86_64` / `darwin-aarch64` / `linux-x86_64` …
    pub platform: String,
    /// 归档下载地址
    pub archive: String,
    /// 归档内可执行文件的相对路径（可能带 `./` 或子目录）
    pub cmd: String,
    /// sha256（十六进制）。缺失时不做自动安装——无法校验完整性
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// 当前系统的平台键（与官方目录的取值同源）。
pub fn current_platform_key() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "unknown"
    };
    format!("{}-{}", os, arch)
}

/// 二进制安装根目录：`<app_root>/agents/<agent_type>`。
///
/// 版本子目录在其下（便于更新时整体切换）；卸载直接删整棵，避免依赖 pkg_state 是否完整。
pub fn binary_install_root(agent_type: &str) -> std::path::PathBuf {
    crate::utils::paths::app_root_dir()
        .join("agents")
        .join(agent_type)
}

/// 生命周期规格。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleSpec {
    pub kind: LifecycleKind,
    #[serde(default)]
    pub packages: Vec<LifecyclePackage>,
    /// 平台制品（仅 binary 用；按 `platform` 取当前系统那一份）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// 显式运行时（市场预置可给出；npm 条目通常留空，安装后由 bin 解析决定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeSpec>,
    // ── shell 专用：显式命令 ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstall_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version_cmd: Option<String>,
}

impl Default for LifecycleKind {
    fn default() -> Self {
        LifecycleKind::Npm
    }
}

/// 派生出的命令（安装/卸载/更新/版本探测）。
#[derive(Debug, Clone, Default)]
pub struct LifecycleCommands {
    pub install: Option<String>,
    pub uninstall: Option<String>,
    /// 已有安装时的升级命令。npm 不单列（装带版本号的包本身就是升级），
    /// shell 类型必须用它——`pip install <pkg>` 对已装版本是 no-op，升级得靠 `update_cmd`。
    pub update: Option<String>,
    pub version_probe: Option<String>,
    pub latest_version: Option<String>,
}

/// 包管理状态（存 `agents.pkg_state_json`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PkgState {
    /// pending_install / ready / failed
    pub status: String,
    /// 包名 → 可执行文件名（安装后由 `npm view <pkg> bin --json` 解析）
    #[serde(default)]
    pub bins: BTreeMap<String, String>,
    /// 二进制安装目录（仅 `binary`：卸载时直接删这棵目录）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
    /// 完整性来源：`sha256`（registry）/ `sha256-file` / `checksums-txt` / `none`（用户确认后不校验）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub updated_at: i64,
}

/// 状态常量（避免字符串散落）
pub const STATUS_PENDING_INSTALL: &str = "pending_install";
pub const STATUS_READY: &str = "ready";
pub const STATUS_FAILED: &str = "failed";

impl PkgState {
    pub fn pending(now_ts: i64) -> Self {
        Self {
            status: STATUS_PENDING_INSTALL.to_string(),
            bins: BTreeMap::new(),
            install_dir: None,
            integrity: None,
            installed_version: None,
            last_error: None,
            updated_at: now_ts,
        }
    }
    pub fn ready(now_ts: i64) -> Self {
        Self {
            status: STATUS_READY.to_string(),
            ..Self::pending(now_ts)
        }
    }
    pub fn failed(err: String, now_ts: i64) -> Self {
        Self {
            status: STATUS_FAILED.to_string(),
            last_error: Some(err),
            ..Self::pending(now_ts)
        }
    }
    pub fn is_ready(&self) -> bool {
        self.status == STATUS_READY
    }
}

/// 配置来源（存 `agents.dist_source_json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistSource {
    /// acp-registry / preset / manual
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub installed_at: i64,
}

pub const SOURCE_ACP_REGISTRY: &str = "acp-registry";
pub const SOURCE_PRESET: &str = "preset";

/// 去掉包名上的版本后缀：`@scope/name@1.2.3` → `@scope/name`，`pkg@latest` → `pkg`。
pub fn package_without_version(package: &str) -> String {
    // 作用域名以 `@` 开头，只从第 2 个字符起找版本分隔用的 `@`
    match package.rfind('@') {
        Some(idx) if idx > 0 => package[..idx].to_string(),
        _ => package.to_string(),
    }
}

impl LifecycleSpec {
    /// 主包（adapter 优先，否则第一个）。
    pub fn primary_package(&self) -> Option<&LifecyclePackage> {
        self.packages
            .iter()
            .find(|p| p.role == PackageRole::Adapter)
            .or_else(|| self.packages.first())
    }

    /// 规格里**钉死**的版本号（npm 包名带 `@x.y.z` 时取它）。
    ///
    /// 用于安装后核对「是否真的装上了目标版本」：`npm install` 对不可达/不存在的
    /// 目标版本可能以 0 退出却什么也不改。`@latest` / 无版本后缀返回 `None`，
    /// 此时不做版本比对（预置条目的 `version` 是配置版本号，不可比）。
    pub fn pinned_version(&self) -> Option<String> {
        let pkg = &self.primary_package()?.package;
        let rest = pkg.get(package_without_version(pkg).len()..)?;
        let v = rest.trim_start_matches('@').trim();
        if v.is_empty() || !v.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            return None;
        }
        Some(v.to_string())
    }

    /// P1 是否支持该 kind 的安装流程。
    ///
    /// `binary` 只支持受限子集（当前平台 + zip + 带 sha256 + 非 `.cmd` 启动器）：
    /// 这些条件缺一条，装下去就是「装完跑不起来」或「无法校验来源」，不如让市场明确置灰。
    pub fn supported(&self) -> Result<(), String> {
        match self.kind {
            LifecycleKind::Npm | LifecycleKind::Shell => Ok(()),
            LifecycleKind::Uvx => Err("uvx 分发当前不支持，请按该 Agent 官方文档手工安装".to_string()),
            LifecycleKind::Binary => match self.current_artifact() {
                Some(_) => Ok(()),
                None => Err(self.binary_block_reason()),
            },
        }
    }

    /// 当前系统可直接安装的制品（P3 受限子集的判定入口）。
    ///
    /// 只判「平台 + 归档形态 + 启动器形态」；**完整性不属于这里**——
    /// registry 没写 sha256 时还可以从厂商校验文件补，补不到则由用户显式确认后安装。
    pub fn current_artifact(&self) -> Option<&Artifact> {
        if self.kind != LifecycleKind::Binary {
            return None;
        }
        let key = current_platform_key();
        let artifact = self.artifacts.iter().find(|a| a.platform == key)?;
        if !artifact.archive.to_lowercase().ends_with(".zip") {
            return None;
        }
        if artifact.cmd.to_lowercase().ends_with(".cmd") {
            return None;
        }
        Some(artifact)
    }

    /// 二进制不可自动安装的具体原因（市场与安装报错共用，避免两处说法不一致）。
    pub fn binary_block_reason(&self) -> String {
        let key = current_platform_key();
        let Some(artifact) = self.artifacts.iter().find(|a| a.platform == key) else {
            return format!("该条目未提供当前系统（{}）的安装包，请按官方文档手工安装", key);
        };
        if !artifact.archive.to_lowercase().ends_with(".zip") {
            return "当前仅支持 .zip 安装包，请按官方文档手工安装".to_string();
        }
        if artifact.cmd.to_lowercase().ends_with(".cmd") {
            return "该安装包的启动器是 .cmd 脚本，当前不支持自动安装，请按官方文档手工安装".to_string();
        }
        "该安装包暂不支持自动安装，请按官方文档手工安装".to_string()
    }

    /// 派生四类命令。
    pub fn commands(&self) -> Result<LifecycleCommands, String> {
        match self.kind {
            LifecycleKind::Npm => {
                let pkgs: Vec<&str> = self
                    .packages
                    .iter()
                    .map(|p| p.package.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if pkgs.is_empty() {
                    return Err("生命周期规格缺少 npm 包名".to_string());
                }
                let join = |cmd: &dyn Fn(&str) -> String| -> String {
                    pkgs.iter()
                        .map(|p| cmd(p))
                        .collect::<Vec<_>>()
                        .join(" && ")
                };
                Ok(LifecycleCommands {
                    install: Some(join(&|p| format!("npm install -g {}", p))),
                    // 不带版本号的 npm 规格装的就是 latest，安装即升级，无需单列 update
                    update: None,
                    uninstall: Some(
                        pkgs.iter()
                            .rev()
                            // npm 的卸载不接受带版本号的 spec：`npm uninstall -g pkg@1.2.3`
                            // 会被当成 no-op（退出码仍为 0），包实际不会被移除。
                            .map(|p| format!("npm uninstall -g {}", package_without_version(p)))
                            .collect::<Vec<_>>()
                            .join(" && "),
                    ),
                    // 版本探测统一走全局清单，**不执行 `<bin> --version`**
                    // （部分适配器会进入 serving 状态卡住）。
                    version_probe: Some(format!(
                        "npm ls -g {} --json",
                        package_without_version(pkgs[0])
                    )),
                    latest_version: Some(format!(
                        "npm view {} version",
                        package_without_version(pkgs[0])
                    )),
                })
            }
            LifecycleKind::Shell => Ok(LifecycleCommands {
                install: self.install_cmd.clone().filter(|s| !s.trim().is_empty()),
                uninstall: self.uninstall_cmd.clone().filter(|s| !s.trim().is_empty()),
                update: self.update_cmd.clone().filter(|s| !s.trim().is_empty()),
                version_probe: self.version_cmd.clone().filter(|s| !s.trim().is_empty()),
                latest_version: self
                    .latest_version_cmd
                    .clone()
                    .filter(|s| !s.trim().is_empty()),
            }),
            // 二进制由安装器下载解压，不走 shell 命令；uvx 不支持
            // （两者都返回 Err，调用方应先按 kind 分派，别拿 commands() 当通用入口）
            LifecycleKind::Uvx | LifecycleKind::Binary => {
                Err("该分发方式不使用命令式安装".to_string())
            }
        }
    }

    /// 解析本机已安装版本：npm 从 `npm ls -g --json` 的输出里取，shell 走通用 semver 提取。
    pub fn parse_installed_version(&self, output: &str) -> Option<String> {
        match self.kind {
            LifecycleKind::Npm => {
                let pkg = self.primary_package()?;
                let name = package_without_version(&pkg.package);
                let json: serde_json::Value = serde_json::from_str(output).ok()?;
                json.get("dependencies")?
                    .get(&name)?
                    .get("version")?
                    .as_str()
                    .map(|s| s.to_string())
            }
            LifecycleKind::Shell => extract_semver(output),
            _ => None,
        }
    }

    /// bin 解析命令：`npm view <pkg> bin --json`（包已全局安装后再跑）。
    pub fn bin_probe_command(&self) -> Option<String> {
        if self.kind != LifecycleKind::Npm {
            return None;
        }
        let pkg = self.primary_package()?;
        Some(format!(
            "npm view {} bin --json",
            package_without_version(&pkg.package)
        ))
    }

    /// 合并所有包的 env（adapter 在前，宿主变量不覆盖适配器）。
    pub fn merged_env(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for p in &self.packages {
            for (k, v) in &p.env {
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        out
    }

    /// 已有 bin 解析结果时，得出 ACP 会话的启动命令。
    ///
    /// - 规格里显式给了 `runtime` → 直接用（hermes 这类宿主自带命令）；
    /// - npm：解析到 bin 就用 bin，否则退 `npm exec --no-install -- <pkg> <args>`。
    pub fn resolved_runtime(&self, bins: &BTreeMap<String, String>) -> Option<RuntimeSpec> {
        if let Some(rt) = &self.runtime {
            if !rt.command.trim().is_empty() {
                return Some(rt.clone());
            }
        }
        let pkg = self.primary_package()?;
        if self.kind != LifecycleKind::Npm {
            return None;
        }
        let name = package_without_version(&pkg.package);
        if let Some(bin) = bins.get(&name).or_else(|| bins.get(&pkg.package)) {
            if !bin.trim().is_empty() {
                return Some(RuntimeSpec {
                    command: bin.clone(),
                    args: pkg.args.clone(),
                });
            }
        }
        let mut args = vec![
            "exec".to_string(),
            "--no-install".to_string(),
            "--".to_string(),
            pkg.package.clone(),
        ];
        args.extend(pkg.args.clone());
        Some(RuntimeSpec {
            command: "npm".to_string(),
            args,
        })
    }
}

/// 从任意文本里提取第一个 semver（`0.16.0` / `v1.2.3-beta`）。
pub fn extract_semver(text: &str) -> Option<String> {
    // 先剔除括号内容：`Hermes Agent vunknown (2026.9.24)` 这类输出把**构建日期**
    // 放在括号里，不剔除就会把日期（2026.9.24）当成版本号显示。
    // 剔除后仍找不到版本就返回 None，由上层显示「版本未知」，不猜。
    let stripped = regex::Regex::new(r"\([^)]*\)")
        .ok()?
        .replace_all(text, " ")
        .to_string();
    let re = regex::Regex::new(r"[vV]?(\d+\.\d+\.\d+[\w.\-]*)").ok()?;
    re.captures(&stripped)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// 方向感知的版本比较：`candidate` 是否**严格新于** `base`。
///
/// 返回 `None` = 两者无法比较（空串、非纯数字段、预发布后缀等）。
/// 调用方据此**不提示更新**（宁可少报、不误报）：只比 `!=` 会在目录快照落后于
/// 包管理器最新版时永久误报「可更新」，点击后还可能把本机装回更旧的版本。
///
/// 与前端环境检测页的 `isOlder` 同口径：去 `v` 前缀、缺段补 0、逐段数值比较。
pub fn is_newer_version(candidate: &str, base: &str) -> Option<bool> {
    fn parse(v: &str) -> Option<Vec<u64>> {
        let v = v.trim().trim_start_matches(['v', 'V']);
        if v.is_empty() {
            return None;
        }
        v.split('.').map(|s| s.parse::<u64>().ok()).collect()
    }
    let a = parse(candidate)?;
    let b = parse(base)?;
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return Some(x > y);
        }
    }
    Some(false)
}

#[cfg(test)]
mod version_compare_tests {
    use super::is_newer_version;

    #[test]
    fn direction_aware_comparison() {
        assert_eq!(is_newer_version("2.1.0", "2.2.2"), Some(false));
        assert_eq!(is_newer_version("2.2.2", "2.1.0"), Some(true));
        assert_eq!(is_newer_version("2.2.2", "2.2.2"), Some(false));
        // v 前缀 / 缺段补 0
        assert_eq!(is_newer_version("v2.1", "2.0.9"), Some(true));
        assert_eq!(is_newer_version("1.0", "1.0.0"), Some(false));
        // 多段版本（构建号）
        assert_eq!(is_newer_version("1.2.3.4", "1.2.3"), Some(true));
    }

    #[test]
    fn unparsable_versions_are_not_comparable() {
        assert_eq!(is_newer_version("", "1.0.0"), None);
        assert_eq!(is_newer_version("latest", "1.0.0"), None);
        assert_eq!(is_newer_version("1.0.0-beta.1", "1.0.0"), None);
        assert_eq!(is_newer_version("1.0.0", "unknown"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npm_spec(role: PackageRole, package: &str, args: &[&str]) -> LifecyclePackage {
        LifecyclePackage {
            role,
            package: package.to_string(),
            bin: None,
            args: args.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn npm_commands_are_derived_from_packages() {
        let spec = LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![
                npm_spec(PackageRole::Adapter, "@agentclientprotocol/codex-acp", &["--acp"]),
                npm_spec(PackageRole::Host, "@openai/codex", &[]),
            ],
            ..Default::default()
        };
        let cmds = spec.commands().unwrap();
        assert_eq!(
            cmds.install.unwrap(),
            "npm install -g @agentclientprotocol/codex-acp && npm install -g @openai/codex"
        );
        assert_eq!(
            cmds.version_probe.as_deref(),
            Some("npm ls -g @agentclientprotocol/codex-acp --json")
        );
    }

    #[test]
    fn pinned_version_only_reads_explicit_numeric_suffix() {
        let spec = |pkg: &str| LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![npm_spec(PackageRole::Adapter, pkg, &[])],
            ..Default::default()
        };
        assert_eq!(
            spec("@agentclientprotocol/codex-acp@2.2.2").pinned_version().as_deref(),
            Some("2.2.2")
        );
        // 无版本后缀 / @latest 都不算钉死版本（预置条目不可比）
        assert_eq!(spec("@openai/codex").pinned_version(), None);
        assert_eq!(spec("pkg@latest").pinned_version(), None);
    }

    #[test]
    fn npm_uninstall_strips_pinned_version() {
        // 回归：`npm uninstall -g pkg@1.2.3` 是 no-op（退出码 0），必须去掉版本号
        let spec = LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![npm_spec(
                PackageRole::Adapter,
                "@agentclientprotocol/codex-acp@2.2.2",
                &[],
            )],
            ..Default::default()
        };
        assert_eq!(
            spec.commands().unwrap().uninstall.as_deref(),
            Some("npm uninstall -g @agentclientprotocol/codex-acp")
        );
    }

    #[test]
    fn package_without_version_handles_scope() {
        assert_eq!(
            package_without_version("@agentclientprotocol/claude-agent-acp@0.4.1"),
            "@agentclientprotocol/claude-agent-acp"
        );
        assert_eq!(package_without_version("agoragentic-mcp@1.3.0"), "agoragentic-mcp");
        assert_eq!(package_without_version("@openai/codex"), "@openai/codex");
    }

    #[test]
    fn npm_version_parsed_from_global_ls() {
        let spec = LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![npm_spec(
                PackageRole::Adapter,
                "@agentclientprotocol/codex-acp@1.2.3",
                &[],
            )],
            ..Default::default()
        };
        let out = r#"{"dependencies":{"@agentclientprotocol/codex-acp":{"version":"1.2.3"}}}"#;
        assert_eq!(spec.parse_installed_version(out).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn runtime_prefers_resolved_bin_and_falls_back_to_npm_exec() {
        let spec = LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![npm_spec(
                PackageRole::Adapter,
                "@agentclientprotocol/codex-acp",
                &["--acp"],
            )],
            ..Default::default()
        };
        let mut bins = BTreeMap::new();
        bins.insert(
            "@agentclientprotocol/codex-acp".to_string(),
            "codex-acp".to_string(),
        );
        let rt = spec.resolved_runtime(&bins).unwrap();
        assert_eq!(rt.command, "codex-acp");
        assert_eq!(rt.args, vec!["--acp".to_string()]);

        let rt2 = spec.resolved_runtime(&BTreeMap::new()).unwrap();
        assert_eq!(rt2.command, "npm");
        assert_eq!(
            rt2.args,
            vec![
                "exec".to_string(),
                "--no-install".to_string(),
                "--".to_string(),
                "@agentclientprotocol/codex-acp".to_string(),
                "--acp".to_string()
            ]
        );
    }

    #[test]
    fn uvx_and_binary_are_rejected_in_p1() {
        let uvx = LifecycleSpec {
            kind: LifecycleKind::Uvx,
            ..Default::default()
        };
        assert!(uvx.commands().is_err());
        let bin = LifecycleSpec {
            kind: LifecycleKind::Binary,
            ..Default::default()
        };
        assert!(bin.commands().is_err());
    }

    #[test]
    fn shell_kind_passes_through_explicit_commands() {
        let spec = LifecycleSpec {
            kind: LifecycleKind::Shell,
            install_cmd: Some("pip install hermes-agent".into()),
            update_cmd: Some("pip install --upgrade hermes-agent".into()),
            version_cmd: Some("hermes --version".into()),
            ..Default::default()
        };
        let cmds = spec.commands().unwrap();
        assert_eq!(cmds.install.as_deref(), Some("pip install hermes-agent"));
        // shell 的升级必须走 update_cmd：install_cmd 对已装版本是 no-op
        assert_eq!(
            cmds.update.as_deref(),
            Some("pip install --upgrade hermes-agent")
        );
        assert!(cmds.uninstall.is_none());
    }

    #[test]
    fn semver_extraction() {
        assert_eq!(extract_semver("hermes 1.4.2 (build 3)").as_deref(), Some("1.4.2"));
        assert_eq!(extract_semver("v0.16.0-beta.1").as_deref(), Some("0.16.0-beta.1"));
        assert!(extract_semver("no version here").is_none());
    }

    #[test]
    fn semver_extraction_ignores_parenthesized_build_date() {
        // 括号里是构建日期，真正的版本在前
        assert_eq!(
            extract_semver("Hermes Agent v0.19.0 (2026.7.20)").as_deref(),
            Some("0.19.0")
        );
        // 版本本身不可知时不能把日期当版本
        assert!(extract_semver("Hermes Agent vunknown (2026.9.24)").is_none());
    }
}
