import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

interface GeneratingState {
  /** 正在生成的会话 ID → true 的映射 */
  generatingMap: Record<string, boolean>;
  /**
   * 会话 ID → 最后一次流式事件时间戳（ms）。
   * 前置安全网据此判断"静默"而不是"整轮耗时"：只要事件仍在流动（正文/思考/工具/进度），
   * 就说明本轮在推进，不该被当作卡死。工具在跑、等待用户决策由调用方另行豁免。
   */
  lastEventAt: Record<string, number>;
  /** 记录一次流式事件（事件总线派发任何非终态 agent-* 事件时调用） */
  touch: (sessionId: string) => void;
  syncGeneratingSessions: (sessionIds: Set<string>) => void;
  markGenerating: (sessionId: string) => void;
  removeGenerating: (sessionId: string) => void;
  /**
   * 从后端运行态注册表对齐（权威来源）：
   * 后端 `run_api_agent` 在后台 tokio task 中运行，进程活着才有记录；前端此前只用组件内的
   * 临时集合表示"生成中"，组件卸载即丢失，列表脉冲随之熄灭。启动/重入时用后端事实校正。
   */
  syncFromBackend: () => Promise<void>;
}

function areEqual(a: Record<string, boolean>, b: Record<string, boolean>): boolean {
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((k) => k in b);
}

export const useGeneratingStore = create<GeneratingState>((set, get) => ({
  generatingMap: {},
  lastEventAt: {},
  touch: (sessionId) => {
    set((state) => ({ lastEventAt: { ...state.lastEventAt, [sessionId]: Date.now() } }));
  },
  syncGeneratingSessions: (sessionIds) => {
    const next: Record<string, boolean> = {};
    sessionIds.forEach((id) => { next[id] = true; });
    const prev = get().generatingMap;
    if (!areEqual(prev, next)) {
      set({ generatingMap: next });
    }
  },
  markGenerating: (sessionId) => {
    if (get().generatingMap[sessionId]) return;
    set((state) => ({ generatingMap: { ...state.generatingMap, [sessionId]: true } }));
  },
  removeGenerating: (sessionId) => {
    const state = get();
    if (!state.generatingMap[sessionId] && !(sessionId in state.lastEventAt)) return;
    set((prev) => {
      const { [sessionId]: _removed, ...rest } = prev.generatingMap;
      const { [sessionId]: _touched, ...restEventAt } = prev.lastEventAt;
      return { generatingMap: rest, lastEventAt: restEventAt };
    });
  },
  syncFromBackend: async () => {
    try {
      const running = await invoke<string[]>('agent_running_sessions');
      get().syncGeneratingSessions(new Set(running));
    } catch {
      // 查询失败保持现状：脉冲只是提示，不应因一次查询失败清空
    }
  },
}));

/** 选择器：指定会话是否正在生成中（返回 boolean，Zustand 可正确比较） */
export const selectIsGenerating = (sessionId: string) => (state: GeneratingState) =>
  !!state.generatingMap[sessionId];
