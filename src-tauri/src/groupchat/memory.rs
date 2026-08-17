//! 分层记忆：L1 摘要 / L2 立场 / L3 定向视图 / L4 最近窗口。

use std::collections::{HashMap, VecDeque};

use super::models::MessageRow;
use super::participant::{ChatMessage, Stance, TurnView};

/// 每参与者 L4 最近窗口条数。
const DEFAULT_WINDOW: usize = 10;

/// 判断某条消息是否对 `participant_id` 可见。
/// `[]`（广播）对所有参与者可见；定向消息仅对 recipients 中的参与者可见。
/// 特权参与者（Director / owner）绕过 recipients 过滤，看到全量。
fn message_visible(msg: &MessageRow, participant_id: &str, is_privileged: bool) -> bool {
    if is_privileged {
        return true;
    }
    let recipients: Vec<String> = serde_json::from_str(&msg.recipients).unwrap_or_default();
    recipients.is_empty() || recipients.iter().any(|r| r == participant_id)
}

pub struct LayeredMemory {
    topic: String,
    summary: String,
    stances: HashMap<String, String>,
    all_messages: Vec<MessageRow>,
    windows: HashMap<String, VecDeque<MessageRow>>,
    window_size: usize,
    names: HashMap<String, String>,
}

impl LayeredMemory {
    pub fn new(topic: &str, summary: &str) -> Self {
        Self {
            topic: topic.to_string(),
            summary: summary.to_string(),
            stances: HashMap::new(),
            all_messages: Vec::new(),
            windows: HashMap::new(),
            window_size: DEFAULT_WINDOW,
            names: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn set_summary(&mut self, summary: &str) {
        self.summary = summary.to_string();
    }

    /// 更新 participant_id -> display_name 映射，供视图组装时使用可读名称。
    pub fn set_names(&mut self, names: HashMap<String, String>) {
        self.names = names;
    }

    pub fn update_stance(&mut self, participant_id: &str, stance: &str) {
        self.stances.insert(participant_id.to_string(), stance.to_string());
    }

    pub fn stances(&self) -> Vec<Stance> {
        self.stances
            .iter()
            .map(|(k, v)| Stance { participant_id: k.clone(), stance: v.clone() })
            .collect()
    }

    /// 写入一条消息：追加到全量日志，并按可见性追加到各参与者 L4 窗口。
    pub fn apply_message(&mut self, msg: &MessageRow, participant_ids: &[String]) {
        self.all_messages.push(msg.clone());
        for pid in participant_ids {
            if !message_visible(msg, pid, false) {
                continue;
            }
            let win = self.windows.entry(pid.clone()).or_default();
            win.push_back(msg.clone());
            while win.len() > self.window_size {
                win.pop_front();
            }
        }
    }

    /// 组装某参与者的 L3 定向视图。
    pub fn build_view(
        &self,
        participant_id: &str,
        system_role: &str,
        is_privileged: bool,
        user_id: &str,
    ) -> TurnView {
        let mut messages: Vec<ChatMessage> = Vec::new();
        if is_privileged {
            for m in &self.all_messages {
                messages.push(row_to_chat(m, user_id, &self.names));
            }
        } else {
            if let Some(win) = self.windows.get(participant_id) {
                for m in win {
                    messages.push(row_to_chat(m, user_id, &self.names));
                }
            }
        }

        TurnView {
            topic: self.topic.clone(),
            summary: self.summary.clone(),
            stances: self.stances(),
            messages,
            system_role: system_role.to_string(),
            task_context: String::new(),
        }
    }

    /// 全量消息（Director 全知视图使用）。
    pub fn all_messages(&self) -> &[MessageRow] {
        &self.all_messages
    }
}

fn row_to_chat(m: &MessageRow, user_id: &str, names: &HashMap<String, String>) -> ChatMessage {
    let role = if m.sender == user_id { "user" } else { "assistant" };
    let name = names.get(&m.sender).cloned().unwrap_or_else(|| m.sender.clone());

    let mut content = m.content.clone();
    let mut images: Option<Vec<String>> = None;

    // 仅用户消息携带附件：图片转 base64 data URL 供多模态，同时在正文追加图片情况与路径说明
    // （使 CLI / 非多模态模型也能感知附件存在）；文件在正文追加路径说明。
    if role == "user" && m.attachments != "[]" {
        if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&m.attachments) {
            let mut imgs = Vec::new();
            let mut image_paths = Vec::new();
            let mut files = Vec::new();
            for it in items {
                let kind = it["kind"].as_str().unwrap_or("");
                let path = it["path"].as_str().unwrap_or("");
                let mime = it["mime"].as_str().unwrap_or("image/png");
                if kind == "image" && !path.is_empty() {
                    image_paths.push(path.to_string());
                    if let Ok(bytes) = std::fs::read(path) {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        imgs.push(format!("data:{};base64,{}", mime, b64));
                    }
                } else if kind == "file" && !path.is_empty() {
                    files.push(path.to_string());
                }
            }
            if !imgs.is_empty() {
                images = Some(imgs);
            }
            if !image_paths.is_empty() || !files.is_empty() {
                content.push_str(&format!(
                    "\n\n[附件清单] 用户提供了 图片 {} 张、文件 {} 个，路径如下：\n",
                    image_paths.len(),
                    files.len()
                ));
                for p in &image_paths {
                    content.push_str(&format!("- [图片] {}\n", p));
                }
                for f in &files {
                    content.push_str(&format!("- [文件] {}\n", f));
                }
                content.push_str("图片为二进制内容，请勿用 read_file 读取（若你的模型支持视觉可基于图片内容分析，否则仅知悉其存在）；文件可调用 read_file 工具按需读取。");
            }
        }
    }

    ChatMessage { role: role.to_string(), name: Some(name), content, images }
}
