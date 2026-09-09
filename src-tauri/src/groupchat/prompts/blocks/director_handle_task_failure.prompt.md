{{FRAGMENT:identity.director}}执行者执行以下子任务失败，请给出处置决策。

{{FRAGMENT:rule.goal_anchor|所有处置必须服务于此目标，不可偏离}}
子任务：{{SECTION:task_desc}}
失败原因：{{SECTION:error}}
已尝试次数：{{VAR:attempt}}（已失败执行者：{{SECTION:previous_executors}}，不得再指派给他们）
可重新指派的参与者：
{{SECTION:roster}}

{{FRAGMENT:rule.catalog_heading}}
{{SECTION:catalog}}
【本场可补充空位】{{VAR:max_new_participants}} 名（0 表示禁止新增参与者）。

{{FRAGMENT:rule.strict_json}}，格式：
{"action":"retry|reassign|skip|abort|ask_user|self_execute|add_participant","assignee":"参与者id(仅 reassign 时必填)","reason":"决策理由","remove_participant":"被移除的僵尸参与者id(可选，可与其他 action 组合)","new_participant":{"id":"新参与者唯一id","display_name":"显示名","system_role":"角色定位","participant_type":"api|cli","provider":"提供商id(api必填)","model":"模型名(api必填)","agent_type":"CLI Agent类型(cli必填)","reason":"补充原因(可选)"}(仅 add_participant 时填写),"confirmation":{...}}
要求（按优先级判断）：
1. 仅当【首次失败且错误明确可修复】（如语法错误、路径缺失、格式问题）时才可 action=retry；只要已有重试或换人记录（attempt > 1），一律不得再 retry；
2. 若换人可继续推进，action=reassign，assignee 必须存在于名册、不得是已失败执行者，并按任务类型与参与者角色匹配选择；assignee 填名册中的裸 id（不带 [@] 括号），必须逐字精确；
3. 若该任务已无必要或无法完成，action=skip；
4. 若失败严重到应终止整个执行流程，action=abort；
5. 若任务关键决策需要用户（人类）拍板才能继续（例如不可逆操作、关键信息缺失需用户补充、方案取舍），action=ask_user，并通过 confirmation 字段发起确认请求；
6. 若反复失败、换人/重试均不现实，但任务对初始目标仍必要，action=self_execute（由主持人亲自执行）；
7. 【补充参与者接盘】当任务反复失败（attempt ≥ 2，如 API 调用失败、模型不支持所需能力、完成质量低等），且现有参与者均不具备完成任务所需的能力/模型，而本场可补充空位 > 0 时，action=add_participant：用 new_participant 指定一名具备所需能力的新参与者接盘该任务（创建后任务将重派给它）；{{FRAGMENT:rule.new_participant_spec}}；空位为 0 或失败原因与能力无关时不得使用该 action；
8. 【移除僵尸参与者】当某参与者反复失败（在当前任务已失败执行者列表中，或连续多次失败）、且失败性质表明其能力/模型持续不可用（如 API 调用失败、模型不支持、完成质量低），可带 remove_participant 移除它释放席位：移除对象必须是已失败执行者（填其裸 id，如 api_2），禁止移除主持人与用户；remove_participant 可与其他 action 组合（如 action=add_participant 且 remove_participant=当前失败者，表示"替换"；或 action=reassign 且 remove_participant=已失败者）；移除后必须保证任务仍有可继续执行的参与者（reassign/add_participant/self_execute 的落点不得是被移除者）；仅当确实要移除时才填写该字段，否则省略；
9. 无论选择何种 action，处置与确认问题都必须始终围绕上述【初始目标】。

confirmation 字段格式：
- 开放式回复（reply_mode=open，items 为空数组）：{"title":"不超过22字的确认事项摘要","reply_mode":"open","prompt":"请说明你的决定或补充信息","items":[]}
- 结构化确认列表（reply_mode=structured）：{"title":"不超过22字的确认事项摘要","reply_mode":"structured","prompt":"请确认以下事项","items":[{"id":"q1","label":"问题标题","input_type":"confirm","options":[],"required":true},{"id":"q2","label":"问题标题","input_type":"select","options":["选项A","选项B"],"required":true},{"id":"q3","label":"问题标题","input_type":"text","required":false,"placeholder":"请输入"}]}
input_type 可选：confirm（确认，固定为 是/否）、select（单选，需 options）、text（文本填写）。
