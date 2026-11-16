import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type {
  MessengerFileView,
  MessengerMemberView,
  MessengerMessageView,
  MessengerSettingsView,
  SendMessengerMessageInput,
} from '../types/enclave';

interface EnclaveMessengerState {
  open: boolean;
  orgId: string;
  peerId: string;
  members: MessengerMemberView[];
  messages: MessengerMessageView[];
  settings: MessengerSettingsView | null;
  loadingMembers: boolean;
  loadingMessages: boolean;
  sending: boolean;
  error: string | null;
  setOpen: (open: boolean) => void;
  openWith: (orgId: string, peerId?: string) => Promise<void>;
  selectPeer: (peerId: string) => Promise<void>;
  loadMembers: (orgId: string) => Promise<void>;
  loadThread: (orgId: string, peerId: string) => Promise<void>;
  loadSettings: (orgId: string) => Promise<void>;
  setRetention: (orgId: string, days: 7 | 30 | 90) => Promise<boolean>;
  send: (orgId: string, input: SendMessengerMessageInput) => Promise<MessengerMessageView | null>;
  sendFile: (
    orgId: string,
    peerId: string,
    fileName: string,
    mime: string,
    dataBase64: string,
  ) => Promise<MessengerMessageView | null>;
  getFile: (orgId: string, fileId: string) => Promise<MessengerFileView | null>;
  recall: (orgId: string, peerId: string, messageId: string) => Promise<MessengerMessageView | null>;
  clearThread: (orgId: string, peerId: string) => Promise<boolean>;
  reset: () => void;
}

export const useEnclaveMessengerStore = create<EnclaveMessengerState>((set, get) => ({
  open: false,
  orgId: '',
  peerId: '',
  members: [],
  messages: [],
  settings: null,
  loadingMembers: false,
  loadingMessages: false,
  sending: false,
  error: null,

  setOpen: (open) => set({ open }),

  openWith: async (orgId, peerId = '') => {
    set({ open: true, orgId, peerId, error: null });
    await Promise.all([get().loadMembers(orgId), get().loadSettings(orgId)]);
    if (peerId) await get().loadThread(orgId, peerId);
    else set({ messages: [] });
  },

  selectPeer: async (peerId) => {
    const orgId = get().orgId;
    set({ peerId });
    if (orgId && peerId) await get().loadThread(orgId, peerId);
  },

  loadMembers: async (orgId) => {
    set({ loadingMembers: true, error: null });
    try {
      const members = await invoke<MessengerMemberView[]>('enclave_messenger_members', { orgId });
      set({ members, loadingMembers: false });
    } catch (e) {
      set({ error: errorMessage(e), loadingMembers: false });
    }
  },

  loadThread: async (orgId, peerId) => {
    set({ loadingMessages: true, error: null });
    try {
      const messages = await invoke<MessengerMessageView[]>('enclave_messenger_messages', {
        orgId,
        peerId,
        limit: 200,
      });
      set((state) => ({
        messages: state.peerId === peerId ? messages : state.messages,
        loadingMessages: false,
      }));
    } catch (e) {
      set({ error: errorMessage(e), loadingMessages: false });
    }
  },

  loadSettings: async (orgId) => {
    try {
      const settings = await invoke<MessengerSettingsView>('enclave_messenger_settings_get', {
        orgId,
      });
      set({ settings });
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  setRetention: async (orgId, days) => {
    try {
      const settings = await invoke<MessengerSettingsView>('enclave_messenger_settings_set', {
        orgId,
        retentionDays: days,
      });
      set({ settings, error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  send: async (orgId, input) => {
    set({ sending: true, error: null });
    try {
      const message = await invoke<MessengerMessageView>('enclave_messenger_send', {
        orgId,
        input,
      });
      set((state) => ({
        messages: state.peerId === input.peerId ? [...state.messages, message] : state.messages,
        sending: false,
      }));
      void get().loadMembers(orgId);
      return message;
    } catch (e) {
      set({ error: errorMessage(e), sending: false });
      return null;
    }
  },

  sendFile: async (orgId, peerId, fileName, mime, dataBase64) => {
    set({ sending: true, error: null });
    try {
      const message = await invoke<MessengerMessageView>('enclave_messenger_send_file', {
        orgId,
        peerId,
        fileName,
        mime,
        dataBase64,
      });
      set((state) => ({
        messages: state.peerId === peerId ? [...state.messages, message] : state.messages,
        sending: false,
      }));
      void get().loadMembers(orgId);
      return message;
    } catch (e) {
      set({ error: errorMessage(e), sending: false });
      return null;
    }
  },

  getFile: async (orgId, fileId) => {
    try {
      return await invoke<MessengerFileView>('enclave_messenger_file_get', { orgId, fileId });
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  recall: async (orgId, peerId, messageId) => {
    try {
      const message = await invoke<MessengerMessageView>('enclave_messenger_recall', {
        orgId,
        peerId,
        messageId,
      });
      set((state) => ({
        messages: state.messages.map((item) => (item.id === messageId ? message : item)),
        error: null,
      }));
      void get().loadMembers(orgId);
      return message;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  clearThread: async (orgId, peerId) => {
    try {
      await invoke<number>('enclave_messenger_clear', { orgId, peerId });
      set({ messages: [], error: null });
      void get().loadMembers(orgId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  reset: () => set({
    open: false,
    orgId: '',
    peerId: '',
    members: [],
    messages: [],
    settings: null,
    loadingMembers: false,
    loadingMessages: false,
    sending: false,
    error: null,
  }),
}));
