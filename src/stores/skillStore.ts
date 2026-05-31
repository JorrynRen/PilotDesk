import { create } from 'zustand';

export interface SkillInfo {
  name: string;
  description: string;
  category?: string;
}

interface SkillState {
  // 按 agentType 分组的技能列表
  skillsByAgent: Record<string, SkillInfo[]>;
  isLoading: boolean;
  lastFetched: number | null;

  // 设置某个 agent 的技能
  setAgentSkills: (agentType: string, skills: SkillInfo[]) => void;

  // 获取某个 agent 的技能
  getAgentSkills: (agentType: string) => SkillInfo[];

  // 设置加载状态
  setLoading: (loading: boolean) => void;

  // 清空所有缓存
  clearAll: () => void;
}

export const useSkillStore = create<SkillState>((set, get) => ({
  skillsByAgent: {},
  isLoading: false,
  lastFetched: null,

  setAgentSkills: (agentType, skills) => {
    set((state) => ({
      skillsByAgent: { ...state.skillsByAgent, [agentType]: skills },
      lastFetched: Date.now(),
    }));
  },

  getAgentSkills: (agentType) => {
    return get().skillsByAgent[agentType] || [];
  },

  setLoading: (loading) => {
    set({ isLoading: loading });
  },

  clearAll: () => {
    set({ skillsByAgent: {}, lastFetched: null });
  },
}));
