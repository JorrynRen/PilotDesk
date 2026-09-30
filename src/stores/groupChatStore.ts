import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  GroupChatRoom,
  GroupChatParticipant,
  GroupChatParticipantInput,
  GroupChatMessage,
  GroupChatStance,
  GroupChatTask,
  GroupChatEvent,
  GroupChatToolCall,
  CreateGroupChatRoomInput,
  GroupChatConfirmationResponseInput,
  GroupChatDepChangePreview,
} from '../types/groupchat';
import type { Attachment } from '../types';
import { errorMessage } from '../utils/errorMessage';

/** 群聊消息分页大小（首屏 + 每次向上加载） */
const GC_PAGE_SIZE = 100;

/** 后端 groupchat_get_messages 的分页返回结构 */
interface MessagePage {
  messages: GroupChatMessage[];
  total: number;
}

interface GroupChatStoreState {
  rooms: GroupChatRoom[];
  currentRoomId: string | null;
  participants: GroupChatParticipant[];
  messages: GroupChatMessage[];
  stances: GroupChatStance[];
  tasks: GroupChatTask[];
  /** 发言中 token_stream 增量缓存（speaker → 累计文本），完整 message 到达后清除 */
  streaming: Record<string, string>;
  /** 当前发言者（floor_granted） */
  currentSpeaker: string | null;
  /** 当前轮次（floor_granted 携带） */
  currentRound: number;
  /** 参与者工具调用实时状态（participantId → 调用链），会话结束后清空 */
  toolCalls: Record<string, GroupChatToolCall[]>;
  /** 参与者实时推理文本（participantId → 累积内容，agent-reasoning 事件驱动），发言落库后清除 */
  reasoning: Record<string, string>;
  /** 房间消息投影水位：最近已应用的 message seq（roomId → seq）。用于丢弃重复/乱序/陈旧补发的 message 事件。 */
  messageSeqWatermarks: Record<string, number>;
  loading: boolean;
  /** 切房间拉取消息期间的加载态（驱动消息区骨架/loading 反馈） */
  messagesLoading: boolean;
  /** 是否还有更早消息可向上加载 */
  hasMoreMessages: boolean;
  /** 正在向上加载更早消息 */
  loadingEarlier: boolean;
  /** 房间消息总数（用于虚拟列表 firstItemIndex 计算） */
  totalMessages: number;
  error: string | null;
  /** 当前房间 Actor（进程内运行实例）是否存活。用于区分"真 running/在跑"与
   * "假 running"（DB 仍标 running 但进程内无 Actor，如异常退出后未恢复）——后者界面应提供"继续/恢复"入口。 */
  actorAlive: boolean;
  /** 房间列表各房间 Actor 存活映射（roomId → bool），供左侧列表状态点按真实运行态点亮。 */
  roomsActorAlive: Record<string, boolean>;

  loadRooms: () => Promise<void>;
  selectRoom: (roomId: string) => Promise<void>;
  refreshActorAlive: (roomId: string) => Promise<void>;
  loadEarlierMessages: () => Promise<void>;
  createRoom: (input: CreateGroupChatRoomInput) => Promise<GroupChatRoom>;
  deleteRoom: (roomId: string) => Promise<void>;
  addParticipant: (input: GroupChatParticipantInput) => Promise<void>;
  removeParticipant: (participantId: string) => Promise<void>;
  sendMessage: (content: string, attachments?: Attachment[], mention?: string | null, recipients?: string[], securityMode?: string) => Promise<void>;
  respondConfirmation: (requestId: string, responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
  pause: () => Promise<void>;
  resume: () => Promise<void>;
  abort: () => Promise<void>;
  setDirector: (newDirectorId: string) => Promise<void>;
  setRoomOutputDir: (outputDir: string) => Promise<void>;
  /** 任务级人工干预（跳过 / 追加新增 / 改依赖）。写库即对调度生效，变更由 task_updated 事件回流。 */
  skipTask: (taskId: string) => Promise<void>;
  addTask: (description: string, dependsOn: string[], assignee?: string | null) => Promise<void>;
  updateTaskDeps: (taskId: string, dependsOn: string[]) => Promise<void>;
  /** 依赖变更预览（纯计算，不落库）：确认卡展示"改完会阻塞/解锁谁、是否成环" */
  previewTaskDeps: (taskId: string, dependsOn: string[]) => Promise<GroupChatDepChangePreview>;
  /** 精准转换：把任务议程落库为可直接运行的工作流定义（返回新定义，供跳转编辑器）。 */
  promoteWorkflow: () => Promise<{ id: string; name: string }>;
  exportWorkflow: () => Promise<Record<string, unknown>>;
  applyEvent: (event: GroupChatEvent) => void;
}

let unlisten: UnlistenFn | null = null;
let unlistenPromise: Promise<UnlistenFn> | null = null;

export const useGroupChatStore = create<GroupChatStoreState>((set, get) => ({
  rooms: [],
  currentRoomId: null,
  participants: [],
  messages: [],
  stances: [],
  tasks: [],
  streaming: {},
  currentSpeaker: null,
  currentRound: 0,
  toolCalls: {},
  reasoning: {},
  messageSeqWatermarks: {},
  loading: false,
  messagesLoading: false,
  hasMoreMessages: false,
  loadingEarlier: false,
  totalMessages: 0,
  error: null,
  actorAlive: false,
  roomsActorAlive: {},

  loadRooms: async () => {
    set({ loading: true, error: null });
    try {
      const rooms = await invoke<GroupChatRoom[]>('groupchat_list_rooms');
      set({ rooms, loading: false });
      if (get().currentRoomId) {
        void get().refreshActorAlive(get().currentRoomId!);
      }
      // 对 running 房间逐一探测 Actor 存活，供列表状态点区分真/假运行（不点亮假 running 的脉冲）。
      const runningIds = rooms.filter((r) => r.status === 'running').map((r) => r.id);
      if (runningIds.length > 0) {
        void Promise.all(
          runningIds.map(async (id) => {
            try {
              return [id, await invoke<boolean>('groupchat_room_actor_alive', { roomId: id })] as const;
            } catch {
              return [id, false] as const;
            }
          })
        ).then((entries) => {
          set((s) => ({
            roomsActorAlive: {
              ...s.roomsActorAlive,
              ...Object.fromEntries(entries),
            },
          }));
        });
      }
    } catch (err) {
      set({ error: errorMessage(err), loading: false });
    }
  },

  selectRoom: async (roomId) => {
    // 空 roomId = 关闭当前会话实例（回到默认页）：仅清空本地选中与数据，不发后端请求。
    if (!roomId) {
      set({
        currentRoomId: '',
        participants: [],
        messages: [],
        stances: [],
        tasks: [],
        streaming: {},
        currentSpeaker: null,
        currentRound: 0,
        toolCalls: {},
        reasoning: {},
        actorAlive: false,
        messagesLoading: false,
        hasMoreMessages: false,
        loadingEarlier: false,
        totalMessages: 0,
      });
      return;
    }
    set({
      currentRoomId: roomId,
      participants: [],
      messages: [],
      stances: [],
      tasks: [],
      streaming: {},
      currentSpeaker: null,
      currentRound: 0,
      toolCalls: {},
      reasoning: {},
      actorAlive: false,
      messagesLoading: true,
      hasMoreMessages: false,
      loadingEarlier: false,
      totalMessages: 0,
    });
    try {
      const [participants, page, stances, tasks, messageWatermark] = await Promise.all([
        invoke<GroupChatParticipant[]>('groupchat_get_participants', { roomId }),
        invoke<MessagePage>('groupchat_get_messages', { roomId, beforeSeq: null, limit: GC_PAGE_SIZE }),
        invoke<GroupChatStance[]>('groupchat_get_stances', { roomId }),
        invoke<GroupChatTask[]>('groupchat_get_tasks', { roomId }),
        invoke<number>('groupchat_message_watermark', { roomId }),
      ]);
      // 重新进入房间时，从已落库消息恢复当前轮次（用户指令为第 0 轮，参与者发言逐轮递增）。
      const maxRound = page.messages.reduce((m, msg) => Math.max(m, msg.round), 0);
      // 投影水位阶段③：整页加载后把房间水位同步到"已落库 message 事件最大 seq"，
      // 使加载前可能重复派发/乱序到达的 message 事件在 applyEvent 中按水位直接丢弃。
      const prevWm = get().messageSeqWatermarks[roomId] ?? 0;
      set({
        participants,
        messages: page.messages,
        stances,
        tasks,
        currentRound: maxRound,
        messagesLoading: false,
        hasMoreMessages: page.messages.length < page.total,
        totalMessages: page.total,
        messageSeqWatermarks: {
          ...get().messageSeqWatermarks,
          [roomId]: Math.max(prevWm, messageWatermark),
        },
      });
    } catch (err) {
      set({ error: errorMessage(err), messagesLoading: false, actorAlive: false });
      return;
    }
    // 探测当前房间 Actor 是否在进程内存活（区分真/假 running）。
    await get().refreshActorAlive(roomId);
  },

  refreshActorAlive: async (roomId) => {
    try {
      const alive = await invoke<boolean>('groupchat_room_actor_alive', { roomId });
      set({ actorAlive: alive });
    } catch {
      set({ actorAlive: false });
    }
  },

  loadEarlierMessages: async () => {
    const { currentRoomId, messages, loadingEarlier, hasMoreMessages } = get();
    if (!currentRoomId || loadingEarlier || !hasMoreMessages) return;
    const oldestSeq = messages.reduce((min, m) => Math.min(min, m.seq), Number.MAX_SAFE_INTEGER);
    set({ loadingEarlier: true });
    try {
      const page = await invoke<MessagePage>('groupchat_get_messages', {
        roomId: currentRoomId,
        beforeSeq: oldestSeq,
        limit: GC_PAGE_SIZE,
      });
      const existingIds = new Set(messages.map((m) => m.id));
      const earlier = page.messages.filter((m) => !existingIds.has(m.id));
      set({
        // earlier 按 seq 升序且均早于当前最早消息，前插后整体仍保持升序。
        messages: [...earlier, ...messages],
        hasMoreMessages: messages.length + earlier.length < page.total,
        totalMessages: page.total,
        loadingEarlier: false,
      });
    } catch (err) {
      set({ error: errorMessage(err), loadingEarlier: false });
    }
  },

  createRoom: async (input) => {
    set({ error: null });
    const room = await invoke<GroupChatRoom>('groupchat_create_room', { input });
    await get().loadRooms();
    return room;
  },

  deleteRoom: async (roomId) => {
    set({ error: null });
    await invoke('groupchat_delete_room', { roomId });
    if (get().currentRoomId === roomId) {
      set({
        currentRoomId: null,
        participants: [],
        messages: [],
        stances: [],
        tasks: [],
        streaming: {},
        currentSpeaker: null,
        currentRound: 0,
        toolCalls: {},
        reasoning: {},
        messagesLoading: false,
        hasMoreMessages: false,
        loadingEarlier: false,
        totalMessages: 0,
      });
    }
    await get().loadRooms();
  },

  addParticipant: async (input) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    set({ error: null });
    await invoke('groupchat_join_room', { roomId, participant: input });
    await get().selectRoom(roomId);
  },

  removeParticipant: async (participantId) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    set({ error: null });
    await invoke('groupchat_remove_participant', { roomId, participantId });
    await get().selectRoom(roomId);
  },

  sendMessage: async (content, attachments, mention, recipients, securityMode) => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    set({ error: null });
    try {
      await invoke('groupchat_send_message', {
        input: {
          roomId,
          content,
          recipients: recipients ?? [],
          replyTo: null,
          mention: mention ?? null,
          attachments: attachments ?? [],
          securityMode: securityMode || 'standard',
        },
      });
    } catch (err) {
      set({ error: errorMessage(err) });
      throw err;
    }
  },

  respondConfirmation: async (requestId: string, responses: GroupChatConfirmationResponseInput[]) => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    set({ error: null });
    try {
      await invoke('groupchat_respond_confirmation', {
        input: { roomId, requestId, responses },
      });
    } catch (err) {
      set({ error: errorMessage(err) });
      throw err;
    }
  },

  pause: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    const room = await invoke<GroupChatRoom>('groupchat_pause', { roomId });
    set((s) => ({
      rooms: s.rooms.map((r) => (r.id === room.id ? { ...r, status: room.status } : r)),
    }));
  },

  resume: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    const room = await invoke<GroupChatRoom>('groupchat_resume', { roomId });
    set((s) => ({
      rooms: s.rooms.map((r) => (r.id === room.id ? { ...r, status: room.status } : r)),
    }));
  },

  abort: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    const room = await invoke<GroupChatRoom>('groupchat_abort', { roomId });
    set((s) => ({
      rooms: s.rooms.map((r) => (r.id === room.id ? { ...r, status: room.status } : r)),
    }));
  },

  setDirector: async (newDirectorId) => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    await invoke('groupchat_set_director', { roomId, newDirectorId });
  },

  setRoomOutputDir: async (outputDir: string) => {
    const roomId = get().currentRoomId;
    if (!roomId) return;
    const room = await invoke<GroupChatRoom>('groupchat_set_output_dir', { roomId, outputDir });
    set((s) => ({
      rooms: s.rooms.map((r) => (r.id === roomId ? { ...r, outputDir: room.outputDir } : r)),
    }));
  },

  exportWorkflow: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    return invoke<Record<string, unknown>>('groupchat_export_workflow', { roomId });
  },

  skipTask: async (taskId) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    await invoke<GroupChatTask>('groupchat_task_skip', { roomId, taskId });
  },

  addTask: async (description, dependsOn, assignee) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    await invoke<GroupChatTask>('groupchat_task_add', {
      input: { roomId, description, dependsOn, assignee: assignee ?? null },
    });
  },

  updateTaskDeps: async (taskId, dependsOn) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    await invoke<GroupChatTask>('groupchat_task_update_deps', { roomId, taskId, dependsOn });
  },

  previewTaskDeps: async (taskId, dependsOn) => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    return invoke<GroupChatDepChangePreview>('groupchat_task_deps_preview', { roomId, taskId, dependsOn });
  },

  promoteWorkflow: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    const def = await invoke<{ id: string; name: string }>('groupchat_promote_workflow', { roomId });
    return { id: def.id, name: def.name };
  },

  applyEvent: (event) => {
    const { currentRoomId } = get();
    const isCurrent = event.roomId === currentRoomId;

    // 房间级事件：无论是否当前房间，都同步房间列表状态。
    switch (event.type) {
      case 'room_status': {
        // 暂停/停止时同步清理流式状态：后端已停止推送 token_stream（抑制流式），
        // 前端同时清空当前发言者与增量缓存，避免"点了停止却还在输出"的错觉。
        const stopped = event.status === 'paused' || event.status === 'aborted';
        set((s) => ({
          rooms: s.rooms.map((r) => (r.id === event.roomId ? { ...r, status: event.status } : r)),
          ...(stopped && s.currentRoomId === event.roomId
            ? { currentSpeaker: null, streaming: {}, toolCalls: {} }
            : {}),
          // Actor 正在推送事件 → 该房间 Actor 存活（真 running；含 paused/aborted 均活）。
          actorAlive: s.currentRoomId === event.roomId ? true : s.actorAlive,
          roomsActorAlive: { ...s.roomsActorAlive, [event.roomId]: true },
        }));
        break;
      }
      case 'finished': {
        set((s) => ({
          rooms: s.rooms.map((r) =>
            r.id === event.roomId ? { ...r, status: 'finished' as const } : r,
          ),
        }));
        break;
      }
      case 'output_dir_updated': {
        // 产物目录已在目标理解阶段确定/变更：同步房间列表，右侧面板动态显示当前值。
        set((s) => ({
          rooms: s.rooms.map((r) =>
            r.id === event.roomId ? { ...r, outputDir: event.outputDir } : r,
          ),
        }));
        break;
      }
      default:
        break;
    }

    // 会话级事件：仅作用于当前选中的房间。
    if (!isCurrent) return;

    switch (event.type) {
      case 'floor_granted': {
        set({ currentSpeaker: event.speaker, currentRound: event.round });
        break;
      }
      case 'token_stream': {
        set((s) => ({
          streaming: {
            ...s.streaming,
            [event.speaker]: (s.streaming[event.speaker] ?? '') + event.delta,
          },
        }));
        break;
      }
      case 'message': {
        const roomSeq = event.message?.seq ?? 0;
        const prevWm = get().messageSeqWatermarks[event.roomId] ?? 0;
        // 事件投影水位（阶段②）：seq 单调递增。收到"严格早于已应用水位"的 message
        // 事件是重复/乱序/陈旧补发 → 丢弃。相等（seq == 水位）可能是快照与实时竞态下
        // 同一条刚落库的消息，交由下方 id 去重兜底，避免水位把新消息误吞。
        if (roomSeq > 0 && prevWm > 0 && roomSeq < prevWm) break;
        set((s) => {
          const exists = s.messages.some((m) => m.id === event.message.id);
          const nextStreaming = { ...s.streaming };
          delete nextStreaming[event.message.sender];
          // 参与者本段发言/执行结果已落库，其临时工具调用链与推理文本一并清空（避免残留）。
          const nextToolCalls = { ...s.toolCalls };
          delete nextToolCalls[event.message.sender];
          const nextReasoning = { ...s.reasoning };
          delete nextReasoning[event.message.sender];
          return {
            messages: exists ? s.messages : [...s.messages, event.message],
            totalMessages: exists ? s.totalMessages : s.totalMessages + 1,
            streaming: nextStreaming,
            toolCalls: nextToolCalls,
            reasoning: nextReasoning,
            currentSpeaker: null,
            messageSeqWatermarks: {
              ...s.messageSeqWatermarks,
              [event.roomId]: Math.max(prevWm, roomSeq),
            },
          };
        });
        break;
      }
      case 'stance_updated': {
        set((s) => {
          const others = s.stances.filter((st) => st.participantId !== event.participantId);
          return {
            stances: [
              ...others,
              {
                roomId: event.roomId,
                participantId: event.participantId,
                stance: event.stance,
                attitude: event.attitude,
                updatedAt: Date.now(),
              },
            ],
          };
        });
        break;
      }
      case 'role_updated': {
        set((s) => ({
          participants: s.participants.map((p) =>
            p.id === event.participantId ? { ...p, systemRole: event.role } : p,
          ),
        }));
        break;
      }
      case 'participant_updated': {
        // 主持人按需自动补充参与者后刷新名册（异步拉取，用户无需任何操作）。
        // 消息流中的 [@id] 提及随后渲染为 @显示名（participants 更新触发重渲染）。
        void (async () => {
          try {
            const participants = await invoke<GroupChatParticipant[]>('groupchat_get_participants', {
              roomId: event.roomId,
            });
            set({ participants });
          } catch {
            // 忽略：重新进入房间（selectRoom）时仍会全量刷新。
          }
        })();
        break;
      }
      case 'task_updated': {
        set((s) => {
          const exists = s.tasks.some((t) => t.id === event.task.id);
          return {
            tasks: exists
              ? s.tasks.map((t) => (t.id === event.task.id ? event.task : t))
              : [...s.tasks, event.task],
          };
        });
        break;
      }
      case 'finished': {
        set(() => ({
          currentSpeaker: null,
          streaming: {},
          toolCalls: {},
          reasoning: {},
        }));
        break;
      }
      case 'started': {
        break;
      }
    }
  },
}));

/** 解析群聊 API 参与者的事件 sessionId（形如 groupchat:{roomId}:{participantId}）。 */
function parseGroupChatSessionId(sessionId: string): { roomId: string; participantId: string } | null {
  if (!sessionId.startsWith('groupchat:')) return null;
  const rest = sessionId.slice('groupchat:'.length);
  const idx = rest.indexOf(':');
  if (idx < 0) return null;
  return { roomId: rest.slice(0, idx), participantId: rest.slice(idx + 1) };
}

function applyGroupChatToolStart(sessionId: string, toolId: string, toolName: string, toolArgs: string) {
  const parsed = parseGroupChatSessionId(sessionId);
  if (!parsed) return;
  const { currentRoomId } = useGroupChatStore.getState();
  if (parsed.roomId !== currentRoomId) return;
  useGroupChatStore.setState((s) => {
    const list = s.toolCalls[parsed.participantId] ?? [];
    if (list.some((t) => t.toolId === toolId)) return s;
    return {
      toolCalls: {
        ...s.toolCalls,
        [parsed.participantId]: [
          ...list,
          { toolId, toolName, arguments: toolArgs, status: 'running' as const },
        ],
      },
    };
  });
}

function applyGroupChatToolResult(
  sessionId: string,
  toolId: string,
  toolName: string,
  result: string,
  success: boolean,
) {
  const parsed = parseGroupChatSessionId(sessionId);
  if (!parsed) return;
  const { currentRoomId } = useGroupChatStore.getState();
  if (parsed.roomId !== currentRoomId) return;
  useGroupChatStore.setState((s) => {
    const list = s.toolCalls[parsed.participantId] ?? [];
    return {
      toolCalls: {
        ...s.toolCalls,
        [parsed.participantId]: list.map((t) =>
          t.toolId === toolId ? { ...t, status: 'done' as const, result, success } : t,
        ),
      },
    };
  });
}

/** 复用会话模式的 agent-reasoning 事件，按 groupchat sessionId 过滤，累积参与者实时推理文本。 */
function applyGroupChatReasoning(sessionId: string, content: string) {
  const parsed = parseGroupChatSessionId(sessionId);
  if (!parsed) return;
  const { currentRoomId } = useGroupChatStore.getState();
  if (parsed.roomId !== currentRoomId) return;
  useGroupChatStore.setState((s) => ({
    reasoning: {
      ...s.reasoning,
      [parsed.participantId]: (s.reasoning[parsed.participantId] ?? '') + content,
    },
  }));
}

/** 注册全局事件监听（App 挂载时调用一次；模块热更新后随新 store 重建），按 roomId 过滤分发到 store。 */
export async function subscribeGroupChat(): Promise<UnlistenFn> {
  if (unlisten) return unlisten;
  if (unlistenPromise) return unlistenPromise;
  unlistenPromise = (async () => {
    const offGroupChat = await listen<GroupChatEvent>('groupchat-event', (raw) => {
      useGroupChatStore.getState().applyEvent(raw.payload);
    });
    // 复用会话模式的 agent-tool-start / agent-tool-result，按 groupchat 事件 sessionId 过滤，
    // 把参与者的工具调用过程实时暴露到前端，避免执行阶段无视觉反馈。
    const offToolStart = await listen<{ sessionId: string; toolId: string; toolName: string; arguments: string }>(
      'agent-tool-start',
      (raw) => {
        const p = raw.payload;
        applyGroupChatToolStart(p.sessionId, p.toolId, p.toolName, p.arguments);
      },
    );
    const offToolResult = await listen<{ sessionId: string; toolId: string; toolName: string; result: string; success: boolean }>(
      'agent-tool-result',
      (raw) => {
        const p = raw.payload;
        applyGroupChatToolResult(p.sessionId, p.toolId, p.toolName, p.result, p.success);
      },
    );
    // 复用会话模式的 agent-reasoning 事件，展示参与者推理步骤（不落库，仅实时）。
    const offReasoning = await listen<{ sessionId: string; content: string }>('agent-reasoning', (raw) => {
      const p = raw.payload;
      applyGroupChatReasoning(p.sessionId, p.content);
    });
    return () => {
      offGroupChat();
      offToolStart();
      offToolResult();
      offReasoning();
    };
  })().then((fn) => {
    unlisten = fn;
    return fn;
  });
  // 任一 listen 失败时清空单例标记，允许后续再次调用时重新注册；
  // 否则被拒绝的 promise 会永久阻塞监听重建，导致实时事件彻底失效。
  unlistenPromise.catch(() => {
    unlisten = null;
    unlistenPromise = null;
  });
  return unlistenPromise;
}

// 模块求值即注册：Vite 热更新（HMR）会重新求值本模块并 create 出新的 store 实例，
// 若仅在 App 挂载时注册一次监听，旧监听会持续把事件写入被废弃的旧 store 实例，
// 表现为「后台在执行、前端不实时更新/严重延后」。此处确保每次重新求值后，
// 事件监听都绑定到最新 store 实例，热更新后即可恢复实时显示。
void subscribeGroupChat();

// 热更新替换旧模块前注销旧监听，避免新旧实例监听叠加导致重复处理。
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    unlisten?.();
    unlisten = null;
    unlistenPromise = null;
  });
}
