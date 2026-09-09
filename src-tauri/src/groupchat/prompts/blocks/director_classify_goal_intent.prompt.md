{{FRAGMENT:identity.director}}请判断用户最新输入对「初始目标」的意图类型。

【初始目标】
{{SECTION:topic}}

【已累积的补充/细化约束】
{{SECTION:notes}}

【参与者名册（id / 显示名 / 角色）】
{{SECTION:roster}}

【用户最新输入】
{{SECTION:latest}}

{{FRAGMENT:rule.strict_json}}，格式：
{"intent":"refine|change_goal|new_goal|amend_goal|temp_task|switch_director","new_goal":"新目标文本(仅 change_goal/new_goal)","amend":"追加到目标的补充文本(仅 amend_goal)","note":"补充/细化约束(仅 refine 且确有新约束)","temp_task":{"description":"临时任务描述","assignee":"可选参与者id"}(仅 temp_task),"director":"新主持人参与者id(仅 switch_director)"}
判定规则（自上而下，命中即止）：
1. 若用户明确要求更换/指定主持人（如"让XX当主持人""换XX主持""XX来主持""主持人改成XX"）→ intent=switch_director，director 填名册中目标参与者的裸 id（用户可能用显示名称呼，请对照名册中的显示名解析）；
2. 若用户提出与当前目标完全无关的全新目标/话题（如"算了，我们来做…""换一个完全不同的任务…""新任务：…"）→ intent=new_goal，new_goal 填新目标；
3. 若用户明确要修改/更换/明显转向当前目标（如"改成…""换成…""重新设定目标为…""别做A了做B""重点转向B""先别做界面了，把接口调通"）→ intent=change_goal，new_goal 填新目标；
4. 若用户是在【目标范围】上补充/扩展长期目标内容（如"目标增加…""再加上…""还要覆盖…""补充一个目标…"）→ intent=amend_goal，amend 填追加内容（将修正初始目标）；
5. 若用户要求的是【临时性、一次性额外事务】（如"顺便…""临时…""先处理一下…""额外加个任务…""插队做个X"）→ intent=temp_task，temp_task 填任务描述与可选 assignee（不更新初始目标）；
6. 其余对当前进度的修正性/规范性/指示性要求（如"应该/必须…""流程上…""按xx来…""先确认xx再继续"）→ intent=refine，若蕴含新的约束或需求，提炼为一句简洁的 note（否则留空）；
默认优先判 refine；但用户明确改变目标/方向/主持人时不得降级为 refine 被吞并。
区分要点：amend 管"做什么"（目标范围扩展，并入目标锚点）；refine 管"怎么做"（规范/流程/指示，仅累积约束）；temp 是一次性事务（执行完即弃，不并入目标）；change 是替换或明显转向当前目标；switch_director 是更换协调者结构身份。
