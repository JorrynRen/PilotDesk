export type ChatMode = 'native' | 'fast' | 'think' | 'expert';

export interface ChatRequest {
  sessionId: string;
  message: string;
  mode: ChatMode;
  cwd?: string;
}

export interface ChatChunk {
  type: 'chunk' | 'done' | 'error' | 'status' | 'skills';
  sessionId: string;
  content?: string;
  error?: string;
  status?: string;
}

export interface WsMessage {
  type: 'chat' | 'stop' | 'session:create' | 'session:close' | 'ping' | 'skills:list' | 'skills:list-all';
  sessionId: string;
  agentType?: 'claude' | 'hermes';
  message?: string;
  mode?: ChatMode;
  cwd?: string;
}

export const MODE_PROMPTS: Record<ChatMode, string> = {
  native: '',
  fast: '快速简洁回答，直接给出结论，无需详细解释推理过程',
  think: '逐步分析推理，详细解释你的思路和过程，给出完整的推理链',
  expert: '以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案',
};

export interface SkillInfo {
  name: string;
  description: string;
  category?: string;
}

export interface SkillsListMessage {
  type: 'skills:list' | 'skills:list-all';
  sessionId: string;
  agentType?: 'claude' | 'hermes';
}

export interface SkillsListResponse {
  type: 'skills';
  sessionId: string;
  agentType: 'claude' | 'hermes';
  skills: SkillInfo[];
}
