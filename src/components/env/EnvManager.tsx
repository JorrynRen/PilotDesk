import { useState, useEffect, useCallback } from 'react';
import { Download, RefreshCw, Loader2, ArrowUpCircle, Trash2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { InstallLog } from './InstallLog';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { SettingsSection, SettingsCard, SettingsButton, SettingsStatusIcon } from '../settings';
import { errorMessage } from '../../utils/errorMessage';

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
  detecting: boolean;
}

interface EnvManagerProps {
  onComplete?: () => void;
}

export type { EnvManagerProps };

export function EnvManager({ onComplete: _onComplete }: EnvManagerProps) {
  const { envInfo, loading: envLoading, agentStatus, refresh: fetchEnv } = useEnvInfo();
  const { agents } = useAgentRegistry();
  const [installing, setInstalling] = useState<string | null>(null);
  const [logs, setLogs] = useState<Array<{ timestamp: number; message: string; level: 'info' | 'warn' | 'error' | 'success' }>>([]);
  /** Latest versions from remote registries */
  const [latestVersions, setLatestVersions] = useState<Record<string, { version: string; releaseTime: string | null } | null>>({});
  const [updateChecking, setUpdateChecking] = useState<Record<string, boolean>>({});

  // Load historical logs from database on mount
  useEffect(() => {
    invoke<Array<{ id: number; timestamp: number; message: string; level: string }>>('list_logs', { limit: 200 })
      .then((entries) => {
        if (entries && entries.length > 0) {
          // list_logs returns DESC order; reverse to show oldest first
          setLogs(entries.reverse().map((e) => ({
            timestamp: e.timestamp,
            message: e.message,
            level: e.level as 'info' | 'warn' | 'error' | 'success',
          })));
        }
      })
      .catch(() => {});
  }, []);

  const addLog = useCallback((message: string, level: 'info' | 'warn' | 'error' | 'success' = 'info') => {
    const entry = { timestamp: Date.now(), message, level };
    setLogs((prev) => [...prev, entry]);
    invoke('insert_log', { message, level }).catch(() => {});
  }, []);

  const checkAgentUpdate = useCallback(async (key: string) => {
    setUpdateChecking((prev) => ({ ...prev, [key]: true }));
    try {
      const info = await invoke<{ version: string; releaseTime: string | null } | null>('check_agent_update', { agentType: key });
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
    const unlisten = listen<{ agent: string; message: string; progress: number }>('install-progress', (event) => {
      addLog(event.payload.message, 'info');
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [addLog]);

  // Latest version detection is now handled in Phase 2 of detect_env
  // (auto-detected via envInfo.agentLatestVersions)

  const fetchEnvWithLog = useCallback(async () => {
    // Force fresh detection by clearing cache
    await invoke('clear_env_detect_cache').catch(() => {});

    // Track which base tools have been logged (env-base-tools emits 3 times, only log actual detections)
    const loggedTools = new Set<string>();
    const unlistenBase = await listen<Record<string, string | null>>('env-base-tools', (event) => {
      const data = event.payload;
      if (data.nodeVersion && !loggedTools.has('node')) {
        loggedTools.add('node');
        addLog(`Node.js: ${data.nodeVersion}`, 'success');
      }
      if (data.gitVersion && !loggedTools.has('git')) {
        loggedTools.add('git');
        addLog(`Git: ${data.gitVersion}`, 'success');
      }
      if (data.pythonVersion && !loggedTools.has('python')) {
        loggedTools.add('python');
        addLog(`Python: ${data.pythonVersion}`, 'success');
      }
    });

    const unlistenAgent = await listen<{ agentType: string; version: string | null }>('env-agent-version', (event) => {
      const { agentType, version } = event.payload;
      const agent = agents.find(a => a.agentType === agentType);
      const name = agent?.displayName || agentType;
      addLog(`${name}: ${version || '未安装'}`, version ? 'success' : 'warn');
    });

    try {
      await fetchEnv();
    } finally {
      // Clean up temporary listeners (even if fetchEnv fails)
      unlistenBase();
      unlistenAgent();
    }

    addLog('环境检测完成', 'success');
  }, [fetchEnv, addLog, agents]);

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

  // Build dependencies: prerequisites + dynamic agents
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
      detecting: envLoading && !envInfo?.nodeVersion,
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
      detecting: envLoading && !envInfo?.gitVersion,
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
      detecting: envLoading && !envInfo?.pythonVersion,
    },
    // Dynamic agents from DB
    ...agents.filter(a => a.isEnabled).sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0)).map((agent) => {
      const version = envInfo?.agentVersions?.[agent.agentType] ?? null;
      const latestVer = envInfo?.agentLatestVersions?.[agent.agentType] ?? latestVersions[agent.agentType]?.version ?? null;
      const latestRelease = latestVersions[agent.agentType]?.releaseTime ?? null;
      const updateCheck = updateChecking[agent.agentType] ?? false;
      return {
        name: agent.displayName,
        key: agent.agentType,
        version,
        installed: !!version,
        action: version ? 'update' as const : 'install' as const,
        installing: installing === agent.agentType,
        latestVersion: latestVer,
        latestReleaseTime: latestRelease,
        hasUpdate: !!version && !!latestVer && isOlder(version, latestVer),
        updateChecking: updateCheck,
        detecting: envLoading && agentStatus[agent.agentType] === undefined,
      };
    }),
  ];

  const handleInstall = async (key: string) => {
    const agent = agents.find(a => a.agentType === key);
    const name = agent?.displayName || key;
    setInstalling(key);
    addLog(`开始安装 ${name}...`, 'info');
    try {
      await invoke('install_agent', { agentType: key });
      addLog(`${name} 安装成功`, 'success');
      await fetchEnv();
      // 安装完成后直接触发版本检测（autoDetected 守卫阻止 useEffect 重复执行）
      checkAgentUpdate(key);
    } catch (err) {
      const msg = errorMessage(err);
      addLog(`${name} 安装失败: ${msg}`, 'error');
    }
    setInstalling(null);
  };

  const handleUninstall = async (key: string) => {
    const agent = agents.find(a => a.agentType === key);
    const name = agent?.displayName || key;
    setInstalling(key);
    addLog(`开始卸载 ${name}...`, 'info');
    try {
      await invoke('uninstall_agent', { agentType: key });
      addLog(`${name} 卸载成功`, 'success');
      await fetchEnv();
      setLatestVersions((prev) => ({ ...prev, [key]: null }));
    } catch (err) {
      const msg = errorMessage(err);
      addLog(`${name} 卸载失败: ${msg}`, 'error');
    }
    setInstalling(null);
  };

  const getAgentName = (key: string) => {
    const agent = agents.find(a => a.agentType === key);
    return agent?.displayName || key;
  };

  /** 安装/更新/卸载前的统一二次确认（走全局确认弹窗；卸载为危险动作用红色确认键）。 */
  const requestEnvAction = async (
    key: string,
    action: 'install' | 'update' | 'uninstall' | 'reinstall',
  ) => {
    const name = getAgentName(key);
    const title =
      action === 'uninstall' ? '确认卸载'
      : action === 'update' ? '确认更新'
      : action === 'reinstall' ? '确认重装'
      : '确认安装';
    const message =
      action === 'uninstall' ? `确定要卸载 ${name} 吗？此操作将从系统中移除该工具。`
      : action === 'update' ? `确定要更新 ${name} 到最新版本吗？`
      : action === 'reinstall' ? `确定要重新安装 ${name} 吗？`
      : `确定要安装 ${name} 吗？`;
    const confirmText =
      action === 'uninstall' ? '卸载' : action === 'update' ? '更新' : action === 'reinstall' ? '重装' : '安装';
    const ok = await confirmDialog({
      title,
      message,
      confirmText,
      danger: action === 'uninstall',
    });
    if (!ok) return;
    if (action === 'uninstall') await handleUninstall(key);
    else await handleInstall(key);
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
                {dep.detecting && <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
              </div>
              <span className="text-[10px]" style={{ color: dep.installed ? 'var(--text-secondary)' : '#EF4444' }}>
                {dep.detecting ? '检查中' : (dep.version ?? '未安装')}
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
              <div className="flex items-center gap-2 min-w-0">
                <SettingsStatusIcon installed={dep.installed} />
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
                      {dep.name}
                    </span>
                    {dep.detecting && <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
                    {dep.hasUpdate && (
                      <ArrowUpCircle size={13} style={{ color: '#F59E0B' }} />
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    <span className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                      {dep.detecting ? '检查中' : (dep.installed ? `v${dep.version}` : '未安装')}
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
                    onClick={() => requestEnvAction(dep.key, 'install')}
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
                  <>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'update')}
                      disabled={!!dep.installing}
                      variant="warning"
                    >
                      {dep.installing ? (
                        <><Loader2 size={11} className="animate-spin" /> 更新中</>
                      ) : (
                        <><ArrowUpCircle size={11} /> 更新</>
                      )}
                    </SettingsButton>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'uninstall')}
                      disabled={!!dep.installing}
                      variant="danger"
                      icon={<Trash2 size={11} />}
                    >
                      卸载
                    </SettingsButton>
                  </>
                )}
                {dep.installed && !dep.hasUpdate && dep.latestVersion && (
                  <>
                    <span className="text-[10px]" style={{ color: '#10B981' }}>
                      已是最新
                    </span>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'uninstall')}
                      disabled={!!dep.installing}
                      variant="danger"
                      icon={<Trash2 size={11} />}
                    >
                      卸载
                    </SettingsButton>
                  </>
                )}
                {dep.installed && !dep.hasUpdate && !dep.latestVersion && !dep.updateChecking && (
                  <>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'reinstall')}
                      disabled={!!dep.installing}
                      variant="secondary"
                      icon={<RefreshCw size={11} />}
                    >
                      重装
                    </SettingsButton>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'uninstall')}
                      disabled={!!dep.installing}
                      variant="danger"
                      icon={<Trash2 size={11} />}
                    >
                      卸载
                    </SettingsButton>
                  </>
                )}
              </div>
            </SettingsCard>
          ))}
        </div>
      </SettingsSection>

      {/* Refresh button */}
      <SettingsButton
        onClick={fetchEnvWithLog}
        disabled={envLoading}
        variant="secondary"
        icon={<RefreshCw size={12} className={envLoading ? 'animate-spin' : ''} />}
      >
        重新检测环境
      </SettingsButton>

      {/* Install Log */}
      <InstallLog logs={logs} isActive={!!installing} onClear={() => { setLogs([]); invoke('clear_logs').catch(() => {}); }} />
    </div>
  );
}
