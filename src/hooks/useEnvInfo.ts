import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
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
 */
let globalEnvInfo: EnvInfo | null = null;
let globalListeners = new Set<() => void>();
let globalLoading = false;
let globalFetching = false;

function notifyListeners() {
  for (const listener of globalListeners) {
    listener();
  }
}

async function fetchEnv() {
  if (globalFetching) return;
  globalFetching = true;
  globalLoading = true;
  notifyListeners();
  try {
    const info = await invoke<EnvInfo>('detect_env');
    globalEnvInfo = info;
  } catch {
    // Silent fail
  }
  globalLoading = false;
  globalFetching = false;
  notifyListeners();
}

export function useEnvInfo() {
  const [, setTick] = useState(0);

  useEffect(() => {
    const listener = () => setTick((t) => t + 1);
    globalListeners.add(listener);

    // Fetch on first mount if not yet fetched (app startup)
    if (globalEnvInfo === null && !globalFetching) {
      fetchEnv();
    }

    return () => {
      globalListeners.delete(listener);
    };
  }, []);

  const refresh = useCallback(() => {
    fetchEnv();
  }, []);

  return {
    envInfo: globalEnvInfo,
    loading: globalLoading,
    refresh,
  };
}

export default useEnvInfo;
