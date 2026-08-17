//! 群聊事件协议（前后端单通道 `groupchat-event`）。
//!
//! payload 统一形如 `{ roomId, type, ... }`，前端单 listener 按 roomId 过滤。

use serde::Serialize;

use super::models::{MessageRow, TaskRow};

/// 群聊事件。除 `roomId` 与 `type` 外，各 type 专属字段均用 Option 承载，
/// 序列化时跳过 None，保证每种事件只带必要字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupChatEvent {
    pub room_id: String,
    #[serde(rename = "type")]
    pub event_type: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<MessageRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskRow>,
}

impl GroupChatEvent {
    pub fn new(room_id: &str, event_type: &str) -> Self {
        Self {
            room_id: room_id.to_string(),
            event_type: event_type.to_string(),
            status: None,
            speaker: None,
            round: None,
            delta: None,
            message: None,
            participant_id: None,
            stance: None,
            role: None,
            task: None,
        }
    }

    pub fn started(room_id: &str) -> Self {
        Self::new(room_id, "started")
    }

    pub fn room_status(room_id: &str, status: &str) -> Self {
        let mut e = Self::new(room_id, "room_status");
        e.status = Some(status.to_string());
        e
    }

    pub fn floor_granted(room_id: &str, speaker: &str, round: i64) -> Self {
        let mut e = Self::new(room_id, "floor_granted");
        e.speaker = Some(speaker.to_string());
        e.round = Some(round);
        e
    }

    pub fn token_stream(room_id: &str, speaker: &str, delta: &str) -> Self {
        let mut e = Self::new(room_id, "token_stream");
        e.speaker = Some(speaker.to_string());
        e.delta = Some(delta.to_string());
        e
    }

    pub fn message(room_id: &str, message: MessageRow) -> Self {
        let mut e = Self::new(room_id, "message");
        e.message = Some(message);
        e
    }

    pub fn stance_updated(room_id: &str, participant_id: &str, stance: &str) -> Self {
        let mut e = Self::new(room_id, "stance_updated");
        e.participant_id = Some(participant_id.to_string());
        e.stance = Some(stance.to_string());
        e
    }

    pub fn role_updated(room_id: &str, participant_id: &str, role: &str) -> Self {
        let mut e = Self::new(room_id, "role_updated");
        e.participant_id = Some(participant_id.to_string());
        e.role = Some(role.to_string());
        e
    }

    pub fn task_updated(room_id: &str, task: TaskRow) -> Self {
        let mut e = Self::new(room_id, "task_updated");
        e.task = Some(task);
        e
    }

    pub fn finished(room_id: &str) -> Self {
        Self::new(room_id, "finished")
    }
}
