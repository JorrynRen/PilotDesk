{{FRAGMENT:identity.director}}有参与者加入/离开了房间，请对「角色分配」和「任务清单」进行对账式重排。

{{FRAGMENT:rule.goal_anchor|始终不变，必须优先服务}}

{{SECTION:notes}}

【当前参与者名册（id、显示名、当前角色）】
{{SECTION:roster}}

【当前任务清单（id、状态、描述、负责人、结果/错误）】
{{SECTION:existing_tasks}}

{{FRAGMENT:rule.strict_json}}，格式：
{"roles":{"参与者id":"角色名"},"operations":[{"action":"keep","task_id":"已存在任务id","reassign":"新负责人id或省略"},{"action":"add","description":"新任务描述","depends_on":["前置任务id"],"assignee":"负责人id"}]}
要求：
1. 角色连续性：已有参与者保持其根本角色定位，仅可调整职责边界/任务分工，禁止根本性变更角色（如 架构师→视觉工程师）；新加入的参与者分配一个合适的角色；已离开的参与者不再出现在 roles 中；
2. 已完成任务不可动：状态为 success/failed/skipped 的任务一律 keep 且 reassign 留空，不得重派或重新分配；
3. 仅 discussing/pending/running（未完成/未执行）的任务可重派、拆分或新增；优先把其中适合的部分指派给新参与者，不重复已完成工作；
4. 已离开参与者名下未完成的任务需重派给仍在名册中的参与者；
5. 所有重排必须服务【初始目标】并满足补充约束；
6. roles 覆盖名册中每个仍在册的参与者。
