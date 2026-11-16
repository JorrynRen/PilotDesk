/**
 * sessionDraftStore — 会话模式「快捷开始」的草稿参数。
 *
 * 背景：新建一个会话真正必填的只有 **会话方式**（CLI Agent / API 提供商·模型）；
 * 工作目录由项目归属或全局工作空间在运行时动态解析。标题可选（运行中可生成）、
 * 温度/maxTokens 有默认值。用户输入消息并发送时再按草稿建会话，
 * 惰性创建不会产生"只建了没用"的空会话。
 *
 * 口径：
 * - `choice` 用一个字符串同时表达"大类 + 具体项"，既方便持久化，也直接对应选择器的 value：
 *   - `cli:<agentType>`（例如 `cli:claude`）
 *   - `api:<providerId>::<model>`（例如 `api:p1::gpt-4o`）
 * - 会话方式记忆在 localStorage，下次打开沿用上次选择
 */
import { create } from 'zustand';

const CHOICE_STORAGE_KEY = 'pilotdesk.session-draft.choice';
const PROJECT_STORAGE_KEY = 'pilotdesk.session-draft.project';

/** CLI 会话方式的选择值 */
export function cliChoice(agentType: string): string {
  return `cli:${agentType}`;
}

/** API 会话方式的选择值（提供商 + 模型一起定，避免两级联动状态） */
export function apiChoice(providerId: string, model: string): string {
  return `api:${providerId}::${model}`;
}

/** 解析选择值（无法解析时返回 null，调用方据此禁用发送） */
export function parseChoice(choice: string): { agentType: string; apiProvider: string | null; apiModel: string | null } | null {
  if (choice.startsWith('cli:')) {
    const agentType = choice.slice(4);
    return agentType ? { agentType, apiProvider: null, apiModel: null } : null;
  }
  if (choice.startsWith('api:')) {
    const rest = choice.slice(4);
    const sep = rest.indexOf('::');
    if (sep <= 0) return null;
    const provider = rest.slice(0, sep);
    const model = rest.slice(sep + 2);
    if (!provider || !model) return null;
    return { agentType: 'api', apiProvider: provider, apiModel: model };
  }
  return null;
}

function load(key: string): string {
  try {
    return localStorage.getItem(key) ?? '';
  } catch {
    return '';
  }
}

function save(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 配额/隐私模式失败可忽略：只是下次要重选 */
  }
}

interface SessionDraftState {
  /** 会话方式选择值（见 cliChoice / apiChoice）；空串 = 未选 */
  choice: string;
  setChoice: (choice: string) => void;
  /** 快捷开始的项目归属；空串 = 未归属项目，使用全局工作空间 */
  projectId: string;
  setProjectId: (projectId: string) => void;
  /** 草稿是否就绪（会话方式已选）—— InputBar 据此放行发送 */
  ready: () => boolean;
  /** 解析成 createSession 入参（未就绪返回 null） */
  resolve: () => {
    agentType: string;
    apiProvider: string | null;
    apiModel: string | null;
    projectId: string | null;
  } | null;
}

export const useSessionDraftStore = create<SessionDraftState>((set, get) => ({
  choice: load(CHOICE_STORAGE_KEY),
  projectId: load(PROJECT_STORAGE_KEY),

  setChoice: (choice) => {
    save(CHOICE_STORAGE_KEY, choice);
    set({ choice });
  },

  setProjectId: (projectId) => {
    save(PROJECT_STORAGE_KEY, projectId);
    set({ projectId });
  },

  ready: () => {
    return parseChoice(get().choice) !== null;
  },

  resolve: () => {
    const parsed = parseChoice(get().choice);
    if (!parsed) return null;
    return {
      agentType: parsed.agentType,
      apiProvider: parsed.apiProvider,
      apiModel: parsed.apiModel,
      projectId: get().projectId || null,
    };
  },
}));
