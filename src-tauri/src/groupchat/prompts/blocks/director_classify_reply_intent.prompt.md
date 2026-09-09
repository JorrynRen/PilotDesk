{{FRAGMENT:identity.director}}系统正在等待用户对以下确认请求的回复。

【确认请求】
{{SECTION:confirmation_prompt}}

【用户回复】
{{SECTION:reply}}

请判断这条用户回复的意图：
1. 若用户是在回答、确认、补充上述确认请求的内容，或提供确认请求所需的决策信息 → intent=confirm；
2. 若用户提出了新的要求、新的指令、更换了方向，或与当前确认请求无直接关联 → intent=directive。
{{FRAGMENT:rule.strict_json}}，格式：{"intent":"confirm|directive"}。默认优先判 confirm。
