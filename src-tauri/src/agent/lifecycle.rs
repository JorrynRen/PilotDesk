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
//! `uvx` / `binary` 在 P1 返回「暂不支持」，由市场置灰（P2/P3 再补）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 生命周期类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleKind {
    /// npm 全局安装（P1 主力）
    Npm,
    /// uvx（P2）
    Uvx,
    /// 直接下载二进制（P3）
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

/// 生命周期规格。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleSpec {
    pub kind: LifecycleKind,
    #[serde(default)]
    pub packages: Vec<LifecyclePackage>,
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

/// 派生出的命令（安装/卸载/版本探测）。
#[derive(Debug, Clone, Default)]
pub struct LifecycleCommands {
    pub install: Option<String>,
    pub uninstall: Option<String>,
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

    /// P1 是否支持该 kind 的安装流程。
    pub fn supported(&self) -> Result<(), String> {
        match self.kind {
            LifecycleKind::Npm | LifecycleKind::Shell => Ok(()),
            LifecycleKind::Uvx => Err("uvx 安装将在后续版本支持".to_string()),
            LifecycleKind::Binary => Err("二进制下载将在后续版本支持".to_string()),
        }
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
                    uninstall: Some(
                        pkgs.iter()
                            .rev()
                            .map(|p| format!("npm uninstall -g {}", p))
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
                version_probe: self.version_cmd.clone().filter(|s| !s.trim().is_empty()),
                latest_version: self
                    .latest_version_cmd
                    .clone()
                    .filter(|s| !s.trim().is_empty()),
            }),
            LifecycleKind::Uvx | LifecycleKind::Binary => {
                self.supported()?;
                unreachable!("supported() 已拦住 P1 未支持的 kind")
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
    let re = regex::Regex::new(r"[vV]?(\d+\.\d+\.\d+[\w.\-]*)").ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
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
            version_cmd: Some("hermes --version".into()),
            ..Default::default()
        };
        let cmds = spec.commands().unwrap();
        assert_eq!(cmds.install.as_deref(), Some("pip install hermes-agent"));
        assert!(cmds.uninstall.is_none());
    }

    #[test]
    fn semver_extraction() {
        assert_eq!(extract_semver("hermes 1.4.2 (build 3)").as_deref(), Some("1.4.2"));
        assert_eq!(extract_semver("v0.16.0-beta.1").as_deref(), Some("0.16.0-beta.1"));
        assert!(extract_semver("no version here").is_none());
    }
}
