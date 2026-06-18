import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Session, Message } from '../types';

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
  selectSession: (id: string) => Promise<void>;
  createSession: (
    agentType: 'claude' | 'hermes' | 'codex' | 'api' | 'codex',
    cwd?: string,
    title?: string | null,
    apiProvider?: string,
    apiModel?: string,
  ) => Promise<Session>;
  renameSession: (id: string, newTitle: string) => Promise<void>;
  archiveSession: (id: string) => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  toggleArchived: () => void;
  addMessage: (msg: Message) => void;
  updateMessage: (id: string, content: string) => Promise<void>;
}

/** 持久化消息到 SQLite（fire-and-forget） */
async function persistMessage(msg: Message): Promise<void> {
  try {
    const saved = await invoke<Message>('save_message', {
      sessionId: msg.sessionId,
      role: msg.role,
      content: msg.content,
      mode: msg.mode,
      reasoningContent: msg.reasoningContent ?? null,
      toolCalls: msg.toolCalls ?? null,
      toolCallId: msg.toolCallId ?? null,
      toolName: msg.toolName ?? null,
    });
    // 更新会话预览
    useSessionStore.getState().fetchSessions();
  } catch (err) {
    console.error('Failed to persist message:', err);
  }
}

export const useSessionStore = create<SessionState>((set) => ({
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
    } catch (err) {
      console.error('Failed to load messages:', err);
    } finally {
      set({ isLoadingMessages: false });
    }
  },

  createSession: async (agentType, cwd, title, apiProvider, apiModel) => {
    const session = await invoke<Session>('create_session', {
      agentType,
      cwd: cwd || null,
      title: title || null,
      apiProvider: apiProvider || null,
      apiModel: apiModel || null,
    });
    set((state) => ({
      sessions: [session, ...state.sessions],
      currentSessionId: session.id,
      messages: [],
      messageIds: new Set(),
    }));
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

  addMessage: (msg: Message) => {
    // ID-based dedup（可靠，无时间窗口问题）
    const state = useSessionStore.getState();
    if (state.messageIds.has(msg.id)) return;

    // 仅当消息属于当前会话时才更新 UI 消息列表
    // 后台会话的消息仅持久化，不污染当前显示
    if (msg.sessionId === state.currentSessionId) {
      set((state) => ({
        messages: [...state.messages, msg],
        messageIds: new Set(state.messageIds).add(msg.id),
      }));
    } else {
      // 非当前会话：仅记录 ID 防重，不更新 UI
      set((state) => ({
        messageIds: new Set(state.messageIds).add(msg.id),
      }));
    }

    // 异步持久化（分离关注点）
    persistMessage(msg);
  },
}));
