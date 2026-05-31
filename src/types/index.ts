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
  role: 'user' | 'assistant' | 'system';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
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

/** Available API providers for direct API sessions */
export const API_PROVIDERS = [
  {
    id: 'anthropic',
    name: 'Anthropic Claude',
    models: ['claude-sonnet-4-20250514', 'claude-opus-4-20250514', 'claude-3-5-haiku-20241022'],
  },
  {
    id: 'openai',
    name: 'OpenAI',
    models: ['gpt-4o', 'gpt-4o-mini', 'o1', 'o1-mini'],
  },
] as const;
