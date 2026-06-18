import { useState, useEffect, useCallback, useRef } from 'react';
import { CheckCircle, XCircle, Download, RefreshCw, Loader2, ArrowUpCircle, ExternalLink } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { InstallLog } from './InstallLog';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { SettingsSection, SettingsCard, SettingsButton, SettingsStatusIcon } from '../settings';

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
  const { envInfo, loading, refresh: fetchEnv } = useEnvInfo();
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
    } catch {
      addLog(`检查 ${key} 更新失败`, 'warn');
    } finally {
      setUpdateChecking((prev) => ({ ...prev, [key]: false }));
    }
  }, [addLog]);

  useEffect(() => {
    // Listen for install progress events
    // useEnvInfo already handles initial environment detection on first mount
    const unlisten = listen<string>('install-progress', (event) => {
      addLog(event.payload, 'info');
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addLog]);
  


const fetchEnvWithLog = useCallback(async () => {
    const info = await fetchEnv();
    addLog('环境检测完成', 'success');
    // After detection, check updates for installed agents
    if (info) {
      if (info.claudeCodeVersion) {
        checkAgentUpdate('claude', '@anthropic-ai/claude-code', 'npm');
      }
      if (info.hermesVersion) {
        checkAgentUpdate('hermes', 'hermes-agent', 'pypi');
      }
      if (info.codexVersion) {
        checkAgentUpdate('codex', '@openai/codex', 'npm');
      }
    }
  }, [fetchEnv, addLog, checkAgentUpdate]);

  interface VersionTimeInfo {
    version: string;
    releaseTime: string | null;
  }

  /** Query latest version for a single agent from registry */
  

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
    {
      name: 'codeX',
      key: 'codex',
      version: envInfo?.codexVersion ?? null,
      installed: !!envInfo?.codexVersion,
      action: envInfo?.codexVersion ? 'update' : 'install',
      installing: installing === 'codex',
      latestVersion: latestVersions['codex']?.version ?? null,
      latestReleaseTime: latestVersions['codex']?.releaseTime ?? null,
      hasUpdate: envInfo?.codexVersion && latestVersions['codex']
        ? isOlder(envInfo.codexVersion, latestVersions['codex'].version)
        : false,
      updateChecking: updateChecking['codex'] ?? false,
    },
  ];

  const handleInstall = async (key: string) => {
    const name = key === 'claude' ? 'Claude Code' : key === 'hermes' ? 'Hermes Agent' : 'codeX';
    setInstalling(key);
    addLog(`开始安装 ${name}...`, 'info');
    try {
      if (key === 'claude') {
        await invoke('install_claude_code');
        addLog('Claude Code 安装成功', 'success');
      } else if (key === 'hermes') {
        await invoke('install_hermes');
        addLog('Hermes Agent 安装成功', 'success');
      } else if (key === 'codex') {
        await invoke('install_codex');
        addLog('codeX 安装成功', 'success');
      }
      const info = await fetchEnv();
      if (info) {
        const pkg = key === 'claude' ? '@anthropic-ai/claude-code'
          : key === 'hermes' ? 'hermes-agent'
          : '@openai/codex';
        const registry = key === 'hermes' ? 'pypi' as const : 'npm' as const;
        checkAgentUpdate(key, pkg, registry);
      }
    } catch (err: any) {
      const msg = err?.message || err?.details || (typeof err === 'string' ? err : JSON.stringify(err));
      addLog(`${name} 安装失败: ${msg}`, 'error');
    }
    setInstalling(null);
  };

  return (
    <div className="space-y-6">
      {/* Prerequisites */}
      <SettingsSection title="前置依赖">
        <div className="space-y-2">
          {dependencies.slice(0, 3).map((dep) => (
            <SettingsCard key={dep.name}>
              <div className="flex items-center gap-2">
                <SettingsStatusIcon installed={dep.installed} />
                <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
                  {dep.name}
                </span>
              </div>
              <span className="text-[10px]" style={{ color: dep.installed ? 'var(--text-secondary)' : '#EF4444' }}>
                {dep.version ?? '未安装'}
              </span>
            </SettingsCard>
          ))}
        </div>
      </SettingsSection>

      {/* Agent Detection with Update Check */}
      <SettingsSection title="Agent 检测">
        <div className="space-y-2">
          {dependencies.slice(3).map((dep) => (
            <SettingsCard key={dep.name} highlight={dep.hasUpdate}>
              {/* Left: status icon + name + version */}
              <div className="flex items-center gap-2 min-w-0">
                <SettingsStatusIcon installed={dep.installed} />
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
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
                  <SettingsButton
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    variant="primary"
                  >
                    {dep.installing ? (
                      <><Loader2 size={11} className="animate-spin" /> 安装中</>
                    ) : (
                      <><Download size={11} /> 安装</>
                    )}
                  </SettingsButton>
                )}
                {dep.hasUpdate && dep.installed && (
                  <SettingsButton
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    variant="warning"
                  >
                    {dep.installing ? (
                      <><Loader2 size={11} className="animate-spin" /> 更新中</>
                    ) : (
                      <><ArrowUpCircle size={11} /> 更新</>
                    )}
                  </SettingsButton>
                )}
                {dep.installed && !dep.hasUpdate && dep.latestVersion && (
                  <span className="text-[10px]" style={{ color: '#10B981' }}>
                    已是最新
                  </span>
                )}
                {dep.installed && !dep.hasUpdate && !dep.latestVersion && !dep.updateChecking && (
                  <SettingsButton
                    onClick={() => handleInstall(dep.key)}
                    disabled={!!dep.installing}
                    variant="secondary"
                    icon={<RefreshCw size={11} />}
                  >
                    重装
                  </SettingsButton>
                )}
              </div>
            </SettingsCard>
          ))}
        </div>
      </SettingsSection>

      {/* Refresh button */}
      <SettingsButton
        onClick={fetchEnvWithLog}
        disabled={loading}
        variant="secondary"
        icon={<RefreshCw size={12} className={loading ? 'animate-spin' : ''} />}
      >
        重新检测环境
      </SettingsButton>

      {/* Install Log */}
      <InstallLog isActive={!!installing} />
    </div>
  );
}
