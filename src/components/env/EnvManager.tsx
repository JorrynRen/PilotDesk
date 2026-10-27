import { useState, useEffect, useCallback, type CSSProperties } from 'react';
import { Download, RefreshCw, Loader2, ArrowUpCircle, Trash2, Copy, Check } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { InstallLog } from './InstallLog';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { SettingsSection, SettingsCard, SettingsButton, SettingsStatusIcon } from '../settings';
import { errorMessage } from '../../utils/errorMessage';
import { needsUnverifiedConsent } from '../../utils/agentIntegrity';

interface DependencyStatus {
  name: string;
  key: string;
  version: string | null;
  installed: boolean;
  /** 启动命令是否可解析（已安装 ≠ 可用：适配器装了但运行时缺失时不可用） */
  available?: boolean;
  /** 不可用/未安装的原因 */
  healthReason?: string | null;
  /** 是否有可执行的安装规格（市场下发的 lifecycle）；手工配置的 Agent 为 false */
  installable?: boolean;
  /** 卸载对象文案：ACP 是「卸载适配器」，本地 CLI 是「卸载 CLI 包」 */
  uninstallLabel?: string;
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

/** 前置依赖的等级：必需缺失会阻断 Agent 能力，推荐缺失只影响对应功能，但两者都给出安装引导。 */
type PrereqTier = '必需' | '推荐';

interface PrerequisiteMeta {
  key: 'node' | 'python' | 'git';
  name: string;
  tier: PrereqTier;
  /** 用途说明（已安装时也展示，让用户知道为什么需要它） */
  note: string;
  url: string;
  downloadLabel: string;
  /** Windows 可复制的安装命令（其他平台只给官方下载页） */
  command: string;
}

const PREREQUISITES: PrerequisiteMeta[] = [
  {
    key: 'node',
    name: 'Node.js',
    tier: '必需',
    note: '内置预置（claude / codex）与 ACP 市场多数条目通过 npm / npx 安装和运行，缺失时这些 Agent 既装不上也起不来。',
    url: 'https://nodejs.org/zh-cn/download',
    downloadLabel: '下载 Node.js（LTS）',
    command: 'winget install OpenJS.NodeJS.LTS',
  },
  {
    key: 'python',
    name: 'Python',
    tier: '推荐',
    note: '内置文档技能（Word / Excel / PPT / PDF）与 execute_python 工具依赖它；缺失时这些能力不可用，其余功能不受影响。安装时请勾选 Add python.exe to PATH。',
    url: 'https://www.python.org/downloads/',
    downloadLabel: '下载 Python 3',
    command: 'winget install Python.Python.3.12',
  },
  {
    key: 'git',
    name: 'Git',
    tier: '推荐',
    note: '用于把当前仓库的分支、最新提交与未提交改动作为上下文提供给 AI；缺失时自动降级，不影响启动、建会话与对话。',
    url: 'https://git-scm.com/download/win',
    downloadLabel: '下载 Git',
    command: 'winget install Git.Git',
  },
];

const TIER_STYLE: Record<PrereqTier, CSSProperties> = {
  必需: { color: '#EF4444', backgroundColor: 'rgba(239, 68, 68, 0.10)' },
  推荐: { color: '#F59E0B', backgroundColor: 'rgba(245, 158, 11, 0.10)' },
};

export type { EnvManagerProps };

export function EnvManager({ onComplete: _onComplete }: EnvManagerProps) {
  const { envInfo, loading: envLoading, agentStatus, refresh: fetchEnv } = useEnvInfo();
  // Agent 列表是全局单例缓存：本页装/卸完必须让它重拉，
  // 否则「设置 › 集成配置」里的包状态（待安装 / 已就绪）一直停在旧值。
  const { agents, fetchAgents } = useAgentRegistry();
  const [installing, setInstalling] = useState<string | null>(null);
  const [logs, setLogs] = useState<Array<{ timestamp: number; message: string; level: 'info' | 'warn' | 'error' | 'success' }>>([]);
  /** Latest versions from remote registries */
  const [latestVersions, setLatestVersions] = useState<Record<string, { version: string; releaseTime: string | null } | null>>({});
  const [updateChecking, setUpdateChecking] = useState<Record<string, boolean>>({});
  /** 刚复制过安装命令的前置依赖（用于按钮短暂变成「已复制」） */
  const [copiedPrereq, setCopiedPrereq] = useState<string | null>(null);
  const isWindows = typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows');

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

  const copyPrereqCommand = useCallback((key: string, command: string) => {
    navigator.clipboard.writeText(command).then(() => {
      setCopiedPrereq(key);
      setTimeout(() => setCopiedPrereq((prev) => (prev === key ? null : prev)), 1500);
    }).catch(() => {});
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

  // 接入方式速查（agentType → transport）：卸载的对象要按接入方式说清楚，
  // ACP 卸的是适配器、本地 CLI 卸的是它自己那个包。AgentConfig 不承载这一列，单独取。
  const [transportMap, setTransportMap] = useState<Record<string, string>>({});
  useEffect(() => {
    let alive = true;
    invoke<Array<{ agentType: string; transport: string }>>('list_agent_transports')
      .then((rows) => {
        if (!alive) return;
        const m: Record<string, string> = {};
        rows.forEach((r) => { m[r.agentType] = r.transport; });
        setTransportMap(m);
      })
      .catch(() => {
        // 读不到就按本地 CLI 处理，不影响检测
      });
    return () => { alive = false; };
  }, []);
  const isAcpAgent = (agentType: string) => transportMap[agentType] === 'acp';

  // 本地 CLI 市集里「同名且可安装」的预置：历史手工创建的 Agent 靠它接管安装/更新/卸载
  const [presetInstallable, setPresetInstallable] = useState<Record<string, boolean>>({});
  useEffect(() => {
    let alive = true;
    invoke<Array<{ agentType: string; supported: boolean }>>('agent_market_list', { category: 'cli' })
      .then((rows) => {
        if (!alive) return;
        const m: Record<string, boolean> = {};
        rows.forEach((r) => { m[r.agentType] = !!r.supported; });
        setPresetInstallable(m);
      })
      .catch(() => {
        // 市集不可用就维持「手工配置」，不影响环境检测本身
      });
    return () => { alive = false; };
  }, []);

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

  // 前置依赖（Node / Python / Git）在「前置依赖」区块单独渲染，这里只构造 Agent 行
  const agentDependencies: DependencyStatus[] = agents
    .filter(a => a.isEnabled)
    .sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0))
    .map((agent) => {
      const version = envInfo?.agentVersions?.[agent.agentType] ?? null;
      // 安装（包/适配器）与可用（启动命令）分开：ACP 装好适配器但运行时缺失时显示「不可用」
      const health = envInfo?.agentHealth?.[agent.agentType];
      const installed = health?.installed ?? !!version;
      const available = health?.available ?? !!version;
      const latestVer = envInfo?.agentLatestVersions?.[agent.agentType] ?? latestVersions[agent.agentType]?.version ?? null;
      const latestRelease = latestVersions[agent.agentType]?.releaseTime ?? null;
      const updateCheck = updateChecking[agent.agentType] ?? false;
      return {
        name: agent.displayName,
        key: agent.agentType,
        version,
        installed,
        available,
        healthReason: health?.reason ?? null,
        // 安装/卸载命令来自市集下发的 lifecycle；历史手工 Agent 若市集有同名可安装预置，
        // 按「接管」处理（后端真正执行前会先把预置规格写回本地）
        installable: !!agent.lifecycle || presetInstallable[agent.agentType] === true,
        uninstallLabel: isAcpAgent(agent.agentType) ? '卸载适配器' : '卸载 CLI 包',
        action: installed ? 'update' as const : 'install' as const,
        installing: installing === agent.agentType,
        latestVersion: latestVer,
        latestReleaseTime: latestRelease,
        hasUpdate: !!version && !!latestVer && isOlder(version, latestVer),
        updateChecking: updateCheck,
        detecting: envLoading && agentStatus[agent.agentType] === undefined,
      };
    });

  const handleInstall = async (key: string, mode: 'install' | 'update' = 'install', allowUnverified = false) => {
    const agent = agents.find(a => a.agentType === key);
    const name = agent?.displayName || key;
    setInstalling(key);
    addLog(`开始${mode === 'update' ? '更新' : '安装'} ${name}...`, 'info');
    try {
      // 市集下发的 Agent 走完整安装流程（解析 bin、回写运行时、置 ready）；
      // 历史手工 Agent 若市集有同名预置也走它（后端先接管预置规格再安装）；
      // 其余（如 Git / Python 等非 Agent 工具）仍走通用的 install_agent。
      if (agent?.pkgState || presetInstallable[key]) {
        await invoke(mode === 'update' ? 'agent_market_update' : 'agent_market_install', {
          agentType: key,
          allowUnverified,
        });
      } else {
        await invoke('install_agent', { agentType: key, allowUnverified });
      }
      addLog(`${name} ${mode === 'update' ? '更新' : '安装'}成功`, 'success');
      // 两处状态都要刷：环境检测（版本/可用性）与 Agent 列表（包状态/lifecycle）
      await Promise.all([fetchEnv(), fetchAgents()]);
      // 安装完成后直接触发版本检测（autoDetected 守卫阻止 useEffect 重复执行）
      checkAgentUpdate(key);
    } catch (err) {
      const msg = errorMessage(err);
      addLog(`${name} ${mode === 'update' ? '更新' : '安装'}失败: ${msg}`, 'error');
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
      // 同安装：包状态与本机版本分属两份缓存，都要刷新
      await Promise.all([fetchEnv(), fetchAgents()]);
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
    // 卸载对象按接入方式说清楚：ACP 卸的是适配器，本地 CLI 卸的是包
    const uninstallTarget = agents.some((a) => a.agentType === key)
      ? isAcpAgent(key) ? 'ACP 适配器' : '本地 CLI 包'
      : null;
    const title =
      action === 'uninstall' ? '确认卸载'
      : action === 'update' ? '确认更新'
      : action === 'reinstall' ? '确认重装'
      : '确认安装';
    const message =
      action === 'uninstall'
        ? uninstallTarget
          ? `确定要卸载 ${name} 的${uninstallTarget}吗？Agent 配置会保留，卸载后需重新安装才能用于会话。`
          : `确定要卸载 ${name} 吗？此操作将从系统中移除该工具。`
      : action === 'update' ? `确定要更新 ${name} 到最新版本吗？`
      : action === 'reinstall' ? `确定要重新安装 ${name} 吗？`
      : `确定要安装 ${name} 吗？`;
    // ACP 官方目录只提供**适配器**的安装信息，但适配器与本体不一定分离：
    // 分离式（codex / claude）装完还要另装宿主 CLI 并登录；内置式（hermes）适配器
    // 就在本体内，更新适配器即更新本体。不提前说清楚，用户会误判装完能不能用。
    const acpCaution =
      action !== 'uninstall' && isAcpAgent(key)
        ? '\n\n注意：ACP 适配器与 Agent 本体的关系因厂商而异——\n· 分离式（如 Codex、Claude）：这里只安装适配器，不包含本体；本体若需要单独的命令行工具或账号，请自行安装并在终端完成登录，否则建立会话会在握手阶段失败；\n· 内置式（如 Hermes）：适配器内置于本体，更新适配器即更新本体。'
        : '';
    // 二进制安装包没有 registry 校验值：后端会先找厂商校验文件，取不到就不校验。
    const unverified = needsUnverifiedConsent(
      agents.find((a) => a.agentType === key)?.lifecycle
    );
    const integrityCaution =
      action !== 'uninstall' && unverified
        ? '\n\n注意：该安装包未提供校验值，安装时会先尝试厂商发布的校验文件；若仍取不到，将不校验完整性直接安装。仅在你信任该来源时继续。'
        : '';
    const confirmText =
      action === 'uninstall' ? '卸载' : action === 'update' ? '更新' : action === 'reinstall' ? '重装' : '安装';
    const ok = await confirmDialog({
      title,
      message: `${message}${acpCaution}${integrityCaution}`,
      confirmText: unverified ? '仍然安装' : confirmText,
      danger: action === 'uninstall',
    });
    if (!ok) return;
    if (action === 'uninstall') await handleUninstall(key);
    else await handleInstall(key, action === 'update' ? 'update' : 'install', unverified);
  };

  return (
    <div className="space-y-6">
      {/* Prerequisites：必需项缺失会阻断 Agent 能力，推荐项缺失只影响对应功能，两者都给安装引导 */}
      <SettingsSection title="前置依赖">
        <div className="space-y-2">
          {PREREQUISITES.map((meta) => {
            const version =
              meta.key === 'node'
                ? envInfo?.nodeVersion
                : meta.key === 'python'
                  ? envInfo?.pythonVersion
                  : envInfo?.gitVersion;
            const installed = !!version;
            const detecting = envLoading && !installed;
            return (
              <SettingsCard key={meta.key} className="items-start">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <SettingsStatusIcon installed={installed} />
                    <span className="text-xs" style={{ color: 'var(--text-primary)' }}>
                      {meta.name}
                    </span>
                    <span
                      className="text-[10px] px-1.5 py-0.5 rounded"
                      style={TIER_STYLE[meta.tier]}
                    >
                      {meta.tier}
                    </span>
                    {detecting && <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
                    <span
                      className="text-[10px]"
                      style={{ color: installed ? 'var(--text-secondary)' : meta.tier === '必需' ? '#EF4444' : '#F59E0B' }}
                    >
                      {detecting ? '检查中' : (version ?? '未安装')}
                    </span>
                  </div>
                  <div className="text-[10px] mt-1 leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                    {meta.note}
                  </div>
                  {!installed && !detecting && (
                    <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                      <SettingsButton
                        variant="secondary"
                        onClick={() => { openUrl(meta.url).catch(() => {}); }}
                        icon={<Download size={11} />}
                      >
                        {meta.downloadLabel}
                      </SettingsButton>
                      {isWindows && (
                        <>
                          <SettingsButton
                            variant="ghost"
                            onClick={() => copyPrereqCommand(meta.key, meta.command)}
                            icon={copiedPrereq === meta.key ? <Check size={11} /> : <Copy size={11} />}
                          >
                            {copiedPrereq === meta.key ? '已复制' : '复制命令'}
                          </SettingsButton>
                          <code
                            className="text-[10px] px-1.5 py-0.5 rounded"
                            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)' }}
                          >
                            {meta.command}
                          </code>
                        </>
                      )}
                    </div>
                  )}
                </div>
              </SettingsCard>
            );
          })}
          {!envLoading && !envInfo?.nodeVersion && (
            <div
              className="text-[10px] leading-relaxed px-3 py-2 rounded-lg"
              style={{ backgroundColor: 'rgba(239, 68, 68, 0.08)', color: '#EF4444' }}
            >
              Node.js 是唯一必需的前置依赖：装上后请重启应用（让新的 PATH 生效），否则依赖 npm / npx 的 Agent 既无法安装也无法建立会话。
            </div>
          )}
        </div>
      </SettingsSection>

      {/* Agent Detection with Update Check */}
      <SettingsSection
        title="Agent 检测"
        actions={
          <SettingsButton
            onClick={fetchEnvWithLog}
            disabled={envLoading}
            variant="secondary"
            icon={<RefreshCw size={12} className={envLoading ? 'animate-spin' : ''} />}
          >
            重新检测环境
          </SettingsButton>
        }
      >
        <div className="space-y-2">
          {agentDependencies.map((dep) => (
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
                    <span
                      className="text-[10px]"
                      style={{ color: !dep.installed || dep.available === false ? '#EF4444' : 'var(--text-secondary)' }}
                      title={!dep.detecting ? (dep.healthReason ?? undefined) : undefined}
                    >
                      {dep.detecting
                        ? '检查中'
                        : !dep.installed
                          ? '未安装'
                          : dep.available === false
                            ? '不可用'
                            : dep.version
                              ? `v${dep.version}`
                              : '可用（版本未知）'}
                    </span>
                    {/* 不可用原因直接展示：登录/授权这类问题藏在 hover 里用户根本发现不了 */}
                    {!dep.detecting && dep.installed && dep.available === false && dep.healthReason && (
                      <span
                        className="text-[10px] truncate max-w-[240px]"
                        style={{ color: '#EF4444' }}
                        title={dep.healthReason}
                      >
                        {dep.healthReason}
                      </span>
                    )}
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
                {!dep.installed && dep.installable !== false && (
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
                {!dep.installed && dep.installable === false && (
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    手工配置
                  </span>
                )}
                {dep.hasUpdate && dep.installed && dep.installable !== false && (
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
                      title={`${dep.uninstallLabel ?? '卸载'}（保留 Agent 配置，卸载后需重新安装才能用于会话）`}
                    >
                      {dep.uninstallLabel ?? '卸载'}
                    </SettingsButton>
                  </>
                )}
                {dep.hasUpdate && dep.installed && dep.installable === false && (
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }} title="手工配置的 Agent 没有安装规格，请在系统内自行升级">
                    手工配置
                  </span>
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
                      title={`${dep.uninstallLabel ?? '卸载'}（保留 Agent 配置，卸载后需重新安装才能用于会话）`}
                    >
                      {dep.uninstallLabel ?? '卸载'}
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
                      title={`${dep.uninstallLabel ?? '卸载'}（保留 Agent 配置，卸载后需重新安装才能用于会话）`}
                    >
                      {dep.uninstallLabel ?? '卸载'}
                    </SettingsButton>
                  </>
                )}
              </div>
            </SettingsCard>
          ))}
        </div>
      </SettingsSection>

      {/* Install Log */}
      <InstallLog logs={logs} isActive={!!installing} onClear={() => { setLogs([]); invoke('clear_logs').catch(() => {}); }} />
    </div>
  );
}
