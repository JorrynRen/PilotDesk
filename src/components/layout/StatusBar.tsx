import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { EnvInfo } from '../../types';

interface StatusBarProps {
  onOpenEnv?: () => void;
}

export function StatusBar({ onOpenEnv }: StatusBarProps) {
  const [envInfo, setEnvInfo] = useState<EnvInfo | null>(null);

  const fetchEnv = useCallback(async () => {
    try {
      const info = await invoke<EnvInfo>('detect_env');
      setEnvInfo(info);
    } catch {
      // Silent fail for status bar
    }
  }, []);

  useEffect(() => {
    fetchEnv();
  }, [fetchEnv]);

  const claudeStatus = envInfo?.claudeCodeVersion
    ? { color: '#10B981', label: envInfo.claudeCodeVersion, connected: true }
    : { color: '#6B7280', label: '未安装', connected: false };

  const hermesStatus = envInfo?.hermesVersion
    ? { color: '#10B981', label: envInfo.hermesVersion, connected: true }
    : { color: '#6B7280', label: '未安装', connected: false };

  return (
    <footer
      className="flex items-center justify-between px-4 h-6 text-[10px] shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1">
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: '#10B981' }} />
          就绪
        </span>
        <button
          onClick={onOpenEnv}
          className="flex items-center gap-1 transition-colors hover:opacity-80"
          title="点击打开环境管理"
        >
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: claudeStatus.color }} />
          Claude: {claudeStatus.label}
        </button>
        <button
          onClick={onOpenEnv}
          className="flex items-center gap-1 transition-colors hover:opacity-80"
          title="点击打开环境管理"
        >
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: hermesStatus.color }} />
          Hermes: {hermesStatus.label}
        </button>
      </div>
      <div className="flex items-center gap-3">
        {envInfo?.nodeVersion && <span>Node {envInfo.nodeVersion}</span>}
        {envInfo?.gitVersion && <span>Git {envInfo.gitVersion}</span>}
        <span style={{ color: 'var(--text-tertiary)' }}>v0.1.0</span>
      </div>
    </footer>
  );
}
