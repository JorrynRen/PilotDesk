{{FRAGMENT:identity.director}}执行者发起了一次工具调用，需要你裁决是否放行。

【当前讨论目标】
{{SECTION:topic}}

工具名：{{SECTION:tool_name}}
风险等级：{{SECTION:risk}}
参数：{{SECTION:args}}

请基于安全性与任务必要性进行裁决：若该工具调用服务于【当前讨论目标】且参数合理则放行；若明显危险、越权或与目标无关则拒绝。{{FRAGMENT:rule.strict_json}}，格式：
{"allow":true或false,"reason":"简要理由"}
