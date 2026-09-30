import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AgentConfig } from '../types';
import { errorMessage } from '../utils/errorMessage';

interface AgentTheme {
  color: string;
  bg: string;
  label: string;
  initial: string;
  icon?: string;
}

/** 非注册 agent 类型（manual/api 等）的显示标签映射 */
const SOURCE_LABELS: Record<string, string> = {
  manual: '手动',
  api: 'API Agent',
};

/** 非注册 agent 的品牌色（Tailwind CSS 品牌色系） */
const BRAND_COLORS: Record<string, string> = {
  api: '#10B981',  // Emerald-500 — 代表连接与 API
};

function fallbackTheme(agentType: string): AgentTheme {
  const label = SOURCE_LABELS[agentType] || agentType;
  const color = BRAND_COLORS[agentType] || '#6B7280';
  return {
    color,
    bg: hexToRgbaFallback(color, 0.15),
    label,
    initial: agentTypeToInitial(agentType),
  };
}

/** hexToRgba 的独立版本（不依赖全局状态，用于 fallbackTheme） */
function hexToRgbaFallback(hex: string, alpha: number): string {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (result) {
    return `rgba(${parseInt(result[1], 16)},${parseInt(result[2], 16)},${parseInt(result[3], 16)},${alpha})`;
  }
  return `rgba(107,114,128,${alpha})`;
}

function agentTypeToInitial(agentType: string): string {
  if (agentType === 'api') return 'A';
  return agentType.charAt(0).toUpperCase();
}

// Global singleton state — shared across all components
let globalAgents: AgentConfig[] = [];
let globalLoading = true;
let globalFetching = false;
let globalError: string | null = null;
const globalListeners = new Set<() => void>();

function notifyListeners() {
  for (const listener of globalListeners) {
    listener();
  }
}

async function fetchAgentsGlobal(): Promise<AgentConfig[]> {
  if (globalFetching) return globalAgents;
  globalFetching = true;
  globalLoading = true;
  notifyListeners();
  try {
    const result = await invoke<AgentConfig[]>('list_agents');
    globalAgents = result;
    globalError = null;
    return result;
  } catch (err) {
    globalError = errorMessage(err);
    return globalAgents;
  } finally {
    globalLoading = false;
    globalFetching = false;
    notifyListeners();
  }
}

export function useAgentRegistry() {
  const [, setTick] = useState(0);

  useEffect(() => {
    const listener = () => setTick((t) => t + 1);
    globalListeners.add(listener);

    // Fetch on first mount if not yet fetched
    if (globalAgents.length === 0 && !globalFetching) {
      fetchAgentsGlobal();
    }

    return () => {
      globalListeners.delete(listener);
    };
  }, []);

  const fetchAgents = useCallback(() => {
    return fetchAgentsGlobal();
  }, []);

  /** Get agent config by type */
  const getAgent = useCallback((agentType: string): AgentConfig | undefined => {
    return globalAgents.find(a => a.agentType === agentType);
  }, []);

  /** Get theme for an agent type (from DB color, fallback to neutral gray) */
  const getTheme = useCallback((agentType: string): AgentTheme => {
    const agent = globalAgents.find(a => a.agentType === agentType);
    if (agent) {
      const color = isValidHexColor(agent.color) ? agent.color : '#3B82F6';
      return {
        color,
        bg: isValidHexColor(agent.color) ? hexToRgba(agent.color, 0.15) : 'rgba(59,130,246,0.15)',
        label: agent.displayName,
        initial: agentTypeToInitial(agentType),
        icon: agent.icon || undefined,
      };
    }
    return fallbackTheme(agentType);
  }, []);

  /** Get display name for an agent type */
  const getDisplayName = useCallback((agentType: string): string => {
    const agent = globalAgents.find(a => a.agentType === agentType);
    return agent?.displayName || SOURCE_LABELS[agentType] || agentType;
  }, []);

  /** Get enabled agent types (for session creation dropdown) */
  const getEnabledAgentTypes = useCallback((): string[] => {
    const dbTypes = globalAgents.filter(a => a.isEnabled).sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0)).map(a => a.agentType);
    if (!dbTypes.includes('api')) {
      return [...dbTypes, 'api'];
    }
    return dbTypes;
  }, []);

  return {
    agents: globalAgents,
    loading: globalLoading,
    error: globalError,
    fetchAgents,
    getAgent,
    getTheme,
    getDisplayName,
    getEnabledAgentTypes,
  };
}

/** 校验颜色值是否有效：空值、非法 hex 格式时返回 false */
function isValidHexColor(color: string): boolean {
  if (!color || color.trim() === '') return false;
  return /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(color.trim());
}

function hexToRgba(hex: string, alpha: number): string {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (result) {
    return `rgba(${parseInt(result[1], 16)},${parseInt(result[2], 16)},${parseInt(result[3], 16)},${alpha})`;
  }
  return `rgba(107,114,128,${alpha})`;
}
