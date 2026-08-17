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
} from '../types/groupchat';
import type { Attachment } from '../types';

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

  loadRooms: () => Promise<void>;
  selectRoom: (roomId: string) => Promise<void>;
  loadEarlierMessages: () => Promise<void>;
  createRoom: (input: CreateGroupChatRoomInput) => Promise<GroupChatRoom>;
  deleteRoom: (roomId: string) => Promise<void>;
  addParticipant: (input: GroupChatParticipantInput) => Promise<void>;
  removeParticipant: (participantId: string) => Promise<void>;
  sendMessage: (content: string, attachments?: Attachment[], mention?: string | null, recipients?: string[]) => Promise<void>;
  respondConfirmation: (requestId: string, responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
  pause: () => Promise<void>;
  resume: () => Promise<void>;
  abort: () => Promise<void>;
  setDirector: (newDirectorId: string) => Promise<void>;
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
  loading: false,
  messagesLoading: false,
  hasMoreMessages: false,
  loadingEarlier: false,
  totalMessages: 0,
  error: null,

  loadRooms: async () => {
    set({ loading: true, error: null });
    try {
      const rooms = await invoke<GroupChatRoom[]>('groupchat_list_rooms');
      set({ rooms, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  selectRoom: async (roomId) => {
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
      messagesLoading: true,
      hasMoreMessages: false,
      loadingEarlier: false,
      totalMessages: 0,
    });
    try {
      const [participants, page, stances, tasks] = await Promise.all([
        invoke<GroupChatParticipant[]>('groupchat_get_participants', { roomId }),
        invoke<MessagePage>('groupchat_get_messages', { roomId, beforeSeq: null, limit: GC_PAGE_SIZE }),
        invoke<GroupChatStance[]>('groupchat_get_stances', { roomId }),
        invoke<GroupChatTask[]>('groupchat_get_tasks', { roomId }),
      ]);
      // 重新进入房间时，从已落库消息恢复当前轮次（用户指令为第 0 轮，参与者发言逐轮递增）。
      const maxRound = page.messages.reduce((m, msg) => Math.max(m, msg.round), 0);
      set({
        participants,
        messages: page.messages,
        stances,
        tasks,
        currentRound: maxRound,
        messagesLoading: false,
        hasMoreMessages: page.messages.length < page.total,
        totalMessages: page.total,
      });
    } catch (err) {
      set({ error: String(err), messagesLoading: false });
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
      set({ error: String(err), loadingEarlier: false });
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

  sendMessage: async (content, attachments, mention, recipients) => {
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
        },
      });
    } catch (err) {
      set({ error: String(err) });
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
      set({ error: String(err) });
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

  exportWorkflow: async () => {
    const roomId = get().currentRoomId;
    if (!roomId) throw new Error('未选择房间');
    return invoke<Record<string, unknown>>('groupchat_export_workflow', { roomId });
  },

  applyEvent: (event) => {
    const { currentRoomId } = get();
    const isCurrent = event.roomId === currentRoomId;

    // 房间级事件：无论是否当前房间，都同步房间列表状态。
    switch (event.type) {
      case 'room_status': {
        set((s) => ({
          rooms: s.rooms.map((r) => (r.id === event.roomId ? { ...r, status: event.status } : r)),
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
        set((s) => {
          const exists = s.messages.some((m) => m.id === event.message.id);
          const nextStreaming = { ...s.streaming };
          delete nextStreaming[event.message.sender];
          // 参与者本段发言/执行结果已落库，其临时工具调用链一并清空（避免残留）。
          const nextToolCalls = { ...s.toolCalls };
          delete nextToolCalls[event.message.sender];
          return {
            messages: exists ? s.messages : [...s.messages, event.message],
            totalMessages: exists ? s.totalMessages : s.totalMessages + 1,
            streaming: nextStreaming,
            toolCalls: nextToolCalls,
            currentSpeaker: null,
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
        set((s) => ({
          currentSpeaker: null,
          streaming: {},
          toolCalls: {},
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

/** 注册全局事件监听（App 挂载时调用一次），按 roomId 过滤分发到 store。 */
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
    return () => {
      offGroupChat();
      offToolStart();
      offToolResult();
    };
  })().then((fn) => {
    unlisten = fn;
    return fn;
  });
  return unlistenPromise;
}
