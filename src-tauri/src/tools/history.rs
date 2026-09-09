//! 文件写入副作用服务：历史快照 + diff 事件。
//!
//! 统一经 `ToolEnv` 注入（替代原 `WriteFileHooks` 工具私有构造通道），
//! 供 write_file / edit_file 共享；会话模式与群聊模式装配处各自组装并注入。
//!
//! `scope` 为归属标识（目录命名空间）：会话模式传 session_id，群聊模式传 room_id
//! （与附件落盘目录 `<工作目录>/attachments/<scope>` 的语义一致）。

use crate::db::init::DbPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::Emitter;

#[derive(Clone)]
pub struct FileHistoryService {
    pub app: tauri::AppHandle,
    /// 归属标识：session_id / room_id（历史记录落库与 diff 事件携带该值）。
    pub scope: String,
    pub pool: DbPool,
    /// 运行期开关：false 时跳过记录（群聊场景热切换用；会话模式恒 true）。
    pub enabled: Arc<AtomicBool>,
}

/// 群聊场景文件历史共享开关：所有群聊服务引用同一 AtomicBool，
/// 群聊页开关热切换后立即对所有运行中的房间生效（无需重建参与者工具集）。
static GROUPCHAT_FILE_HISTORY_ENABLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// 获取群聊共享开关的 Arc（惰性初始化，默认 true）。
pub fn groupchat_file_history_flag() -> Arc<AtomicBool> {
    GROUPCHAT_FILE_HISTORY_ENABLED
        .get_or_init(|| Arc::new(AtomicBool::new(true)))
        .clone()
}

/// 热切换群聊文件历史（调用方需同步持久化 tool_overrides，保持数据一致）。
pub fn set_groupchat_file_history_enabled(v: bool) {
    groupchat_file_history_flag().store(v, Ordering::Relaxed);
}

impl FileHistoryService {
    /// 记录修改前快照（覆盖旧内容超限时自动跳过；`old_content == new_content` 时不记录）。
    pub fn record(&self, file_path: &str, old_content: &str, new_content: &str, existed: bool) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if old_content == new_content {
            return;
        }
        crate::commands::file_history::record_file_change(
            &self.pool,
            &self.scope,
            file_path,
            old_content,
            existed,
        );
        // 通知前端：群聊右侧面板文件历史徽标实时刷新（scope=room_id，前端按 roomId 过滤）；
        // 会话模式无监听方，事件被忽略，无副作用。
        let _ = self.app.emit(
            "file-history-updated",
            serde_json::json!({ "roomId": self.scope }),
        );
    }

    /// 发送文件 diff 事件（会话模式前端文件差异面板监听；群聊前端当前忽略）。
    pub fn emit_diff(&self, file_path: &str, old_content: &str, new_content: &str) {
        let diff = crate::compute_diff(old_content, new_content);
        if diff.is_empty() {
            return;
        }
        let _ = self.app.emit(
            "agent-file-diff",
            serde_json::json!({
                "sessionId": self.scope,
                "path": file_path,
                "diff": diff,
            }),
        );
    }
}
