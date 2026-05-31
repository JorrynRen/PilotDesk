import type { ChatRequest, ChatMode } from '../types';

export interface AgentAdapter {
  agentType: 'claude' | 'hermes';
  createSession(sessionId: string, cwd?: string): Promise<void>;
  closeSession(sessionId: string): Promise<void>;
  sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown>;
  stopGeneration(sessionId: string): void;
}

export function buildPrompt(message: string, mode: ChatMode): string {
  const modePrompts: Record<ChatMode, string> = {
    native: '',
    fast: '快速简洁回答，直接给出结论，无需详细解释推理过程',
    think: '逐步分析推理，详细解释你的思路和过程，给出完整的推理链',
    expert: '以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案',
  };
  const prefix = modePrompts[mode];
  return prefix ? `[系统指令：${prefix}]

${message}` : message;
}
