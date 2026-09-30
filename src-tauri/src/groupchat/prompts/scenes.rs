//! 场景矩阵：所有 LLM 调用点（环节 × 输入数据段/记忆层 × 提示词块）。
//!
//! 每个场景声明"该调用点看到什么"，是数据链完整性（谁收到什么）的校验入口。
//! 四层记忆映射：L1=summary / L2=stances / L3=build_view(定向视图) / L4=windows(消息流)。

use super::SceneSpec;

/// 阶段A 目标确认：clarify_goal 调用。
pub const CLARIFY_GOAL: SceneSpec = SceneSpec {
    id: "scene.clarify_goal",
    flow: "flow.clarify",
    input_sections: &["latest", "topic", "notes", "roster"],
    blocks: &["director.clarify_goal"],
};

/// 阶段B 初始编排：kickoff 调用。
pub const KICKOFF: SceneSpec = SceneSpec {
    id: "scene.kickoff",
    flow: "flow.orchestrate",
    input_sections: &["topic", "notes", "latest", "roster", "catalog", "failures"],
    blocks: &["director.kickoff"],
};

/// 阶段B 追加重排：replan 调用。
pub const REPLAN: SceneSpec = SceneSpec {
    id: "scene.replan",
    flow: "flow.orchestrate",
    input_sections: &[
        "topic",
        "notes",
        "latest",
        "roster",
        "catalog",
        "failures",
        "existing_tasks",
    ],
    blocks: &["director.replan"],
};

/// 名册变更重排：replan_for_roster_change 调用。
pub const REPLAN_FOR_ROSTER_CHANGE: SceneSpec = SceneSpec {
    id: "scene.replan_for_roster_change",
    flow: "flow.orchestrate",
    input_sections: &["topic", "notes", "roster", "existing_tasks"],
    blocks: &["director.replan_for_roster_change"],
};

/// 阶段C 选发言者：next_speaker 调用（输入含 L1 摘要 + L2 立场 + L4 最近发言 + 补人清单）。
pub const NEXT_SPEAKER: SceneSpec = SceneSpec {
    id: "scene.next_speaker",
    flow: "flow.discuss",
    input_sections: &[
        "topic",
        "summary",
        "task_manifest",
        "roster",
        "mention",
        "stances",
        "latest",
        "notes",
        "failures",
        "recent_speeches",
        "catalog",
    ],
    blocks: &["director.next_speaker"],
};

/// 阶段D 评估：director_review 调用（更新 L1 摘要 + 收敛判定）。
pub const REVIEW: SceneSpec = SceneSpec {
    id: "scene.review",
    flow: "flow.review",
    input_sections: &[
        "topic",
        "notes",
        "latest",
        "task_manifest",
        "summary",
        "transcript",
        "stances",
    ],
    blocks: &["director.review"],
};

/// 阶段E 结论：conclude 调用。
pub const CONCLUDE: SceneSpec = SceneSpec {
    id: "scene.conclude",
    flow: "flow.conclude",
    input_sections: &["topic", "summary", "task_manifest", "transcript"],
    blocks: &["director.conclude"],
};

/// 接收用户消息：意图分类（classify_goal_intent）。
pub const CLASSIFY_GOAL_INTENT: SceneSpec = SceneSpec {
    id: "scene.classify_goal_intent",
    flow: "flow.receive",
    input_sections: &["topic", "notes", "roster", "latest"],
    blocks: &["director.classify_goal_intent"],
};

/// 接收用户消息：确认回复语义（classify_reply_intent）。
pub const CLASSIFY_REPLY_INTENT: SceneSpec = SceneSpec {
    id: "scene.classify_reply_intent",
    flow: "flow.receive",
    input_sections: &["confirmation_prompt", "reply"],
    blocks: &["director.classify_reply_intent"],
};

/// 阶段C 立场预处理（extract_stance）。
pub const EXTRACT_STANCE: SceneSpec = SceneSpec {
    id: "scene.extract_stance",
    flow: "flow.discuss",
    input_sections: &["speech"],
    blocks: &["director.extract_stance"],
};

/// 接收用户消息：输入物语义解析（parse_input_refs）。
pub const PARSE_INPUT_REFS: SceneSpec = SceneSpec {
    id: "scene.parse_input_refs",
    flow: "flow.receive",
    input_sections: &["texts"],
    blocks: &["director.parse_input_refs"],
};

/// 阶段C2 失败处置（handle_task_failure）。
pub const HANDLE_TASK_FAILURE: SceneSpec = SceneSpec {
    id: "scene.handle_task_failure",
    flow: "flow.execute",
    input_sections: &[
        "topic",
        "task_desc",
        "error",
        "previous_executors",
        "roster",
        "catalog",
    ],
    blocks: &["director.handle_task_failure"],
};

/// 阶段C2 工具授权裁决（authorize_tool）。
pub const AUTHORIZE_TOOL: SceneSpec = SceneSpec {
    id: "scene.authorize_tool",
    flow: "flow.execute",
    input_sections: &["topic", "tool_name", "risk", "args"],
    blocks: &["director.authorize_tool"],
};

/// 阶段C2 空转裁决（decide_stall_continue）。
pub const DECIDE_STALL_CONTINUE: SceneSpec = SceneSpec {
    id: "scene.decide_stall_continue",
    flow: "flow.execute",
    input_sections: &["topic", "summary"],
    blocks: &["director.decide_stall_continue"],
};

/// 阶段C1 参与者发言（API 型）：L3 定向视图（L1+L2+L4 窗口）+ task_context。
pub const PARTICIPANT_LLM: SceneSpec = SceneSpec {
    id: "scene.participant_llm",
    flow: "flow.discuss",
    input_sections: &[
        "topic",
        "roster",
        "summary",
        "stances",
        "task_context",
        "product_norm",
        "output_dir",
    ],
    blocks: &["participant"],
};

/// 阶段C1 参与者发言（CLI 型）：同一共用模板，仅工具纪律经变体注入不同。
pub const PARTICIPANT_CLI: SceneSpec = SceneSpec {
    id: "scene.participant_cli",
    flow: "flow.discuss",
    input_sections: &[
        "topic",
        "roster",
        "summary",
        "stances",
        "task_context",
        "product_norm",
        "output_dir",
    ],
    blocks: &["participant"],
};

/// 讨论轮强约束（注入参与者 task_context）。
pub const ROOM_DISCUSSION_TURN: SceneSpec = SceneSpec {
    id: "scene.room_discussion_turn",
    flow: "flow.discuss",
    input_sections: &["work_content"],
    blocks: &["room.discussion_turn"],
};
