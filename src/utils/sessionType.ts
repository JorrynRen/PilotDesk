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

/** Get the CSS variable for an agent type's theme color */
export function agentTypeToCssVar(agentType: string): string {
  const map: Record<string, string> = {
    claude: 'var(--claude-tag)',
    hermes: 'var(--hermes-tag)',
    codex: 'var(--codex-tag)',
    api: 'var(--api-tag)',
  };
  return map[agentType] || 'var(--text-tertiary)';
}

/** Get a human-readable label for an agent type */
export function getAgentLabel(agentType: string): string {
  const map: Record<string, string> = {
    claude: 'Claude Code',
    hermes: 'Hermes Agent',
    codex: 'codeX',
    api: 'API 模型',
  };
  return map[agentType] || agentType;
}
