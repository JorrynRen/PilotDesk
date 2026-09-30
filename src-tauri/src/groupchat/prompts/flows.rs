//! 业务环节清单（按业务流程组织，可读性主干）。
//! 每环节聚合：业务逻辑 + 场景清单 + 记忆层/数据段注入。

use super::FlowSpec;

/// 全部业务环节（按执行顺序）。
pub const FLOWS: &[FlowSpec] = &[
    FlowSpec {
        id: "flow.receive",
        name: "接收用户消息",
        scenes: &[
            "scene.classify_goal_intent",
            "scene.classify_reply_intent",
            "scene.parse_input_refs",
        ],
    },
    FlowSpec {
        id: "flow.clarify",
        name: "目标确认（阶段A）",
        scenes: &["scene.clarify_goal"],
    },
    FlowSpec {
        id: "flow.orchestrate",
        name: "任务编排（阶段B）",
        scenes: &[
            "scene.kickoff",
            "scene.replan",
            "scene.replan_for_roster_change",
        ],
    },
    FlowSpec {
        id: "flow.discuss",
        name: "方案讨论（阶段C1）",
        scenes: &[
            "scene.next_speaker",
            "scene.participant_llm",
            "scene.participant_cli",
            "scene.room_discussion_turn",
            "scene.extract_stance",
        ],
    },
    FlowSpec {
        id: "flow.execute",
        name: "执行（阶段C2）",
        scenes: &[
            "scene.handle_task_failure",
            "scene.authorize_tool",
            "scene.decide_stall_continue",
        ],
    },
    FlowSpec {
        id: "flow.review",
        name: "整体评估（阶段D）",
        scenes: &["scene.review"],
    },
    FlowSpec {
        id: "flow.conclude",
        name: "最终结论（阶段E）",
        scenes: &["scene.conclude"],
    },
];
