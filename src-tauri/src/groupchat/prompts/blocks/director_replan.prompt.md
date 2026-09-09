{{FRAGMENT:identity.director}}用户对初始目标补充了新的指令，请对「当前任务清单」进行对账式重排。

{{FRAGMENT:rule.goal_anchor|始终不变，必须优先服务}}

{{SECTION:notes}}

【本轮新增指令（用户最新输入）】
{{SECTION:latest}}

参与者列表：
{{SECTION:roster}}

{{FRAGMENT:rule.catalog_heading}}
{{SECTION:catalog}}

{{FRAGMENT:rule.failures_heading}}
{{SECTION:failures}}

当前任务清单（id 必须原样保留；仅未执行任务可删减或重分配）：
{{SECTION:existing_tasks}}

{{FRAGMENT:rule.strict_json}}，格式：
{"operations":[{"action":"keep","task_id":"已存在任务id","reassign":"新负责人id或省略"},{"action":"remove","task_id":"已存在任务id","reason":"移除理由"},{"action":"add","description":"新任务描述","depends_on":["前置任务id"],"assignee":"负责人id"}],"roles":{"参与者id":"角色名"},"new_participants":[{"id":"新参与者唯一id","display_name":"显示名","system_role":"角色定位","participant_type":"api|cli","provider":"提供商id(api必填)","model":"模型名(api必填)","agent_type":"CLI Agent类型(cli必填)","reason":"补充原因(可选，说明为何需要该参与者)"}]}
new_participants 为可选字段（名册无需补充时省略或输出空数组）。

要求：
1. 状态为 success/failed/skipped 的任务一律 keep 且 reassign 留空，不得改动；
2. 状态为 discussing（尚未执行）的任务：若本轮指令使其不再必要，用 remove 并说明理由；若需更换负责人，用 keep 且填 reassign（reassign 目标不得是近期反复失败（失败记录 ≥ 2 次）的参与者）；
3. 状态为 pending（等待用户确认）的任务不要 remove，用 keep 保持；
4. 状态为 running（正在执行）的任务不要 remove；若需更换负责人，用 keep 且填 reassign（执行人变更在任务重跑后生效）；
5. 本轮指令若需要新的子任务，用 add 新增；depends_on 可引用「已有任务 id」或「本批 add 中排在本条之前的任务 id」（本批内按输出顺序，后发任务可依赖先发任务，反映真实流程）；禁止引用本批中排在本条之后的任务 id（防前向/环形依赖）；
6. 若本轮指令并未要求新增任务、且已有任务已覆盖目标，则不要输出 add 操作，operations 只保留必要的 keep；
7. 所有任务都必须服务于【初始目标】；
8. add 的任务遵循阶段B任务描述约束：
{{FRAGMENT:rule.task_description}}
{{FRAGMENT:rule.prior_product}}
{{FRAGMENT:rule.input_ref}}
9. 若本轮指令要求调整参与者角色，用 roles 指定（仅包含需要调整角色的参与者）；无角色调整需求时 roles 输出空对象 {}。已有参与者保持其根本角色定位，仅可调整职责边界/任务分工，禁止根本性变更（如 架构师→视觉工程师）；
10. 【用户最新指令优先级最高】用户最新指令（含补充/细化约束）必须被纳入本轮编排，不得因"锚定初始目标"而忽略或拒绝落实用户明确提出的新要求；若用户最新指令与初始目标冲突，以用户最新指令为准，并相应调整/重排/归档旧任务；
11. 【按需组建参与者】讨论中发现名册缺少完成当前目标/子任务所必需的角色时，用 new_participants 补充：
{{FRAGMENT:rule.slot_limit}}
{{FRAGMENT:rule.new_participant_spec}}
{{FRAGMENT:rule.minimal_roster}}。
