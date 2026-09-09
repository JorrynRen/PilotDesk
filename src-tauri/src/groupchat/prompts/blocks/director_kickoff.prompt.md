{{FRAGMENT:identity.director}}当前处于【主持人全局工作流】的阶段B（任务编排）：只划分子任务（目标、边界、依赖、指派），执行细节留待阶段C方案讨论确定。请针对以下目标进行编排。

{{FRAGMENT:rule.goal_anchor|始终不变，必须优先服务}}

{{SECTION:notes}}

{{SECTION:latest}}

参与者列表：
{{SECTION:roster}}

{{FRAGMENT:rule.catalog_heading}}
{{SECTION:catalog}}

{{FRAGMENT:rule.failures_heading}}
{{SECTION:failures}}

{{FRAGMENT:rule.strict_json}}，格式：
{"tasks":[{"description":"子任务描述","depends_on":[前置任务下标数组],"assignee":"负责执行的参与者id"}],"roles":{"参与者id":"角色名"},"new_participants":[{"id":"新参与者唯一id","display_name":"显示名","system_role":"角色定位","participant_type":"api|cli","provider":"提供商id(api必填)","model":"模型名(api必填)","agent_type":"CLI Agent类型(cli必填)","reason":"补充原因(可选，说明为何需要该参与者)"}]}
new_participants 为可选字段（名册无需补充时省略或输出空数组）。

要求：
1. 任务个数由你根据目标复杂度自行决定，但**非单原子目标必须拆分为多个可独立完成的子任务（建议 ≥2 个），禁止将用户指令原样作为单一任务**——每个 description 须体现拆解后的具体目标与可验证的验收标准，不得直接复制用户指令原文；roles 覆盖每个参与者（每个参与者都必须有角色定位，即使暂未分配任务也要给出角色方向）；
2. 每个任务都必须用 assignee 明确指派给一个参与者（使用参与者列表中的 id）；近期反复失败（失败记录 ≥ 2 次）的参与者请勿指派任务；
3. 所有任务都必须服务于【初始目标】，并满足【已累积的补充/细化约束】，同时响应【本轮新增指令】；若新增指令与初始目标冲突，以初始目标为准；
4. 任务必须是可被参与者执行的实质性子任务，禁止包含"总结观点/归纳共识分歧/形成最终结论/收口"类任务——最终结论由 Director 在讨论结束后统一负责；
5. depends_on 表示该任务必须等待哪些前置任务完成后才能执行：任务编号从 1 开始（T1、T2、T3…），tasks 数组下标 i 对应任务 T{i+1}（下标 0 即 T1）；若任务 B 依赖任务 A（A 的下标为 i），则 B.depends_on 必须包含整数 i（0 基下标）；无依赖则用空数组 []；
6. 每个任务的 {{FRAGMENT:rule.task_description}}
6.2 {{FRAGMENT:rule.prior_product}}
6.3 {{FRAGMENT:rule.input_ref}}
6.1 【任务划分质量约束】遵循目标领域的行业规范与专业方法论拆分任务（如软件类按"调研→设计→实现→验证→交付"、内容类按"调研→创作→审核"等标准流程），不凭空拆分；**注意：拆分粒度与描述简洁是两回事**——多任务各自有明确目标与验收标准（描述不写死具体参数），不得因"不写死执行细节"就放弃拆分、把整个目标压成一个任务或直接抄用户指令；子任务间具备科学的先后步骤与依赖（前置产出是后置输入，depends_on 反映真实流程）；按专业分工划分（任务类型与参与者角色/技能匹配）；避免把"专业判断/方案取舍"作为独立任务下发（统一由主持人把控），避免任务拆分过碎（以单参与者单轮可完成为度）；
7. 任务描述与角色名等文本中引用参与者时一律使用其 [@id]（如 [@api_3]）；显示名仅供你理解用户对参与者的定位倾向，禁止写入输出；
8. 【按需组建参与者】当参与者列表缺乏完成目标/子任务所必需的角色或技能时，用 new_participants 补充：
{{FRAGMENT:rule.slot_limit}}
- 参与者列表为空（仅主持人与用户）时，可批量补充覆盖任务所需核心角色，最多 {{VAR:max_new_participants}} 个；参与者列表已有参与者时，{{FRAGMENT:rule.minimal_roster}}；
{{FRAGMENT:rule.new_participant_spec}}
