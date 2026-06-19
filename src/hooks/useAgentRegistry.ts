import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AgentConfig, AgentTheme } from '../types';

const FALLBACK_THEME: AgentTheme = {
  color: '#6B7280',
  bg: 'rgba(107,114,128,0.15)',
  label: '未知',
  initial: '?',
  cssVar: 'var(--text-tertiary)',
};

function agentTypeToCssVar(agentType: string): string {
  const map: Record<string, string> = {
    claude: 'var(--claude-tag)',
    hermes: 'var(--hermes-tag)',
    codex: 'var(--codex-tag)',
    api: 'var(--api-tag)',
  };
  return map[agentType] || 'var(--text-tertiary)';
}

function agentTypeToInitial(agentType: string): string {
  if (agentType === 'codex') return 'X';
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
    globalError = String(err);
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

  /** Get theme for an agent type (from DB color, fallback to AGENT_THEMES) */
  const getTheme = useCallback((agentType: string): AgentTheme => {
    const agent = globalAgents.find(a => a.agentType === agentType);
    if (agent) {
      return {
        color: agent.color,
        bg: hexToRgba(agent.color, 0.15),
        label: agent.displayName,
        initial: agentTypeToInitial(agentType),
        cssVar: agentTypeToCssVar(agentType),
      };
    }
    return getBuiltinTheme(agentType);
  }, []);

  /** Get display name for an agent type */
  const getDisplayName = useCallback((agentType: string): string => {
    const agent = globalAgents.find(a => a.agentType === agentType);
    return agent?.displayName || getBuiltinLabel(agentType);
  }, []);

  /** Get enabled agent types (for session creation dropdown) */
  const getEnabledAgentTypes = useCallback((): string[] => {
    const dbTypes = globalAgents.filter(a => a.isEnabled).map(a => a.agentType);
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

function hexToRgba(hex: string, alpha: number): string {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (result) {
    return `rgba(${parseInt(result[1], 16)},${parseInt(result[2], 16)},${parseInt(result[3], 16)},${alpha})`;
  }
  return `rgba(107,114,128,${alpha})`;
}

/** Built-in fallback themes for types not yet in agents table */
const BUILTIN_THEMES: Record<string, AgentTheme> = {
  claude: { color: '#3B82F6', bg: 'rgba(59,130,246,0.15)', label: 'Claude Code', initial: 'C', cssVar: 'var(--claude-tag)' },
  hermes: { color: '#8B5CF6', bg: 'rgba(139,92,246,0.15)', label: 'Hermes Agent', initial: 'H', cssVar: 'var(--hermes-tag)' },
  codex: { color: '#F59E0B', bg: 'rgba(245,158,11,0.15)', label: 'codeX', initial: 'X', cssVar: 'var(--codex-tag)' },
  api: { color: '#10B981', bg: 'rgba(16,185,129,0.15)', label: 'API 直连', initial: 'A', cssVar: 'var(--api-tag)' },
  manual: { color: '#6B7280', bg: 'rgba(107,114,128,0.15)', label: '手动', initial: 'M', cssVar: 'var(--text-tertiary)' },
};

function getBuiltinTheme(agentType: string): AgentTheme {
  return BUILTIN_THEMES[agentType] || FALLBACK_THEME;
}

function getBuiltinLabel(agentType: string): string {
  const map: Record<string, string> = {
    claude: 'Claude Code',
    hermes: 'Hermes Agent',
    codex: 'codeX',
    api: 'API 直连',
  };
  return map[agentType] || agentType;
}
