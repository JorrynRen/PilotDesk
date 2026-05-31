import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { EnvInfo } from '../../types';

interface StatusBarProps {
  onOpenEnv?: () => void;
  wsConnected?: boolean;
}

export function StatusBar({ onOpenEnv, wsConnected }: StatusBarProps) {
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
    // Refresh every 60 seconds
    const timer = setInterval(fetchEnv, 60000);
    return () => clearInterval(timer);
  }, [fetchEnv]);

  const claudeStatus = envInfo?.claudeCodeVersion
    ? { color: '#10B981', label: envInfo.claudeCodeVersion }
    : { color: '#6B7280', label: '未安装' };

  const hermesStatus = envInfo?.hermesVersion
    ? { color: '#10B981', label: envInfo.hermesVersion }
    : { color: '#6B7280', label: '未安装' };

  const wsColor = wsConnected ? '#10B981' : '#F59E0B';
  const wsLabel = wsConnected ? '已连接' : '未连接';

  return (
    <footer
      className="flex items-center justify-between px-4 h-6 text-[10px] shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        {/* WebSocket status */}
        <span className="flex items-center gap-1">
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: wsColor }} />
          Sidecar: {wsLabel}
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
      <div className="flex items-center gap-2">
        <span style={{ color: 'var(--text-tertiary)' }}>PilotDesk v0.1.0</span>
      </div>
    </footer>
  );
}
