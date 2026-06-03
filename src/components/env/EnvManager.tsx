import { useState, useEffect, useCallback, useRef } from 'react';
import { CheckCircle, XCircle, Download, RefreshCw, Loader2, ArrowUpCircle, ExternalLink } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import type { EnvInfo } from '../../types';
import { InstallLog } from './InstallLog';

interface DependencyStatus {
  name: string;
  key: string;
  version: string | null;
  installed: boolean;
  action?: string;
  installing?: boolean;
  /** Update check state */
  latestVersion: string | null;
  latestReleaseTime: string | null;
  hasUpdate: boolean;
  updateChecking: boolean;
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
  /** Latest versions from remote registries */
  const [latestVersions, setLatestVersions] = useState<Record<string, { version: string; releaseTime: string | null } | null>>({});
  const [updateChecking, setUpdateChecking] = useState<Record<string, boolean>>({});


  const addLog = useCallback((message: string, level: 'info' | 'warn' | 'error' | 'success' = 'info') => {
    const entry = { timestamp: Date.now(), message, level };
    setLogs((prev) => [...prev, entry]);
    // Persist to SQLite (fire-and-forget)
    invoke('insert_log', { message, level }).catch(() => {});
  }, []);

  const fetchEnv = useCallback(async () => {
    setLoading(true);
    try {
      const info = await invoke<EnvInfo>('detect_env');
      setEnvInfo(info);
      addLog('环境检测完成', 'success');
    } catch (err: any) {
      const msg = err?.message || err?.code || (typeof err === 'string' ? err : JSON.stringify(err));
      addLog(`环境检测失败: ${msg}`, 'error');
    }
    setLoading(false);
  }, [addLog]);

  interface VersionTimeInfo {
    version: string;
    releaseTime: string | null;
  }

  /** Query latest version for a single agent from registry */
  const checkAgentUpdate = useCallback(async (key: string, packageName: string, registry: 'npm' | 'pypi') => {
    setUpdateChecking((prev) => ({ ...prev, [key]: true }));
    try {
      let info: VersionTimeInfo | null = null;
      if (registry === 'npm') {
        const resp = await invoke<VersionTimeInfo>('check_single_npm', { packageName });
        info = resp && resp.version ? resp : null;
      } else {
        const resp = await invoke<VersionTimeInfo>('check_single_pypi', { packageName });
        info = resp && resp.version ? resp : null;
      }
      setLatestVersions((prev) => ({ ...prev, [key]: info }));
      if (info) {
        addLog(`${key} 最新版本: ${info.version}${info.releaseTime ? ` (${info.releaseTime})` : ''}`, 'info');
      }
    } catch (err: any) {
      const msg = err?.message || (typeof err === 'string' ? err : JSON.stringify(err));
      addLog(`检查 ${key} 更新失败: ${msg}`, 'warn');
    } finally {
      setUpdateChecking((prev) => ({ ...prev, [key]: false }));
    }
  }, [addLog]);

  const fetchedRef = useRef(false);

  useEffect(() => {
    // Prevent duplicate fetch in React StrictMode (dev mode double-invocation)
    if (fetchedRef.current) return;
    fetchedRef.current = true;

    fetchEnv();

    // Listen for install progress events
    const unlisten = listen<string>('install-progress', (event) => {
      addLog(event.payload, 'info');
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchEnv, addLog]);

  /** After env detection completes, auto-check updates for installed agents */
  useEffect(() => {
    if (envInfo) {
      if (envInfo.claudeCodeVersion) {
        checkAgentUpdate('claude', '@anthropic-ai/claude-code', 'npm');
      }
      if (envInfo.hermesVersion) {
        checkAgentUpdate('hermes', 'hermes-agent', 'pypi');
      }
    }
  }, [envInfo, checkAgentUpdate]);

  /** Simple semver older check */
  const isOlder = (current: string, latest: string) => {
    const parse = (v: string) => v.replace(/^v/, '').split('.').map(Number);
    const a = parse(current);
    const b = parse(latest);
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      const av = a[i] ?? 0;
      const bv = b[i] ?? 0;
      if (av < bv) return true;
      if (av > bv) return false;
    }
    return false;
  };

  const dependencies: DependencyStatus[] = [
    {
      name: 'Node.js',
      key: 'node',
      version: envInfo?.nodeVersion ?? null,
      installed: !!envInfo?.nodeVersion,
      latestVersion: null,
      latestReleaseTime: null,
      hasUpdate: false,
      updateChecking: false,
    },
    {
      name: 'Git',
      key: 'git',
      version: envInfo?.gitVersion ?? null,
      installed: !!envInfo?.gitVersion,
      latestVersion: null,
      latestReleaseTime: null,
      hasUpdate: false,
      updateChecking: false,
    },
    {
      name: 'Python',
      key: 'python',
      version: envInfo?.pythonVersion ?? null,
      installed: !!envInfo?.pythonVersion,
      latestVersion: null,
      latestReleaseTime: null,
      hasUpdate: false,
      updateChecking: false,
    },
    {
      name: 'Claude Code',
      key: 'claude',
      version: envInfo?.claudeCodeVersion ?? null,
      installed: !!envInfo?.claudeCodeVersion,
      action: envInfo?.claudeCodeVersion ? 'update' : 'install',
      installing: installing === 'claude',
      latestVersion: latestVersions['claude']?.version ?? null,
      latestReleaseTime: latestVersions['claude']?.releaseTime ?? null,
      hasUpdate: envInfo?.claudeCodeVersion && latestVersions['claude']
        ? isOlder(envInfo.claudeCodeVersion, latestVersions['claude'].version)
        : false,
      updateChecking: updateChecking['claude'] ?? false,

    },
    {
      name: 'Hermes Agent',
      key: 'hermes',
      version: envInfo?.hermesVersion ?? null,
      installed: !!envInfo?.hermesVersion,
      action: envInfo?.hermesVersion ? 'update' : 'install',
      installing: installing === 'hermes',
      latestVersion: latestVersions['hermes']?.version ?? null,
      latestReleaseTime: latestVersions['hermes']?.releaseTime ?? null,
      hasUpdate: envInfo?.hermesVersion && latestVersions['hermes']
        ? isOlder(envInfo.hermesVersion, latestVersions['hermes'].version)
        : false,
      updateChecking: updateChecking['hermes'] ?? false,

    },
  ];

  const handleInstall = async (key: string) => {
    const name = key === 'claude' ? 'Claude Code' : 'Hermes Agent';
    setInstalling(key);
    addLog(`开始安装 ${name}...`, 'info');
    try {
      if (key === 'claude') {
        await invoke('install_claude_code');
        addLog('Claude Code 安装成功', 'success');
      } else if (key === 'hermes') {
        await invoke('install_hermes');
        addLog('Hermes Agent 安装成功', 'success');
      }
      await fetchEnv();
    } catch (err: any) {
      const msg = err?.message || err?.details || (typeof err === 'string' ? err : JSON.stringify(err));
      addLog(`${name} 安装失败: ${msg}`, 'error');
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

      {/* Agent Detection with Update Check */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          Agent 检测
        </h3>
        <div className="space-y-2">
          {dependencies.slice(3).map((dep) => (
            <div
              key={dep.name}
              className="flex items-center justify-between px-3 py-2.5 rounded-lg transition-colors"
              style={{
                backgroundColor: dep.hasUpdate ? 'rgba(245, 158, 11, 0.06)' : 'var(--bg-tertiary)',
                border: dep.hasUpdate ? '1px solid rgba(245, 158, 11, 0.2)' : '1px solid transparent',
              }}
            >
              {/* Left: status icon + name + version */}
              <div className="flex items-center gap-2 min-w-0">
                {dep.installed ? (
                  <CheckCircle size={14} style={{ color: '#10B981', flexShrink: 0 }} />
                ) : (
                  <XCircle size={14} style={{ color: '#EF4444', flexShrink: 0 }} />
                )}
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                      {dep.name}
                    </span>
                    {dep.hasUpdate && (
                      <ArrowUpCircle size={13} style={{ color: '#F59E0B' }} />
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    <span className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                      {dep.installed ? `v${dep.version}` : '未安装'}
                    </span>

                    {dep.updateChecking && (
                      <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
                    )}
                    {dep.latestVersion && !dep.updateChecking && (
                      <span className="text-[10px]" style={{ color: dep.hasUpdate ? '#F59E0B' : 'var(--text-tertiary)' }}>
                        最新 v{dep.latestVersion}
                      </span>
                    )}
                    {dep.latestVersion && dep.latestReleaseTime && !dep.updateChecking && (
                      <span className="text-[10px]" style={{ color: dep.hasUpdate ? '#F59E0B' : 'var(--text-tertiary)' }}>
                        ({dep.latestReleaseTime})
                      </span>
                    )}
                  </div>
                </div>
              </div>

              {/* Right: action buttons */}
              <div className="flex items-center gap-1.5 shrink-0">
                {!dep.installed && (
                  <button
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    className="flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-medium transition-colors disabled:opacity-50"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                  >
                    {dep.installing ? (
                      <><Loader2 size={11} className="animate-spin" /> 安装中</>
                    ) : (
                      <><Download size={11} /> 安装</>
                    )}
                  </button>
                )}
                {dep.hasUpdate && dep.installed && (
                  <button
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    className="flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-medium transition-colors disabled:opacity-50"
                    style={{ backgroundColor: '#F59E0B', color: '#fff' }}
                  >
                    {dep.installing ? (
                      <><Loader2 size={11} className="animate-spin" /> 更新中</>
                    ) : (
                      <><ArrowUpCircle size={11} /> 更新</>
                    )}
                  </button>
                )}
                {dep.installed && !dep.hasUpdate && dep.latestVersion && (
                  <span className="text-[10px]" style={{ color: '#10B981' }}>
                    已是最新
                  </span>
                )}
                {dep.installed && !dep.hasUpdate && !dep.latestVersion && !dep.updateChecking && (
                  <button
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    className="flex items-center gap-1 px-2 py-1 rounded text-[10px] transition-colors disabled:opacity-50"
                    style={{
                      backgroundColor: 'var(--bg-secondary)',
                      color: 'var(--text-secondary)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    <RefreshCw size={11} />
                    重装
                  </button>
                )}
              </div>
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
      <InstallLog logs={logs} isActive={!!installing} onClear={() => { setLogs([]); invoke('clear_logs').catch(() => {}); }} />
    </div>
  );
}
