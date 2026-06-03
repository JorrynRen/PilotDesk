export interface Session {
  id: string;
  agentType: 'claude' | 'hermes' | 'api';
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
  sourceAgent: 'claude' | 'hermes' | 'manual';
  isFavorite: boolean;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

export interface BotChannel {
  id: string;
  agentType: 'claude' | 'hermes';
  platform: string;
  method: string;
  status: string;
  triggerPrefix: string;
  responseFormat: string;
  config: Record<string, unknown>;
  createdAt: number;
  updatedAt: number;
}

export interface EnvInfo {
  nodeVersion: string | null;
  gitVersion: string | null;
  pythonVersion: string | null;
  claudeCodeVersion: string | null;
  hermesVersion: string | null;
}

export type ChatMode = 'native' | 'fast' | 'think' | 'expert';

export type PanelContent =
  | { kind: 'inspiration-form'; prefill: string }
  | { kind: 'skill-detail'; skillName: string }
  | { kind: 'memory-browser' }
  | { kind: 'config-editor'; agent: string }
  | { kind: 'bot-setup'; agent: string }
  | { kind: 'update-check' };

export const MODE_PROMPTS: Record<ChatMode, string> = {
  native: '',
  fast: '快速简洁回答，直接给出结论，无需详细解释推理过程',
  think: '逐步分析推理，详细解释你的思路和过程，给出完整的推理链',
  expert: '以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案',
};

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
}

export const AGENT_THEMES: Record<string, AgentTheme> = {
  claude: {
    color: '#3B82F6',
    bg: 'rgba(59,130,246,0.15)',
    label: 'Claude Code',
    initial: 'C',
    cssVar: 'var(--claude-tag)',
  },
  hermes: {
    color: '#8B5CF6',
    bg: 'rgba(139,92,246,0.15)',
    label: 'Hermes Agent',
    initial: 'H',
    cssVar: 'var(--hermes-tag)',
  },
  api: {
    color: '#10B981',
    bg: 'rgba(16,185,129,0.15)',
    label: 'API 直连',
    initial: 'A',
    cssVar: 'var(--api-tag)',
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
