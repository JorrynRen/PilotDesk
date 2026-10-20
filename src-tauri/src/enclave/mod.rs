#![allow(dead_code)]
//! 飞地（Enclave）领域层：内网小团队（微型组织）协作工作台。
//!
//! 与 `groupchat/` 同构分工：**领域逻辑全在本模块，命令层（`commands/enclave*.rs`）只做薄适配**。
//!
//! 架构约束（详见设计文档《飞地（Enclave）产品设计与落地方案》）：
//! - **`owner = host`**：平台组织 owner 即主机，其设备承载团队数据（`enclave.db`）。
//! - **两套权威**：组织/成员/角色/席位**以平台为准**（本模块只存只读快照，本地不写）；
//!   任务/汇报/产出/文件**以主机为准**（本模块写入 `enclave.db`）。
//! - **不缓存**：成员端不缓存团队数据（离线不可用），因此没有 `enclave_cache.db`。

pub mod authz;
pub mod backup;
pub mod db;
pub mod files;
pub mod gate;
pub mod host;
pub mod knowledge;
pub mod notify;
pub mod org;
pub mod progress;
pub mod reports;
pub mod rpc;
pub mod snapshot;
pub mod tasks;
pub mod tls;
pub mod tools;
pub mod transfer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use db::EnclaveStore;

/// 主机服务默认监听端口（`enclave_settings.enclave_host_port` 或环境变量可覆盖）。
pub const ENCLAVE_DEFAULT_PORT: u16 = 47820;

/// 治理快照的新鲜度上限（秒）：超过即视为"可能过期"。
/// 治理类操作（成员/席位/策略/转移）前强刷；团队**业务**读写不受此约束。
pub const SNAPSHOT_STALE_MAX_SECS: i64 = 600;

/// 治理快照后台轮询的默认间隔（秒，30 分钟）。
pub const SNAPSHOT_DEFAULT_INTERVAL_SECS: i64 = 30 * 60;

/// 飞地模块的托管状态（`tauri::Builder::manage`）。
///
/// 只持有**共享引用 + 运行态标志**：真实数据在 `EnclaveStore`（`Arc` 共享给
/// 后台任务与命令层），状态本身不复制连接。
pub struct EnclaveState {
    /// `enclave.db` 连接层（团队业务数据的唯一权威源）。
    pub store: Arc<EnclaveStore>,
    /// 应急开关运行态：`true` = 飞地对外服务已临时关闭（本机使用不受影响）。
    pub disabled: AtomicBool,
    /// 最近一次治理快照成功同步时间（unix 秒，0 = 从未成功）。
    pub last_snapshot_at: AtomicI64,
    /// 主机服务运行态（M7）。
    pub host: host::HostRuntime,
    /// 成员端发现到的主机（内存缓存，按 org_id；不落库）。
    pub discovered: Mutex<HashMap<String, host::DiscoveredHost>>,
}

impl EnclaveState {
    pub fn new(store: Arc<EnclaveStore>) -> Self {
        Self {
            store,
            disabled: AtomicBool::new(false),
            last_snapshot_at: AtomicI64::new(0),
            host: host::HostRuntime::default(),
            discovered: Mutex::new(HashMap::new()),
        }
    }

    /// 最近一次治理快照成功同步时间（unix 秒，0 = 从未成功）。供命令层读新鲜度用。
    pub fn last_snapshot_at(&self) -> i64 {
        self.last_snapshot_at.load(Ordering::Relaxed)
    }

    /// 设置应急开关运行态（`true` = 飞地对外服务已临时关闭）。
    pub fn set_disabled(&self, disabled: bool) {
        self.disabled.store(disabled, Ordering::Relaxed);
    }

    /// 当前是否处于「飞地对外服务已临时关闭」（本机使用不受影响）。
    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// 取当前组织发现到的主机（顺带清理过期条目）。
    pub fn discovered_host(&self, org_id: &str) -> Option<host::DiscoveredHost> {
        let now = chrono::Utc::now().timestamp();
        let mut map = self.discovered.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, d| now.saturating_sub(d.seen_at) <= host::DISCOVERY_TTL_SECS);
        map.get(org_id).cloned()
    }
}
