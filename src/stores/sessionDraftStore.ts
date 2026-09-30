/**
 * sessionDraftStore — 会话模式「快捷开始」的草稿参数。
 *
 * 背景：新建一个会话真正必填的只有两样 —— **工作目录** 与 **会话方式**（CLI Agent / API 提供商·模型）；
 * 标题可选（运行中可生成）、温度/maxTokens 有默认值。于是 InputBar 在**没有选中会话**时直接提供
 * 这两个选择，用户输入消息并发送时再按草稿建会话（惰性创建：不产生"只建了没用"的空会话）。
 *
 * 口径：
 * - `choice` 用一个字符串同时表达"大类 + 具体项"，既方便持久化，也直接对应选择器的 value：
 *   - `cli:<agentType>`（例如 `cli:claude`）
 *   - `api:<providerId>::<model>`（例如 `api:p1::gpt-4o`）
 * - `cwd`：绝对路径 / `GLOBAL_CWD`（全局工作空间）/ 空串（未选 → 不允许发送）
 * - 两个值都记忆在 localStorage，下次打开沿用上次选择
 */
import { create } from 'zustand';

/** 工作目录选「使用全局工作空间」的哨兵值（不落 AnyProject 目录，交由后端按默认解析） */
export const GLOBAL_CWD = '__global__';

const CHOICE_STORAGE_KEY = 'pilotdesk.session-draft.choice';
const CWD_STORAGE_KEY = 'pilotdesk.session-draft.cwd';

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
  /** 工作目录：绝对路径 / GLOBAL_CWD / 空串（未选） */
  cwd: string;
  setChoice: (choice: string) => void;
  setCwd: (cwd: string) => void;
  /** 草稿是否就绪（目录 + 会话方式都选了）—— InputBar 据此放行发送 */
  ready: () => boolean;
  /** 解析成 createSession 入参（未就绪返回 null） */
  resolve: () => { agentType: string; apiProvider: string | null; apiModel: string | null; cwd: string | null } | null;
}

export const useSessionDraftStore = create<SessionDraftState>((set, get) => ({
  choice: load(CHOICE_STORAGE_KEY),
  cwd: load(CWD_STORAGE_KEY),

  setChoice: (choice) => {
    save(CHOICE_STORAGE_KEY, choice);
    set({ choice });
  },

  setCwd: (cwd) => {
    save(CWD_STORAGE_KEY, cwd);
    set({ cwd });
  },

  ready: () => {
    const { choice, cwd } = get();
    return Boolean(cwd) && parseChoice(choice) !== null;
  },

  resolve: () => {
    const { choice, cwd } = get();
    if (!cwd) return null;
    const parsed = parseChoice(choice);
    if (!parsed) return null;
    return {
      agentType: parsed.agentType,
      apiProvider: parsed.apiProvider,
      apiModel: parsed.apiModel,
      // 全局工作空间：不传 cwd（后端按默认工作区解析）
      cwd: cwd === GLOBAL_CWD ? null : cwd,
    };
  },
}));
