/**
 * Session type utilities — centralize agent type checks.
 * Use these helpers instead of hardcoded `agentType === 'api'` comparisons.
 */

/** Check if a session uses API direct mode (not an agent subprocess) */
export function isApiSession(agentType: string): boolean {
  return agentType === 'api';
}

/** Check if a session uses an agent subprocess (Claude Code / Hermes / Codex / custom) */
export function isAgentSession(agentType: string): boolean {
  return agentType !== 'api';
}

/**
 * 会话是否由工作流节点自动创建的内部会话。
 * 值形如 `workflow:{工作流定义id}`（历史行仅 `workflow`），故按前缀判定，
 * 不依赖标题文案（标题是展示用的可变文案）。
 */
export function isWorkflowSession(origin?: string | null): boolean {
  return typeof origin === 'string' && origin.startsWith('workflow');
}