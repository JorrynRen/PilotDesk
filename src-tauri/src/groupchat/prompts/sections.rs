//! 数据段契约（含四层记忆注入声明）。
//!
//! 四层记忆：L1 摘要（summary，Director LLM 压缩）/ L2 立场（stances）/
//! L3 定向视图（build_view：L1+L2+L4 组装）/ L4 最近窗口（windows，消息流）。
//!
//! 每段声明来源与用途，供"场景声明的输入段"校验（数据链完整性，防 v3.4au/av 类
//! "谁没收到什么"问题）与审查。

pub struct SectionSpec {
    pub id: &'static str,
    pub source: &'static str,
    pub usage: &'static str,
}

pub const SECTIONS: &[SectionSpec] = &[
    // ── 目标与约束 ──
    SectionSpec {
        id: "latest",
        source: "用户最新指令（拼接当前真实时钟）",
        usage: "目标整理/意图分类输入",
    },
    SectionSpec {
        id: "topic",
        source: "房间目标锚点（阶段A整理后落位）",
        usage: "各决策点的初始目标",
    },
    SectionSpec {
        id: "notes",
        source: "goal_notes 累积约束 + 用户输入物解析",
        usage: "补充/细化约束",
    },
    SectionSpec {
        id: "mention",
        source: "用户 @ 指定（下一发言者）",
        usage: "选人优先依据",
    },
    // ── 名册与参与者 ──
    SectionSpec {
        id: "roster",
        source: "参与者名册（id/显示名/角色）",
        usage: "角色/任务分配参考",
    },
    SectionSpec {
        id: "catalog",
        source: "可用模型 + CLI Agent 清单",
        usage: "按需组建参与者",
    },
    SectionSpec {
        id: "failures",
        source: "参与者失败记录",
        usage: "避免指派反复失败者",
    },
    SectionSpec {
        id: "previous_executors",
        source: "已失败执行者列表",
        usage: "失败处置禁止重派",
    },
    SectionSpec {
        id: "task_context",
        source: "本轮工作内容（讨论/执行指派）",
        usage: "参与者本轮职责",
    },
    // ── 四层记忆 ──
    SectionSpec {
        id: "summary",
        source: "L1 摘要（Director LLM 输出侧压缩）",
        usage: "全局进度",
    },
    SectionSpec {
        id: "stances",
        source: "L2 立场快照",
        usage: "收敛判断",
    },
    SectionSpec {
        id: "transcript",
        source: "L3/L4 消息转录（完整，无输入侧硬截断）",
        usage: "评估/结论上下文",
    },
    SectionSpec {
        id: "recent_speeches",
        source: "最近发言（完整，L4）",
        usage: "判断任务是否已实质推进",
    },
    SectionSpec {
        id: "clock",
        source: "当前真实时钟（ISO + UTC 偏移 + IANA 时区）",
        usage: "相对时间表达理解",
    },
    // ── 任务与成果 ──
    SectionSpec {
        id: "task_manifest",
        source: "任务清单（T1…编号/状态/负责人）",
        usage: "编排/评估输入",
    },
    SectionSpec {
        id: "existing_tasks",
        source: "现有任务清单（id/状态/描述/负责人）",
        usage: "对账重排输入",
    },
    SectionSpec {
        id: "prior_results",
        source: "前置任务成果（完整透传执行者输出）",
        usage: "任务延续性",
    },
    SectionSpec {
        id: "task_desc",
        source: "失败子任务描述",
        usage: "失败处置输入",
    },
    SectionSpec {
        id: "error",
        source: "失败原因",
        usage: "失败处置输入",
    },
    // ── 决策上下文 ──
    SectionSpec {
        id: "confirmation_prompt",
        source: "当前确认请求文本",
        usage: "确认回复语义判定",
    },
    SectionSpec {
        id: "reply",
        source: "用户对确认的回复",
        usage: "确认回复语义判定",
    },
    SectionSpec {
        id: "speech",
        source: "参与者单次发言（完整）",
        usage: "立场提取",
    },
    SectionSpec {
        id: "texts",
        source: "用户消息/目标/约束合并文本",
        usage: "输入物语义解析",
    },
    SectionSpec {
        id: "tool_name",
        source: "待裁决工具名",
        usage: "工具授权裁决",
    },
    SectionSpec {
        id: "risk",
        source: "工具风险等级",
        usage: "工具授权裁决",
    },
    SectionSpec {
        id: "args",
        source: "工具调用参数",
        usage: "工具授权裁决",
    },
    SectionSpec {
        id: "work_content",
        source: "主持人本轮指派内容",
        usage: "讨论轮约束注入",
    },
    // ── 参与者展示段 ──
    SectionSpec {
        id: "product_norm",
        source: "产物命名规范（按用户语言）",
        usage: "参与者产物规范",
    },
    SectionSpec {
        id: "output_dir",
        source: "房间统一产物目录",
        usage: "参与者产物落位",
    },
];
