import { useState, useEffect, useCallback } from 'react';
import { CheckCircle, XCircle, Download, RefreshCw, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { EnvInfo } from '../../types';
import { InstallLog } from './InstallLog';

interface DependencyStatus {
  name: string;
  version: string | null;
  installed: boolean;
  action?: string;
  installing?: boolean;
}

interface EnvManagerProps {
  onComplete?: () => void;
}

export type { EnvManagerProps };

export function EnvManager({ onComplete: _onComplete }: EnvManagerProps) {
  const [envInfo, setEnvInfo] = useState<EnvInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState<string | null>(null);
  const [logs, setLogs] = useState<Array<{ timestamp: number; message: string; level: 'info' | 'warn' | 'error' | 'success' }>>([]);

  const addLog = useCallback((message: string, level: 'info' | 'warn' | 'error' | 'success' = 'info') => {
    setLogs((prev) => [...prev, { timestamp: Date.now(), message, level }]);
  }, []);

  const fetchEnv = useCallback(async () => {
    setLoading(true);
    try {
      const info = await invoke<EnvInfo>('detect_env');
      setEnvInfo(info);
      addLog('环境检测完成', 'success');
    } catch (err) {
      addLog(`环境检测失败: ${err}`, 'error');
    }
    setLoading(false);
  }, [addLog]);

  useEffect(() => {
    fetchEnv();

    // Listen for install progress events
    const unlisten = listen<string>('install-progress', (event) => {
      addLog(event.payload, 'info');
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchEnv, addLog]);

  const dependencies: DependencyStatus[] = [
    {
      name: 'Node.js',
      version: envInfo?.nodeVersion ?? null,
      installed: !!envInfo?.nodeVersion,
    },
    {
      name: 'Git',
      version: envInfo?.gitVersion ?? null,
      installed: !!envInfo?.gitVersion,
    },
    {
      name: 'Python',
      version: envInfo?.pythonVersion ?? null,
      installed: !!envInfo?.pythonVersion,
    },
    {
      name: 'Claude Code',
      version: envInfo?.claudeCodeVersion ?? null,
      installed: !!envInfo?.claudeCodeVersion,
      action: envInfo?.claudeCodeVersion ? 'update' : 'install',
      installing: installing === 'claude',
    },
    {
      name: 'Hermes Agent',
      version: envInfo?.hermesVersion ?? null,
      installed: !!envInfo?.hermesVersion,
      action: envInfo?.hermesVersion ? 'update' : 'install',
      installing: installing === 'hermes',
    },
  ];

  const handleInstall = async (name: string) => {
    setInstalling(name);
    addLog(`开始安装 ${name}...`, 'info');
    try {
      if (name === 'Claude Code') {
        await invoke('install_claude_code');
        addLog('Claude Code 安装成功', 'success');
      } else if (name === 'Hermes Agent') {
        await invoke('install_hermes');
        addLog('Hermes Agent 安装成功', 'success');
      }
      await fetchEnv();
    } catch (err) {
      addLog(`${name} 安装失败: ${err}`, 'error');
    }
    setInstalling(null);
  };

  return (
    <div className="space-y-6">
      {/* Prerequisites */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          前置依赖
        </h3>
        <div className="space-y-2">
          {dependencies.slice(0, 3).map((dep) => (
            <div
              key={dep.name}
              className="flex items-center justify-between px-3 py-2 rounded-lg"
              style={{ backgroundColor: 'var(--bg-tertiary)' }}
            >
              <div className="flex items-center gap-2">
                {dep.installed ? (
                  <CheckCircle size={14} style={{ color: '#10B981' }} />
                ) : (
                  <XCircle size={14} style={{ color: '#EF4444' }} />
                )}
                <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                  {dep.name}
                </span>
              </div>
              <span className="text-[10px]" style={{ color: dep.installed ? 'var(--text-secondary)' : 'var(--danger)' }}>
                {dep.version ?? '未安装'}
              </span>
            </div>
          ))}
        </div>
      </section>

      {/* Agent Detection */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          Agent 检测
        </h3>
        <div className="space-y-2">
          {dependencies.slice(3).map((dep) => (
            <div
              key={dep.name}
              className="flex items-center justify-between px-3 py-2 rounded-lg"
              style={{ backgroundColor: 'var(--bg-tertiary)' }}
            >
              <div className="flex items-center gap-2">
                {dep.installed ? (
                  <CheckCircle size={14} style={{ color: '#10B981' }} />
                ) : (
                  <XCircle size={14} style={{ color: '#EF4444' }} />
                )}
                <div>
                  <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                    {dep.name}
                  </span>
                  <span className="text-[10px] ml-2" style={{ color: 'var(--text-secondary)' }}>
                    {dep.version ?? '未安装'}
                  </span>
                </div>
              </div>
              <button
                onClick={() => handleInstall(dep.name)}
                disabled={!!dep.installing}
                className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium transition-colors disabled:opacity-50"
                style={{
                  backgroundColor: dep.installed ? 'var(--bg-secondary)' : 'var(--accent)',
                  color: dep.installed ? 'var(--text-secondary)' : '#fff',
                  border: `1px solid ${dep.installed ? 'var(--border)' : 'transparent'}`,
                }}
              >
                {dep.installing ? (
                  <>
                    <Loader2 size={12} className="animate-spin" />
                    安装中
                  </>
                ) : dep.installed ? (
                  <>
                    <RefreshCw size={12} />
                    重新安装
                  </>
                ) : (
                  <>
                    <Download size={12} />
                    安装
                  </>
                )}
              </button>
            </div>
          ))}
        </div>
      </section>

      {/* Refresh button */}
      <button
        onClick={fetchEnv}
        disabled={loading}
        className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors disabled:opacity-50"
        style={{
          backgroundColor: 'var(--bg-tertiary)',
          color: 'var(--text-secondary)',
          border: '1px solid var(--border)',
        }}
      >
        <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
        重新检测环境
      </button>

      {/* Install Log */}
      <InstallLog logs={logs} isActive={!!installing} />
    </div>
  );
}
