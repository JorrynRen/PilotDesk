import { invoke } from '@tauri-apps/api/core';
import { useState, useCallback } from 'react';

export function useTauriCommand<T, A extends unknown[] = []>(
  commandName: string
) {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const execute = useCallback(async (...args: A) => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<T>(commandName, ...(args as any));
      setData(result);
      return result;
    } catch (e: any) {
      const errMsg = e?.message || e?.toString() || '未知错误';
      setError(errMsg);
      throw e;
    } finally {
      setLoading(false);
    }
  }, [commandName]);

  return { data, loading, error, execute };
}
