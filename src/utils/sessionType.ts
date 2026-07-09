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