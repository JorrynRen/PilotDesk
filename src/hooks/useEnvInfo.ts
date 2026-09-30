import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { EnvInfo } from '../types';

/**
 * Shared hook for environment info (agent versions, tool versions).
 * Both StatusBar and EnvManager use this to avoid duplicate detect_env calls.
 *
 * Features:
 * - Singleton: only one detect_env call at a time (Rust side also has caching)
 * - Fetch on first mount (app startup)
 * - Manual refresh: via refresh() callback (used after install/update)
 * - No auto-polling: agent versions rarely change during a session
 * - Streaming: receives incremental events for progressive rendering
 */

/** Per-agent detection state */
export type AgentDetectStatus = 'pending' | 'detecting' | 'done' | 'error';

interface EnvState {
  info: EnvInfo | null;
  loading: boolean;
  /** Per-agent status map for progressive rendering */
  agentStatus: Record<string, AgentDetectStatus>;
}

const globalState: EnvState = {
  info: null,
  loading: false,
  agentStatus: {},
};
const globalListeners = new Set<() => void>();
let globalFetchPromise: Promise<EnvInfo | null> | null = null;
const unlistenFns: Array<() => void> = [];
let listenersSetup = false;

function notifyListeners() {
  for (const listener of globalListeners) {
    listener();
  }
}

/** Set up Tauri event listeners (once, idempotent) */
function setupListeners() {
  if (listenersSetup) return;
  listenersSetup = true;

  // Base tools (node/git/python) — arrive first
  listen<Record<string, string | null>>('env-base-tools', (event) => {
    const data = event.payload;
    globalState.info = {
      nodeVersion: data.nodeVersion ?? null,
      gitVersion: data.gitVersion ?? null,
      pythonVersion: data.pythonVersion ?? null,
      agentVersions: globalState.info?.agentVersions ?? {},
    };
    notifyListeners();
  }).then((unlisten) => unlistenFns.push(unlisten));

  // Per-agent version (arrives incrementally, before base tools)
  listen<{ agentType: string; version: string | null }>('env-agent-version', (event) => {
    const { agentType, version } = event.payload;
    globalState.info = {
      nodeVersion: globalState.info?.nodeVersion ?? null,
      gitVersion: globalState.info?.gitVersion ?? null,
      pythonVersion: globalState.info?.pythonVersion ?? null,
      agentVersions: { ...(globalState.info?.agentVersions ?? {}), [agentType]: version },
    };
    globalState.agentStatus = {
      ...globalState.agentStatus,
      [agentType]: version !== null ? 'done' : 'error',
    };
    notifyListeners();
  }).then((unlisten) => unlistenFns.push(unlisten));

  // Per-agent latest version (arrives incrementally, Phase 2)
  listen<{ agentType: string; version: string | null }>('env-agent-latest-version', (event) => {
    const { agentType, version } = event.payload;
    globalState.info = {
      ...globalState.info!,
      agentLatestVersions: { ...(globalState.info?.agentLatestVersions ?? {}), [agentType]: version },
    };
    notifyListeners();
  }).then((unlisten) => unlistenFns.push(unlisten));

  // Final complete result (write to cache on Rust side)
  listen<EnvInfo>('env-detect-done', (event) => {
    globalState.info = event.payload;
    // Mark all remaining agents as done
    const finalStatus: Record<string, AgentDetectStatus> = {};
    for (const key of Object.keys(event.payload.agentVersions)) {
      finalStatus[key] = event.payload.agentVersions[key] !== null ? 'done' : 'error';
    }
    globalState.agentStatus = finalStatus;
    notifyListeners();
  }).then((unlisten) => unlistenFns.push(unlisten));
}

async function fetchEnv(): Promise<EnvInfo | null> {
  // If a fetch is already in-flight, return the same promise (no duplicate call)
  if (globalFetchPromise) return globalFetchPromise;

  globalState.loading = true;
  globalState.agentStatus = {};
  notifyListeners();

  globalFetchPromise = (async () => {
    try {
      const info = await invoke<EnvInfo>('detect_env');
      // The streaming events may have already populated parts of the state,
      // but ensure final state is consistent with the invoke result.
      globalState.info = info;
      const finalStatus: Record<string, AgentDetectStatus> = {};
      for (const key of Object.keys(info.agentVersions)) {
        finalStatus[key] = info.agentVersions[key] !== null ? 'done' : 'error';
      }
      globalState.agentStatus = finalStatus;
      return info;
    } catch {
      return null;
    } finally {
      globalState.loading = false;
      globalFetchPromise = null;
      notifyListeners();
    }
  })();

  return globalFetchPromise;
}

export function useEnvInfo() {
  const [, setTick] = useState(0);

  useEffect(() => {
    // Ensure event listeners are set up (only once globally)
    setupListeners();

    const listener = () => setTick((t) => t + 1);
    globalListeners.add(listener);

    // Fetch on first mount if not yet fetched (app startup)
    if (globalState.info === null && !globalFetchPromise) {
      fetchEnv();
    }

    return () => {
      globalListeners.delete(listener);
    };
  }, []);

  const refresh = useCallback(() => {
    // Reset promise to force a fresh detection (ignore in-flight)
    globalFetchPromise = null;
    return fetchEnv();
  }, []);

  return {
    envInfo: globalState.info,
    loading: globalState.loading,
    agentStatus: globalState.agentStatus,
    refresh,
  };
}

export default useEnvInfo;
