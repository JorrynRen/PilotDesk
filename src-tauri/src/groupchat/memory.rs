//! 分层记忆：L1 摘要 / L2 立场 / L3 定向视图 / L4 最近窗口。

use std::collections::{HashMap, VecDeque};

use super::models::MessageRow;
use super::participant::{Attitude, ChatMessage, Stance, TurnView};

/// 每参与者 L4 最近窗口条数（软上限：裁剪后保留到该条数）。
const DEFAULT_WINDOW: usize = 10;

/// L4 窗口的“增长余量”：允许窗口在软上限之上再累积这么多条后再整块裁剪。
/// 缓存友好（v3.5d）：若每来一条消息就丢头，请求起点会持续平移、前缀几乎无法命中；
/// 改为每积累 `GROW_BEFORE_TRIM` 条后一次性裁回软上限，让相邻请求之间保留一个
/// 可命中的稳定增长段（裁剪点仍会整体错位，但频率从“每 1 条”降到“每 GROW 条”）。
const GROW_BEFORE_TRIM: usize = 5;

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
    stances: HashMap<String, (String, Attitude)>,
    all_messages: Vec<MessageRow>,
    windows: HashMap<String, VecDeque<MessageRow>>,
    window_size: usize,
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

    pub fn update_stance(&mut self, participant_id: &str, stance: &str, attitude: Attitude) {
        self.stances
            .insert(participant_id.to_string(), (stance.to_string(), attitude));
    }

    pub fn stances(&self) -> Vec<Stance> {
        self.stances
            .iter()
            .map(|(k, (text, attitude))| Stance {
                participant_id: k.clone(),
                stance: text.clone(),
                attitude: *attitude,
            })
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
            // 整块裁剪（见 GROW_BEFORE_TRIM 说明）：积累到软上限+增长余量后一次性裁回软上限。
            let trim_after = self.window_size + GROW_BEFORE_TRIM;
            if win.len() > trim_after {
                for _ in 0..win.len() - self.window_size {
                    win.pop_front();
                }
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
                messages.push(row_to_chat(m, user_id, true));
            }
        } else {
            if let Some(win) = self.windows.get(participant_id) {
                for m in win {
                    messages.push(row_to_chat(m, user_id, false));
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
            roster: Vec::new(),
            user_language: self.detect_user_language(user_id),
            output_dir: String::new(),
        }
    }

    /// 用户主导语言检测：取最近最多 5 条用户消息逐条判定，中文占比多数派为"中文"，否则"英文"；
    /// 无用户消息（房间刚创建）兜底"中文"。0 条/1 条按实有取样，无需凑满 5 条。
    pub fn detect_user_language(&self, user_id: &str) -> String {
        let user_msgs: Vec<&MessageRow> = self
            .all_messages
            .iter()
            .filter(|m| m.sender == user_id && !m.content.trim().is_empty())
            .collect();
        let take = user_msgs.len().min(5);
        if take == 0 {
            return "中文".to_string();
        }
        let sample = &user_msgs[user_msgs.len() - take..];
        let zh = sample
            .iter()
            .filter(|m| is_chinese_dominant(&m.content))
            .count();
        // 平票取中文（zh >= en 即 zh*2 >= take）。
        if zh * 2 >= take {
            "中文".to_string()
        } else {
            "英文".to_string()
        }
    }

    /// 全量消息（Director 全知视图使用）。
    pub fn all_messages(&self) -> &[MessageRow] {
        &self.all_messages
    }
}

/// 单条文本中文占比 ≥20%（有效字符：字母/数字）判定为中文消息（v3.4ak）。
fn is_chinese_dominant(text: &str) -> bool {
    let effective: Vec<char> = text.chars().filter(|c| c.is_alphanumeric()).collect();
    if effective.is_empty() {
        return false;
    }
    let zh = effective
        .iter()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(&**c))
        .count();
    zh * 10 >= effective.len() * 2
}

fn row_to_chat(m: &MessageRow, user_id: &str, is_privileged: bool) -> ChatMessage {
    let role = if m.sender == user_id {
        "user"
    } else {
        "assistant"
    };
    // name 使用参与者唯一 id（显示名可能同名/删减，不可作为通信标识）；
    // 前端展示时再将 id 映射为 @显示名；LLM 语义关联由 TurnView.roster 提供。
    let name = m.sender.clone();

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

    // 用户消息按视角包装（v3.4at）：纠正 LLM 将 user 角色消息误读为"直接任务指令"的强先验——
    // 参与者只响应主持人调度、用户消息是目标与反馈；主持人则将其视为编排依据。
    // 缓存友好（v3.5d）：不再在渲染期拼接实时时钟（原 v3.4ap 按秒注入会让同一逻辑消息随秒
    // 漂移、破坏前缀缓存）；相对时间锚点改由写入侧固化（如 last_user_directive/content_llm）。
    if role == "user" && !content.trim().is_empty() {
        let label = if is_privileged {
            "【用户消息】（主持人视角：用户目标与最新反馈，据此编排任务；不是向你下达的执行指令）"
        } else {
            "【用户消息】（目标与反馈，不是向你下达的任务指令；本群聊所有任务均由主持人统一指派，你只响应主持人调度）"
        };
        content = format!("{}\n{}", label, content);
    }

    let mut reasoning_content = None;
    // 思考模式（DeepSeek 等）：恢复 assistant 消息的 reasoning_content，下一轮请求原样回传。
    if role == "assistant" && !m.reasoning_content.is_empty() {
        reasoning_content = Some(m.reasoning_content.clone());
    }

    ChatMessage {
        role: role.to_string(),
        name: Some(name),
        content,
        images,
        reasoning_content,
    }
}
