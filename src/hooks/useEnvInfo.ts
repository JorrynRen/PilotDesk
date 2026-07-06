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
/** Active fetch promise — all callers share the same in-flight request */
let globalFetchPromise: Promise<EnvInfo | null> | null = null;

function notifyListeners() {
  for (const listener of globalListeners) {
    listener();
  }
}

async function fetchEnv(): Promise<EnvInfo | null> {
  // If a fetch is already in-flight, return the same promise (no duplicate call)
  if (globalFetchPromise) return globalFetchPromise;

  globalLoading = true;
  notifyListeners();

  globalFetchPromise = (async () => {
    try {
      const info = await invoke<EnvInfo>('detect_env');
      globalEnvInfo = info;
      return info;
    } catch {
      return null;
    } finally {
      globalLoading = false;
      globalFetchPromise = null;
      notifyListeners();
    }
  })();

  return globalFetchPromise;
}

export function useEnvInfo() {
  const [, setTick] = useState(0);

  useEffect(() => {
    const listener = () => setTick((t) => t + 1);
    globalListeners.add(listener);

    // Fetch on first mount if not yet fetched (app startup)
    if (globalEnvInfo === null && !globalFetchPromise) {
      fetchEnv();
    }

    return () => {
      globalListeners.delete(listener);
    };
  }, []);

  const refresh = useCallback(() => {
    return fetchEnv();
  }, []);

  return {
    envInfo: globalEnvInfo,
    loading: globalLoading,
    refresh,
  };
}

export default useEnvInfo;
