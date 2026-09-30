import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Session, Message } from '../types';
import type { WorkflowDefinition } from '../types/workflow';
import { elide } from '../utils/text';
import { isWorkflowSession } from '../utils/sessionType';
import { globalEventBus } from '../plugin/GlobalEventBus';

/**
 * 宿主事件投递（宿主 → 插件），命名空间 `动作:对象`。
 *
 * 纯通知：`globalEventBus.emit` 已逐个 try/catch 各插件的 handler，这里再兜一层，
 * 保证"投递失败也绝不影响应用主流程"。载荷**只放标识**（id / 角色 / 类型），
 * 不放消息正文、密钥或文件内容。
 */
function emitHostEvent(event: string, payload: Record<string, unknown>): void {
  try {
    globalEventBus.emit(event, payload);
  } catch (err) {
    console.warn(`[sessionStore] 投递事件 ${event} 失败:`, err);
  }
}

/**
 * 已写入「机械占位标题」、等着用模型标题回填的会话（一次性闸门）。
 *
 * 两个作用：① 只在首轮**结束后**补一次标题；② 只对"我们自己写过占位"的会话生效——
 * 用户自定义标题、工作流会话标题从不进这个集合，因此不会被模型标题覆盖。
 */
const pendingModelTitle = new Set<string>();

/** 前端确认后回传的待转换任务（与后端 PlanTaskInput 对齐） */
export interface PlanTaskInput {
  title: string;
  detail: string;
  /**
   * 依赖：本次提交数组里的 **1-based 位置**（不是显示序号）；
   * `[]` = 显式无依赖（可并行）；`null`/省略 = 未提供（后端按提交顺序串成链，兼容旧前端）
   */
  deps?: number[] | null;
  /** 预览时的默认勾选状态（仅回传，后端不参与组装） */
  suggested?: boolean;
  /**
   * 确认类任务（需要用户拍板）：转工作流时生成「人工交互」节点（运行期挂起等用户作答），
   * 否则生成 Agent 节点。模型提炼会按需标记，用户可在弹窗里逐行切换。
   */
  confirm?: boolean;
  /** 确认项的默认值（仅 confirm 有意义）：无人应答时交互节点按它继续，自动运行不空转 */
  defaultValue?: string | null;
}

/** 转换来源：会话计划（todo_write）/ 退化的用户消息 / 前端编辑后的清单 */
export type SessionPlanSource = 'todos' | 'messages' | 'edited';

/** 候选来源：会话计划（todo_write）/ 用户发言 / 模型提炼 / 前端回传的清单 */
export type PlanCandidateOrigin = 'todos' | 'messages' | 'ai' | 'edited';

/** 后端返回的候选任务（title/detail 已按 60/400 字符截断） */
export interface PlanCandidate {
  title: string;
  detail: string;
  /** 是否"建议默认勾选"；false 表示被自动忽略的确认/寒暄/追问，用户可勾回来 */
  suggested: boolean;
  /**
   * 候选来源分组：todos / messages / ai 同时并列（and 合并，不做二选一）；
   * edited 仅出现在"提交覆盖"的返回里（前端回传清单），可忽略。
   */
  origin?: PlanCandidateOrigin;
  /**
   * 依赖：**本数组内的 1-based 位置**（后端已过滤越界与自依赖）。
   * 仅 `session_extract_tasks`（模型提炼）返回；会话计划/消息候选不含依赖。
   */
  deps?: number[];
  /** 确认类任务（模型提炼按 kind=confirm 标记）：转工作流时生成「人工交互」节点 */
  confirm?: boolean;
  /** 确认项的默认值（模型按 default 给出；空/未给为 null）：无人应答时交互节点按它继续 */
  defaultValue?: string | null;
}

/** 会话 → 工作流：预览结果（定义 + 来源信息 + 候选清单） */
export interface WorkflowExportResult {
  definition: WorkflowDefinition;
  goal: string;
  source: SessionPlanSource;
  /** 被默认取消勾选的候选条数 */
  ignoredCount: number;
  /** 本会话里用户已确认过的需求条数（来自 ask_user 问答）；答复已并入总目标（goal），运行期无需再确认 */
  confirmedCount: number;
  tasks: PlanCandidate[];
}

/** 会话 → 群聊预览里的单条任务（与后端 PlanTask 对齐） */
export interface RoomPlanTask {
  no: number;
  title: string;
  detail: string;
  /** 是否"建议默认勾选"；false 表示被自动忽略的确认/寒暄/追问，用户可勾回来 */
  suggested: boolean;
  /** 候选来源分组（口径同 PlanCandidate.origin） */
  origin?: PlanCandidateOrigin;
  /** 前置任务 key（如 "t1"） */
  deps: string[];
}

/** 会话 → 群聊：预览结果（房间标题 / 议题 / 任务清单 + 来源信息） */
export interface RoomPlan {
  sessionId: string;
  title: string;
  topic: string;
  source: SessionPlanSource;
  /** 被默认取消勾选的候选条数 */
  ignoredCount: number;
  /** 本会话里用户已确认过的需求条数（来自 ask_user 问答）；答复已并入总目标（topic），运行期无需再确认 */
  confirmedCount: number;
  tasks: RoomPlanTask[];
}

interface SessionState {
  sessions: Session[];
  archivedSessions: Session[];
  currentSessionId: string | null;
  messages: Message[];
  messageIds: Set<string>;       // ID 去重集合
  isLoadingSessions: boolean;
  isLoadingMessages: boolean;
  showArchived: boolean;

  fetchSessions: () => Promise<void>;
  /** 静默刷新会话列表（不改变 isLoadingSessions，不触发 DOM 替换） */
  refreshSessions: () => Promise<void>;
  selectSession: (id: string) => Promise<void>;
  /**
   * 回到「快捷开始」：清空当前选中（不建会话、不动数据）。
   * 用于"在已有会话里想开新会话"——输入区切回草稿态，首次发送时才建会话。
   */
  startNewSession: () => void;
  createSession: (
    agentType: string,
    cwd?: string,
    title?: string | null,
    apiProvider?: string,
    apiModel?: string,
    temperature?: number,
    maxTokens?: number,
  ) => Promise<Session>;
  renameSession: (id: string, newTitle: string) => Promise<void>;
  /**
   * 会话标题契约：依据第一条用户消息让模型起标题（后端"比较后再写"，落库也在后端）。
   * `expectedTitle` = 调用时前端显示的标题；不可用/失败时保留原标题，不做改动。
   */
  suggestSessionTitle: (id: string, expectedTitle: string) => Promise<void>;
  /** 切换会话工作目录（项目根），对后续消息生效 */
  updateSessionCwd: (id: string, cwd: string) => Promise<void>;
  /**
   * 切换 API 会话的提供商/模型，对**后续**消息生效（运行期每轮从会话行读取）。
   * 后端"比较后再写"：`expected*` 与库中不一致时不覆盖，返回 false 并把当前值同步回本地。
   * 时间列不动（`updated_at` 只表示最后消息时间）。
   */
  updateSessionModel: (id: string, apiProvider: string, apiModel: string) => Promise<boolean>;
  archiveSession: (id: string) => Promise<void>;
  unarchiveSession: (id: string) => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  toggleArchived: () => void;
  addMessage: (msg: Message) => void;
  updateMessage: (id: string, content: string) => Promise<void>;
  /**
   * 会话 → 工作流：导出转换后的定义与候选清单（仅预览，不落库）。
   * `tasks` 为 null/省略时由后端决定来源（会话计划 → 退化为用户消息）；
   * 传数组时表示"用户在前端确认后的清单"，后端按此组装（source = edited）。
   * `name`/`description` 传了就按传入值生成定义，传 null 用后端默认。
   * 失败时 reject，错误文案可直接展示给用户。
   */
  exportWorkflow: (
    sessionId: string,
    tasks?: PlanTaskInput[] | null,
    name?: string | null,
    description?: string | null,
  ) => Promise<WorkflowExportResult>;
  /** 会话 → 工作流：落库为可运行定义（成功时返回其 id/name，供提示与跳转）；tasks/name/description 口径同上 */
  promoteWorkflow: (
    sessionId: string,
    tasks?: PlanTaskInput[] | null,
    name?: string | null,
    description?: string | null,
  ) => Promise<{ id: string; name: string }>;
  /** 会话 → 群聊：导出转换后的房间预览（议题 + 候选任务清单；仅预览，不落库）；tasks 口径同上 */
  exportRoomPlan: (sessionId: string, tasks?: PlanTaskInput[] | null) => Promise<RoomPlan>;
  /**
   * 会话 → 任务清单（**模型提炼**）：由后端发起一次专用模型调用（不进会话对话），
   * 返回模型整理出的候选清单（origin 为 'ai'、suggested 为 true）。
   * 与「来源清单」互斥：前端二选一展示。失败时 reject，错误文案可直接展示给用户。
   */
  extractTasks: (sessionId: string, target?: 'workflow' | 'room') => Promise<{ tasks: PlanCandidate[] }>;
  /** 会话 → 群聊：创建房间并把任务清单落库（成功时返回房间 id 与任务数，供提示与跳转） */
  promoteToRoom: (
    sessionId: string,
    input: unknown,
    assignee: string | null,
    tasks?: PlanTaskInput[] | null,
  ) => Promise<{ roomId: string; taskCount: number }>;
}

/** 持久化消息到 SQLite（fire-and-forget） */
async function persistMessage(msg: Message): Promise<void> {
  try {
    await invoke<Message>('save_message', {
      sessionId: msg.sessionId,
      role: msg.role,
      content: msg.content,
      mode: msg.mode,
      toolCalls: msg.toolCalls ?? null,
      toolCallId: msg.toolCallId ?? null,
      toolName: msg.toolName ?? null,
      attachments: msg.attachments ?? null,
    });
    // 不再调用 fetchSessions()，避免每次消息持久化时刷新整个会话列表
    // 会话预览通过其他机制更新（如 selectSession 时加载最新数据）
  } catch (err) {
    console.error('Failed to persist message:', err);
  }
}

export const useSessionStore = create<SessionState>((set, get) => ({
  sessions: [],
  archivedSessions: [],
  currentSessionId: null,
  messages: [],
  messageIds: new Set(),
  isLoadingSessions: false,
  isLoadingMessages: false,
  showArchived: false,

  fetchSessions: async () => {
    set({ isLoadingSessions: true });
    try {
      const sessions = await invoke<Session[]>('list_sessions');
      const archivedSessions = await invoke<Session[]>('list_archived_sessions');
      set({ sessions, archivedSessions });
    } catch (err) {
      console.error('Failed to fetch sessions:', err);
    } finally {
      set({ isLoadingSessions: false });
    }
  },

  /** 静默刷新：仅更新数据，不触碰 isLoadingSessions，不导致列表 DOM 替换 */
  refreshSessions: async () => {
    try {
      const [sessions, archivedSessions] = await Promise.all([
        invoke<Session[]>('list_sessions'),
        invoke<Session[]>('list_archived_sessions'),
      ]);
      set({ sessions, archivedSessions });
    } catch (err) {
      console.error('Failed to refresh sessions:', err);
    }
  },

  selectSession: async (id: string) => {
    set({ currentSessionId: id, isLoadingMessages: true });
    try {
      const messages = await invoke<Message[]>('get_session_messages', {
        sessionId: id,
      });
      set({
        messages,
        messageIds: new Set(messages.map((m) => m.id)),
      });
      // 不再持久化"上次会话"：会话模式启动固定停在初始页（不预选任何会话），
      // 选中由用户完成，恢复上次会话会让用户一进来就落在某个会话里。
    } catch (err) {
      console.error('Failed to load messages:', err);
    } finally {
      set({ isLoadingMessages: false });
    }
  },

  /**
   * 回到「快捷开始」：只清空选中（不建会话、不动任何数据），输入区随即回到
   * "选好目录与会话方式直接发消息"的草稿态，首次发送时才真正建会话。
   */
  startNewSession: () => {
    set({ currentSessionId: null, messages: [], messageIds: new Set() });
  },

  createSession: async (agentType, cwd, title, apiProvider, apiModel, temperature, maxTokens) => {
    const session = await invoke<Session>('create_session', {
      agentType,
      cwd: cwd || null,
      title: title || null,
      apiProvider: apiProvider || null,
      apiModel: apiModel || null,
      temperature: temperature ?? null,
      maxTokens: maxTokens ?? null,
    });
    set((state) => ({
      sessions: [session, ...state.sessions],
      currentSessionId: session.id,
      messages: [],
      messageIds: new Set(),
    }));
    // 会话创建成功后通知插件（失败已由上面 reject，不会走到这里）
    emitHostEvent('session:created', { sessionId: session.id, agentType: session.agentType });
    return session;
  },

  renameSession: async (id, newTitle) => {
    await invoke('rename_session', { sessionId: id, newTitle });
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === id ? { ...s, title: newTitle } : s
      ),
    }));
  },

  /**
   * 会话标题契约：让模型依据**第一条用户消息**起标题（后端"比较后再写"，落库也在后端）。
   * `expectedTitle` 是调用时前端显示的标题（机械截断的占位）——期间被用户改过则后端不覆盖。
   * 不可用（没有可用的 API 提供商）或失败时返回原值，这里不做任何改动。
   */
  suggestSessionTitle: async (id, expectedTitle) => {
    try {
      const title = await invoke<string>('session_suggest_title', {
        sessionId: id,
        expectedTitle,
      });
      if (!title || title === expectedTitle) {
        // 后端保留占位标题时会把原因写进 [Title] 日志（无可用的 API 提供商 / 模型没给出可用标题等）
        console.info('[sessionStore] 会话标题未被替换，保留占位:', { expectedTitle, title });
        return;
      }
      set((state) => ({
        sessions: state.sessions.map((s) => (s.id === id ? { ...s, title } : s)),
        archivedSessions: state.archivedSessions.map((s) => (s.id === id ? { ...s, title } : s)),
      }));
    } catch (err) {
      // 标题只是辅助信息：生成失败就保留机械标题，不打扰用户
      console.warn('[sessionStore] 生成会话标题失败:', err);
    }
  },

  updateSessionCwd: async (id, cwd) => {
    await invoke('update_session_cwd', { sessionId: id, cwd });
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === id ? { ...s, cwd } : s
      ),
    }));
  },

  updateSessionModel: async (id, apiProvider, apiModel) => {
    // 用 create 回调里的 get() 而不是 useSessionStore.getState()：后者让 store 的类型
    // 依赖自己的初始化器（TS7022 循环推导），会连着把 this 里所有回调参数变成 implicit any。
    const current = get().sessions.find((s) => s.id === id);
    const res = await invoke<{ applied: boolean; apiProvider: string; apiModel: string }>(
      'update_session_model',
      {
        sessionId: id,
        apiProvider,
        apiModel,
        expectedProvider: current?.apiProvider ?? '',
        expectedModel: current?.apiModel ?? '',
      },
    );
    // 无论是否写入，都按后端回传的当前值对齐本地（CAS 未命中时这一步就是"纠正显示"）
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === id ? { ...s, apiProvider: res.apiProvider || undefined, apiModel: res.apiModel || undefined } : s
      ),
    }));
    return res.applied;
  },

  archiveSession: async (id) => {
    const archivedSessions = await invoke<Session[]>('list_archived_sessions');
    await invoke('archive_session', { sessionId: id });
    set((state) => {
      const session = state.sessions.find((s) => s.id === id);
      return {
        sessions: state.sessions.filter((s) => s.id !== id),
        archivedSessions: session
          ? [...archivedSessions, session]
          : archivedSessions,
        currentSessionId:
          state.currentSessionId === id ? null : state.currentSessionId,
        messages: state.currentSessionId === id ? [] : state.messages,
        messageIds: state.currentSessionId === id ? new Set() : state.messageIds,
      };
    });
  },

  unarchiveSession: async (id) => {
    await invoke('unarchive_session', { sessionId: id });
    set((state) => {
      const session = state.archivedSessions.find((s) => s.id === id);
      if (!session) return {};
      // list_sessions 按 updated_at DESC 排序，而 unarchive_session 刚把 updated_at 刷成当前时间，
      // 所以移回活动列表时应置于最前（等价于按 updated_at 重排，只是不必重新拉全量列表）。
      const restored = { ...session, updatedAt: Math.floor(Date.now() / 1000) };
      return {
        sessions: [restored, ...state.sessions],
        archivedSessions: state.archivedSessions.filter((s) => s.id !== id),
      };
    });
  },

  deleteSession: async (id) => {
    await invoke('delete_session', { sessionId: id });
    set((state) => ({
      sessions: state.sessions.filter((s) => s.id !== id),
      archivedSessions: state.archivedSessions.filter((s) => s.id !== id),
      currentSessionId:
        state.currentSessionId === id ? null : state.currentSessionId,
      messages: state.currentSessionId === id ? [] : state.messages,
      messageIds: state.currentSessionId === id ? new Set() : state.messageIds,
    }));
    // 会话删除成功后通知插件（invoke 失败会 reject，不会走到这里）
    emitHostEvent('session:deleted', { sessionId: id });
  },

  toggleArchived: () => {
    set((state) => ({ showArchived: !state.showArchived }));
  },

  updateMessage: async (id: string, content: string) => {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const updated = await invoke<Message>('update_message', { messageId: id, content });
      set((state) => ({
        messages: state.messages.map((m) => m.id === id ? updated : m),
      }));
    } catch (err) {
      console.error('Failed to update message:', err);
    }
  },

  /**
   * 会话 → 工作流：导出转换后的定义与候选清单（与落库所用定义逐字一致，仅供预览）。
   * 失败时向上抛出：错误文案来自后端，可直接作为 Toast/弹窗文案展示。
   */
  exportWorkflow: async (
    sessionId: string,
    tasks?: PlanTaskInput[] | null,
    name?: string | null,
    description?: string | null,
  ) => {
    try {
      return await invoke<WorkflowExportResult>('session_export_workflow', {
        sessionId,
        tasks: tasks ?? null,
        name: name ?? null,
        description: description ?? null,
      });
    } catch (err) {
      console.error('Failed to export workflow from session:', err);
      throw err;
    }
  },

  /** 会话 → 工作流：落库为可运行定义；这里只取 id/name（调用方用于提示与跳转编辑器） */
  promoteWorkflow: async (
    sessionId: string,
    tasks?: PlanTaskInput[] | null,
    name?: string | null,
    description?: string | null,
  ) => {
    try {
      const def = await invoke<WorkflowDefinition>('session_promote_workflow', {
        sessionId,
        tasks: tasks ?? null,
        name: name ?? null,
        description: description ?? null,
      });
      return { id: def.id, name: def.name };
    } catch (err) {
      console.error('Failed to promote session to workflow:', err);
      throw err;
    }
  },

  /**
   * 会话 → 群聊：导出将要创建的房间（议题 + 候选任务清单），仅供预览。
   * 失败时向上抛出：错误文案来自后端（如"还没有可转换的计划"），可直接展示。
   */
  exportRoomPlan: async (sessionId: string, tasks?: PlanTaskInput[] | null) => {
    try {
      return await invoke<RoomPlan>('session_export_room_plan', {
        sessionId,
        tasks: tasks ?? null,
      });
    } catch (err) {
      console.error('Failed to export room plan from session:', err);
      throw err;
    }
  },

  /**
   * 会话 → 任务清单（模型提炼）：专用调用，失败时向上抛出（中文文案可直接展示）。
   * `target` 决定后端提示词与确认类任务口径（工作流的 Agent 不能与用户交互 / 群聊主持人可以），
   * 省略时后端按 'workflow' 处理。
   */
  extractTasks: async (sessionId: string, target?: 'workflow' | 'room') => {
    try {
      return await invoke<{ tasks: PlanCandidate[] }>('session_extract_tasks', { sessionId, target });
    } catch (err) {
      console.error('Failed to extract tasks from session:', err);
      throw err;
    }
  },

  /** 会话 → 群聊：创建房间 + 参与者 + 任务；这里只取 roomId/taskCount（调用方用于提示与跳转） */
  promoteToRoom: async (sessionId, input, assignee, tasks?: PlanTaskInput[] | null) => {
    try {
      return await invoke<{ roomId: string; taskCount: number }>('session_promote_to_room', {
        sessionId,
        input,
        assignee,
        tasks: tasks ?? null,
      });
    } catch (err) {
      console.error('Failed to promote session to room:', err);
      throw err;
    }
  },

  addMessage: (msg: Message) => {
    // ID-based dedup（可靠，无时间窗口问题）
    const state = get();
    if (state.messageIds.has(msg.id)) return;

    // 计算预览文本（取前 100 个字符；按码点计，避免把 emoji 切成半个）
    const preview = elide(msg.content, 100, '...');

    // 首条用户消息时，自动更新会话标题为消息摘要
    const session = state.sessions.find(s => s.id === msg.sessionId);
    const isFirstUserMessage = msg.role === 'user' && session && session.messageCount === 0 && !session.title;

    // 标题契约的触发点：首轮**结束后**的第一条消息（assistant 回复 / system 报错，即"这轮有结果了"）。
    // 不在发送瞬间调用：那是与主请求并发的第二个请求，免费/限流档位的提供商会直接 429
    //（实测 Agnes AI 免费档），标题永远出不来且失败无声。闸门由 pendingModelTitle 一次性关闭。
    const suggestTitleFor = msg.role !== 'user' && session
      && pendingModelTitle.has(session.id) && !isWorkflowSession(session.origin)
      ? session.id
      : null;
    if (suggestTitleFor) pendingModelTitle.delete(suggestTitleFor);
    if (isFirstUserMessage) pendingModelTitle.add(msg.sessionId);

    // ── 始终更新目标会话的 lastMessagePreview（修复"每个会话都显示同一内容"）──
    // 之前只在 currentSession 时更新，导致后台会话预览永远停留在初始加载值
    set((state) => {
      let updatedSessions = state.sessions.map((s) =>
        s.id === msg.sessionId
          ? { ...s, lastMessagePreview: preview, messageCount: s.messageCount + 1 }
          : s
      );

      // 更新标题（首条用户消息）：先落"机械截断"做即时占位（同步，列表立刻有标题），
      // 模型语义标题由下方异步段补齐。
      if (isFirstUserMessage) {
        const titlePreview = elide(msg.content, 30, '...');
        updatedSessions = updatedSessions.map((s) =>
          s.id === msg.sessionId ? { ...s, title: titlePreview } : s
        );
      }

      // 仅当前会话才追加到 messages 列表
      const isCurrent = msg.sessionId === state.currentSessionId;
      return {
        sessions: updatedSessions,
        messageIds: new Set(state.messageIds).add(msg.id),
        ...(isCurrent ? { messages: [...state.messages, msg] } : {}),
      };
    });

    // 消息已发出并写入会话之后，通知插件。
    // 载荷只给标识（不含正文）；role 区分 user / assistant / system / tool。
    // 投递是纯通知：不 await、不看返回值，插件的处理结果与异常都不影响消息流程。
    emitHostEvent('message:sent', { sessionId: msg.sessionId, messageId: msg.id, role: msg.role });

    // 异步持久化落库（标题契约依赖库里的第一条用户消息，必须等它写完）
    void (async () => {
      await persistMessage(msg);

      if (isFirstUserMessage) {
        const titlePreview = elide(msg.content, 30, '...');
        try {
          await invoke('rename_session', { sessionId: msg.sessionId, newTitle: titlePreview });
        } catch (err) {
          console.warn('[sessionStore] 写入占位标题失败:', err);
        }
      }

      if (suggestTitleFor) {
        // 期望值 = 当前显示的占位标题：期间被用户改过则后端不覆盖（比较后再写）
        const expectedTitle = get().sessions
          .find((s) => s.id === suggestTitleFor)?.title ?? '';
        await get().suggestSessionTitle(suggestTitleFor, expectedTitle);
      }
    })();
  },
}));
