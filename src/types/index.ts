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
  /** 模型温度 (0.0-2.0) */
  temperature?: number;
  /** 最大生成 token 数 */
  maxTokens?: number;
}

/** 附件（图片/文件，落盘 + 路径引用） */
export interface Attachment {
  /** "image" 或 "file" */
  kind: 'image' | 'file';
  /** 原始文件名 */
  name: string;
  /** 落盘后的绝对路径 */
  path: string;
  /** MIME 类型 */
  mime: string;
  /** 文件大小（字节） */
  size: number;
}

export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert' | 'plan';
  timestamp: number;
  /** 完整思维链（reasoning + tool 调用 + file_diff 步骤，JSON 数组字符串） */
  toolCalls?: string;
  /** Tool call ID for role='tool' messages */
  toolCallId?: string;
  /** Tool name for role='tool' messages */
  toolName?: string;
  /** 附件（图片/文件，落盘 + 路径引用），仅用于 user 消息 */
  attachments?: Attachment[];
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
  /** Dynamic agent latest versions keyed by agent_type */
  agentLatestVersions?: Record<string, string | null>;
}

export type ChatMode = 'native' | 'fast' | 'think' | 'expert' | 'plan';

export type PanelContent =
  | { kind: 'inspiration-form'; prefill: string }
  | { kind: 'skill-detail'; skillName: string }
  | { kind: 'memory-browser' }
  | { kind: 'bot-setup'; agent: string }
  | { kind: 'update-check' };

/** Get the current system prompt for a chat mode (from app_settings) */
export async function getModePrompt(mode: ChatMode): Promise<string> {
  try {
    const key = `mode_prompt_${mode}`;
    const value = await _invoke('get_app_setting', { key });
    if (typeof value === 'string' && value !== '') return value;
  } catch { /* storage not available */ }
  // 规划模式默认 prompt
  if (mode === 'plan') {
    return `You are operating in PLAN MODE. Before executing any actions, follow these rules:

1. **Analyze** the user's request thoroughly
2. **Create a structured plan** with clear, numbered steps
3. **Present the plan** to the user for review — do NOT execute yet
4. **Wait for approval** before proceeding with any execution

Format your plan as:
\`\`\`
## Plan: [Brief title]

**Goal:** [One-sentence summary]

**Steps:**
1. [Step 1 description]
2. [Step 2 description]
...
N. [Final step description]

**Expected outcome:** [What will be achieved]

Ready to execute? Confirm to proceed.
\`\`\`

After the user confirms, proceed to execute each step.`;
  }
  return '';
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
  const modes: ChatMode[] = ['native', 'fast', 'think', 'expert', 'plan'];
  const result: Record<ChatMode, string> = { native: '', fast: '', think: '', expert: '', plan: '' };
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
  plan: '规划',
};

export const MODE_COLORS: Record<ChatMode, string> = {
  native: 'var(--mode-native)',
  fast: 'var(--mode-fast)',
  think: 'var(--mode-think)',
  expert: 'var(--mode-expert)',
  plan: 'var(--mode-think)',
};

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
  version: string;
}



/** Search engine result item */
export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}
