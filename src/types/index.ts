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
  /** 会话来源：'workflow' = 工作流 Agent 节点自动创建；缺省 = 用户会话 */
  origin?: string;
  /** 模型温度 (0.0-2.0) */
  temperature?: number;
  /** 最大生成 token 数 */
  maxTokens?: number;
}

/** 项目级记忆（<记忆根>/MEMORY.md）读取结果 */
export interface ProjectMemoryInfo {
  root: string;
  exists: boolean;
  content: string;
  charCount: number;
}

/** 已使用项目根列表（会话 cwd 去重 + 兜底工作区） */
export async function listProjectRoots(): Promise<string[]> {
  return await _invoke('list_project_roots');
}

/** 读取指定记忆根 MEMORY.md；cwd 为空=兜底记忆根 */
export async function getProjectMemory(cwd?: string): Promise<ProjectMemoryInfo> {
  return await _invoke('get_project_memory', { cwd: cwd || null });
}

/** 写入指定记忆根 MEMORY.md */
export async function updateProjectMemory(cwd: string | undefined, content: string): Promise<ProjectMemoryInfo> {
  return await _invoke('update_project_memory', { cwd: cwd || null, content });
}

/** MEMORY.md 新建模板 */
export async function projectMemoryTemplate(): Promise<string> {
  return await _invoke('project_memory_template');
}

/** 读取用户偏好 USER.md（统一配置根；不存在时 exists=false） */
export async function getUserPreferences(): Promise<ProjectMemoryInfo> {
  return await _invoke('get_user_preferences');
}

/** 写入用户偏好 USER.md */
export async function updateUserPreferences(content: string): Promise<ProjectMemoryInfo> {
  return await _invoke('update_user_preferences', { content });
}

/** USER.md 新建模板 */
export async function userPreferencesTemplate(): Promise<string> {
  return await _invoke('user_preferences_template');
}

/** 全局 KV 记忆条目 */
export interface MemoryEntryView {
  key: string;
  value: string;
  category: string;
  createdAt: number;
  updatedAt: number;
  accessCount: number;
  /** 最近一次被检索/读取的时间（内容编辑不刷新） */
  lastAccessedAt: number;
  /** 重要标记：置位后永不参与自动清理 */
  pin: boolean;
  /** 逗号分隔的检索标签（可选，补充 key 词面的检索面） */
  tags: string;
}

/** KV 记忆自动维护策略（后端常量同源） */
export interface MemoryPolicy {
  maxEntries: number;
  idleDays: number;
  minAccess: number;
}

/** 全局 KV 记忆统计（总数/pin 数/注入 top-5/可清理候选数 + 策略） */
export interface MemoryStats {
  total: number;
  pinned: number;
  candidates: number;
  injected: MemoryEntryView[];
  policy: MemoryPolicy;
}

export async function listMemoryEntries(category?: string, query?: string): Promise<MemoryEntryView[]> {
  return await _invoke('list_memory_entries', { category: category || null, query: query || null });
}

export async function saveMemoryEntry(key: string, value: string, category: string, important?: boolean, tags?: string): Promise<MemoryEntryView> {
  return await _invoke('save_memory_entry', { key, value, category, important: important ?? false, tags: tags ?? null });
}

export async function deleteMemoryEntry(key: string): Promise<boolean> {
  return await _invoke('delete_memory_entry', { key });
}

/** 置/取消某条 KV 记忆的 pin（重要）标记 */
export async function setMemoryPin(key: string, pin: boolean): Promise<void> {
  return await _invoke('set_memory_pin', { key, pin });
}

/** 预览自动维护将清理的候选条目（不删除） */
export async function previewMemoryMaintenance(): Promise<MemoryEntryView[]> {
  return await _invoke('preview_memory_maintenance');
}

/** 执行一次自动维护（僵尸清理 + 配额驱逐），返回被删除的 key 列表 */
export async function runMemoryMaintenance(): Promise<string[]> {
  return await _invoke('run_memory_maintenance');
}

export async function getMemoryStats(): Promise<MemoryStats> {
  return await _invoke('get_memory_stats');
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

// ── 全局用量统计（api_usage_log） ──

export interface UsageTotals {
  callCount: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  /** 总缓存 = cacheReadTokens + cacheWriteTokens（兼容旧展示）。 */
  cachedTokens: number;
  /** 缓存命中读取 token（未命中输入见 promptTokens）。 */
  cacheReadTokens: number;
  /** 缓存写入 token（如 Anthropic cache_creation）。 */
  cacheWriteTokens: number;
  /** 0-100 缓存命中率 = cacheReadTokens / (promptTokens + cacheReadTokens + cacheWriteTokens)。 */
  cacheHitRate: number;
}

export interface UsageGroup {
  name: string;
  totals: UsageTotals;
}

export interface UsageDay {
  date: string;
  promptTokens: number;
  cachedTokens: number;
  /** 与 `UsageTotals` 同口径：聚合（按周/月）时必须用这两项按桶求和后**重算**命中率，不能取平均。 */
  cacheReadTokens: number;
  cacheWriteTokens: number;
  cacheHitRate: number;
}

export interface UsageSummary {
  totals: UsageTotals;
  byProvider: UsageGroup[];
  byModel: UsageGroup[];
  trend: UsageDay[];
}

/** 拉取全局用量汇总。days<=0 表示全量，否则为近 N 天窗口。 */
export async function getUsageSummary(days = 30): Promise<UsageSummary> {
  return await _invoke('get_usage_summary', { days });
}

/**
 * 单个成本归因维度：既是聚合口径（totals 供「按归因」总览），也是明细入口（groups 供下钻）。
 * key ∈ `session` | `groupchat` | `workflow` | `knowledge`。
 */
export interface UsageDimension {
  key: string;
  /** 该维度合计（各分组四桶累加，命中率按累加结果重算）。 */
  totals: UsageTotals;
  /**
   * 明细分组：会话=一行一会话；群聊=一房间一行；工作流=一定义一行；知识库=一库一行。
   *
   * **知识库维度的 `name` 是库 id**（知识库定义在 MEMORY.db，用量查询在主库上，跨库不 JOIN），
   * 由 `UsageStats` 用 `kb_list_bases` 映射成库名展示；映射不到（库已删）就显示 id。
   */
  groups: UsageGroup[];
}

/**
 * 成本归因维度汇总（固定顺序：会话 → 群聊 → 工作流 → 知识库）。
 * 前两者与工作流归因自 `api_usage_log.session_id` 的命名约定与会话来源推导；
 * 知识库按 `kb:{kbId}` 前缀识别（写入方 `commands/knowledge.rs::record_kb_usage`）。
 * CLI Agent（终端/插件/CLI 会话）不经宿主发起调用，无用量行，故合计可能小于全局总量。
 */
export interface UsageAttribution {
  dimensions: UsageDimension[];
}

/** 拉取三维成本归因。days<=0 表示全量，否则为近 N 天窗口。 */
export async function getUsageAttribution(days = 30): Promise<UsageAttribution> {
  return await _invoke('get_usage_attribution', { days });
}

/** 群聊房间用量（director + 各参与者，按 model/provider 聚合）。 */
export interface RoomUsage {
  totals: UsageTotals;
  byModel: UsageGroup[];
  byProvider: UsageGroup[];
}

export async function getRoomUsage(roomId: string): Promise<RoomUsage> {
  return await _invoke('get_room_usage', { roomId });
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
  version: string;
}



/** Search engine result item */
export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}
