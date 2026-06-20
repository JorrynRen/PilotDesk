import { invoke as _invoke } from '@tauri-apps/api/core';
export { _invoke as invoke };

export interface Session {
  id: string;
  agentType: string;
  title: string;
  cwd: string;
  createdAt: number;
  updatedAt: number;
  lastMessagePreview: string;
  messageCount: number;
  status: 'active' | 'archived';
  /** For API direct sessions: which provider (e.g. "anthropic", "openai") */
  apiProvider?: string;
  /** For API direct sessions: which model (e.g. "claude-sonnet-4-20250514") */
  apiModel?: string;
  /** Agent-side session ID (e.g. Claude Code session UUID) for session continuity */
  agentSessionId?: string;
}

export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
  /** Reasoning/thinking content (e.g. DeepSeek reasoning_content) */
  reasoningContent?: string;
  /** Tool calls requested by the model (JSON array string) */
  toolCalls?: string;
  /** Tool call ID for role='tool' messages */
  toolCallId?: string;
  /** Tool name for role='tool' messages */
  toolName?: string;
}

/** Tool definition following OpenAI function calling format */
export interface ToolDefinition {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}

/** Tool call returned by the model */
export interface ToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

/** Tool execution result */
export interface ToolResult {
  toolCallId: string;
  toolName: string;
  content: string;
  isError?: boolean;
}

export interface Inspiration {
  id: string;
  icon: string;
  title: string;
  content: string;
  sourceAgent: string;
  isFavorite: boolean;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

export interface EnvInfo {
  nodeVersion: string | null;
  gitVersion: string | null;
  pythonVersion: string | null;
  /** Dynamic agent versions keyed by agent_type */
  agentVersions: Record<string, string | null>;
}

export type ChatMode = 'native' | 'fast' | 'think' | 'expert';

export type PanelContent =
  | { kind: 'inspiration-form'; prefill: string }
  | { kind: 'skill-detail'; skillName: string }
  | { kind: 'memory-browser' }
  | { kind: 'bot-setup'; agent: string }
  | { kind: 'update-check' };

/**
 * Chat mode system prompts — loaded from app_settings storage.
 * Falls back to built-in defaults if not yet customized.
 */
const DEFAULT_MODE_PROMPTS: Record<ChatMode, string> = {
  native: '',
  fast: '快速简洁回答，直接给出结论，无需详细解释推理过程',
  think: '逐步分析推理，详细解释你的思路和过程，给出完整的推理链',
  expert: '以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案',
};

export const MODE_PROMPTS_DEFAULTS = DEFAULT_MODE_PROMPTS;

/** Get the current system prompt for a chat mode (from storage or default) */
export async function getModePrompt(mode: ChatMode): Promise<string> {
  try {
    const key = `mode_prompt_${mode}`;
    const value = await _invoke('get_app_setting', { key });
    if (typeof value === 'string' && value !== '') return value;
  } catch { /* storage not available or error — use default */ }
  return DEFAULT_MODE_PROMPTS[mode] ?? '';
}

/** Save a custom system prompt for a chat mode */
export async function saveModePrompt(mode: ChatMode, prompt: string): Promise<void> {
  const key = `mode_prompt_${mode}`;
  try {
    await _invoke('set_app_setting', { key, value: prompt });
  } catch { /* ignore save errors */ }
}

/** Get all mode prompts at once */
export async function getAllModePrompts(): Promise<Record<ChatMode, string>> {
  const modes: ChatMode[] = ['native', 'fast', 'think', 'expert'];
  const result: Record<ChatMode, string> = { native: '', fast: '', think: '', expert: '' };
  for (const m of modes) {
    result[m] = await getModePrompt(m);
  }
  return result;
}

export const MODE_LABELS: Record<ChatMode, string> = {
  native: '原生',
  fast: '快速',
  think: '深度思考',
  expert: '专家',
};

export const MODE_COLORS: Record<ChatMode, string> = {
  native: 'var(--text-secondary)',
  fast: '#10B981',
  think: 'var(--accent)',
  expert: 'var(--hermes-tag)',
};

/** Agent type theme configuration — centralized color, label, and initial for all agent types */
export interface AgentTheme {
  color: string;    // Primary hex color
  bg: string;       // Background with alpha (rgba)
  label: string;    // Display name
  initial: string;  // Single letter initial
  cssVar: string;   // CSS variable reference
  icon?: string;    // Icon reference (file:filename.ico / URL / emoji)
}

/** Agent config from the backend agents table */
export interface AgentConfig {
  agentType: string;
  displayName: string;
  description: string;
  cliCommand: string;
  npmPackage: string | null;
  pipPackage: string | null;
  installCmd: string;
  uninstallCmd: string;
  updateCmd: string;
  versionCmd: string;
  latestVersionCmd: string;
  runCmdTemplate: string;
  outputParser: string;
  outputFilterRegex: string;
  versionPattern: string;
  supportsSessionContinuity: boolean;
  sessionIdSource: string;
  sessionIdEventType: string;
  sessionIdField: string;
  resumeArgTemplate: string;
  skillsDir: string;
  skillEntryFile: string;
  skillDisplayMode: string;
  color: string;
  icon?: string;
  sortOrder: number;
  isEnabled: boolean;
  isBuiltin: boolean;
}

/**
 * @deprecated Use useAgentRegistry() hook instead for dynamic agent themes.
 * This static map is kept as fallback for components not yet migrated.
 */
export const AGENT_THEMES: Record<string, AgentTheme> = {
  claude: {
    color: '#3B82F6',
    bg: 'rgba(59,130,246,0.15)',
    label: 'Claude Code',
    initial: 'C',
    cssVar: 'var(--claude-tag)',
    icon: 'file:claude_icon.ico',
  },
  hermes: {
    color: '#8B5CF6',
    bg: 'rgba(139,92,246,0.15)',
    label: 'Hermes Agent',
    initial: 'H',
    cssVar: 'var(--hermes-tag)',
    icon: 'file:hermes_icon.ico',
  },
  api: {
    color: '#10B981',
    bg: 'rgba(16,185,129,0.15)',
    label: 'API 直连',
    initial: 'A',
    cssVar: 'var(--api-tag)',
  },
  codex: {
    color: '#F59E0B',
    bg: 'rgba(245,158,11,0.15)',
    label: 'codeX',
    initial: 'X',
    cssVar: 'var(--codex-tag)',
    icon: 'file:codex_icon.ico',
  },
  manual: {
    color: '#6B7280',
    bg: 'rgba(107,114,128,0.15)',
    label: '手动',
    initial: 'M',
    cssVar: 'var(--text-tertiary)',
  },
};


/** Search engine result item */
export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}
