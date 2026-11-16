import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface SessionWorkspaceInfo {
  workRoot: string;
  projectRoot?: string | null;
  linkedRoots: string[];
  memoryRoot: string;
  artifactRoot: string;
  projectId?: string;
  source: 'hostBound' | 'project' | 'global';
  globalWorkspaceConfigured: boolean;
  hostResume: boolean;
}

/** 读取后端本次会话实际采用的工作根；普通会话不持久化该路径。 */
export function useSessionWorkspaceInfo(
  sessionId?: string | null,
  revision?: string | number | null,
): {
  info: SessionWorkspaceInfo | null;
  loading: boolean;
} {
  const scope = `${sessionId ?? ''}:${revision ?? ''}`;
  const [result, setResult] = useState<{
    scope: string;
    info: SessionWorkspaceInfo | null;
  }>({ scope, info: null });
  const current = result.scope === scope
    ? result
    : { scope, info: null };

  useEffect(() => {
    if (!sessionId) return;
    let disposed = false;
    void invoke<SessionWorkspaceInfo>('session_workspace_info', { sessionId })
      .then((next) => {
        if (!disposed) {
          setResult({ scope, info: next });
        }
      })
      .catch(() => {
        if (!disposed) {
          setResult({ scope, info: null });
        }
      });

    return () => {
      disposed = true;
    };
  }, [scope, sessionId]);

  return { info: current.info, loading: Boolean(sessionId) && result.scope !== scope };
}
