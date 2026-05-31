import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Session, Message } from '../types';

interface SessionState {
  sessions: Session[];
  archivedSessions: Session[];
  currentSessionId: string | null;
  messages: Message[];
  isLoadingSessions: boolean;
  isLoadingMessages: boolean;
  showArchived: boolean;

  fetchSessions: () => Promise<void>;
  selectSession: (id: string) => Promise<void>;
  createSession: (
    agentType: 'claude' | 'hermes' | 'api',
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
}

export const useSessionStore = create<SessionState>((set) => ({
  sessions: [],
  archivedSessions: [],
  currentSessionId: null,
  messages: [],
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
      set({ messages });
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
    await invoke('archive_session', { sessionId: id });
    set((state) => ({
      sessions: state.sessions.filter((s) => s.id !== id),
      currentSessionId:
        state.currentSessionId === id ? null : state.currentSessionId,
      messages: state.currentSessionId === id ? [] : state.messages,
    }));
  },

  deleteSession: async (id) => {
    await invoke('delete_session', { sessionId: id });
    set((state) => ({
      sessions: state.sessions.filter((s) => s.id !== id),
      archivedSessions: state.archivedSessions.filter((s) => s.id !== id),
      currentSessionId:
        state.currentSessionId === id ? null : state.currentSessionId,
      messages: state.currentSessionId === id ? [] : state.messages,
    }));
  },

  toggleArchived: () => {
    set((state) => ({ showArchived: !state.showArchived }));
  },

  addMessage: (msg: Message) => {
    // Persist to database (fire-and-forget, don't block UI)
    invoke<Message>('save_message', {
      sessionId: msg.sessionId,
      role: msg.role,
      content: msg.content,
      mode: msg.mode,
    }).then((saved) => {
      // Update session list with latest preview
      set((state) => ({
        sessions: state.sessions.map((s) =>
          s.id === msg.sessionId
            ? { ...s, lastMessagePreview: saved.content.slice(0, 100), messageCount: s.messageCount + 1, updatedAt: saved.timestamp }
            : s
        ),
      }));
    }).catch((err) => {
      console.error('Failed to persist message:', err);
    });

    // Add to in-memory state immediately for UI responsiveness
    set((state) => ({
      messages: [...state.messages, msg],
    }));
  },
}));
