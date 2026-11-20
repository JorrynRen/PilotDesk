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
  /** 本轮检测是否给出了结论（false 表示没测到，不能当成「未安装」） */
  detected: boolean;
  /** 启动命令是否可解析（已安装 ≠ 可用：适配器装了但运行时缺失时不可用） */
  available?: boolean;
  /** 不可用/未安装的原因 */
  healthReason?: string | null;
  /** 已安装但探测异常（--version 超时/非零退出），值为原因 */
  broken?: string | null;
  /** 是否有可执行的安装规格（市场下发的 lifecycle）；手工配置的 Agent 为 false */
  installable?: boolean;
  /** 适配器是否内置于宿主：决定环境检测页升级/卸载的是「适配器」还是「宿主」 */
  adapterEmbedded?: boolean;
  /** 本页动作对象文案：适配器 / 宿主 */
  targetLabel: '适配器' | '宿主';
  /** 卸载对象文案：独立适配器是「卸载适配器」，内置适配器/CLI 是「卸载宿主」 */
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
  const { envInfo, loading: envLoading, error: envError, agentStatus, refresh: fetchEnv } = useEnvInfo();
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
  // API 提供商（http 接入）不是可安装的 Agent：没有版本/启动命令，放进「Agent 检测」
  // 只会永远显示「未安装」。它们归「提供商」页管理。
  const isApiProvider = (agentType: string) =>
    agentType === 'api' || agentType.startsWith('api:') || transportMap[agentType] === 'http';

  // 市场条目的形态信息（按接入方式分开）：本页需要知道「适配器是否内置于宿主」，
  // 以便把动作说成「更新适配器」还是「更新宿主」，并把请求交给后端按形态分流。
  const [marketForms, setMarketForms] = useState<{
    acp: Record<string, { supported: boolean; adapterEmbedded: boolean }>;
    cli: Record<string, { supported: boolean; adapterEmbedded: boolean }>;
  }>({ acp: {}, cli: {} });
  useEffect(() => {
    let alive = true;
    Promise.all(
      (['acp', 'cli'] as const).map((category) =>
        invoke<Array<{ agentType: string; supported: boolean; adapterEmbedded?: boolean }>>(
          'agent_market_list',
          { category }
        ).catch(() => [])
      )
    )
      .then(([acp, cli]) => {
        if (!alive) return;
        const toMap = (rows: Array<{ agentType: string; supported: boolean; adapterEmbedded?: boolean }>) => {
          const m: Record<string, { supported: boolean; adapterEmbedded: boolean }> = {};
          rows.forEach((r) => {
            m[r.agentType] = { supported: !!r.supported, adapterEmbedded: !!r.adapterEmbedded };
          });
          return m;
        };
        // 按接入方式分开存：同名 ACP/CLI 条目互不覆盖
        setMarketForms({ acp: toMap(acp), cli: toMap(cli) });
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

    const unlistenAgent = await listen<{ agentType: string; version: string | null; installed?: boolean; broken?: string | null }>('env-agent-version', (event) => {
      const { agentType, version, installed, broken } = event.payload;
      if (isApiProvider(agentType)) return;
      const agent = agents.find(a => a.agentType === agentType);
      const name = agent?.displayName || agentType;
      // 日志按「是否已安装」判定：版本探测失败（网络/超时）时 version 为 null，
      // 但 Agent 可能已装可用，不能一律记成「未安装」。
      const ok = installed ?? version !== null;
      addLog(
        `${name}: ${ok ? (version ? `v${version}` : broken ? `安装异常（${broken}）` : '已安装（版本未知）') : '未安装'}`,
        ok ? 'success' : 'warn',
      );
    });

    let ok = false;
    try {
      ok = (await fetchEnv()) !== null;
    } finally {
      // Clean up temporary listeners (even if fetchEnv fails)
      unlistenBase();
      unlistenAgent();
    }

    addLog(ok ? '环境检测完成' : '环境检测失败', ok ? 'success' : 'error');
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
    .filter(a => a.isEnabled && !isApiProvider(a.agentType))
    .sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0))
    .map((agent) => {
      const version = envInfo?.agentVersions?.[agent.agentType] ?? null;
      // 安装（包/适配器）与可用（启动命令）分开：ACP 装好适配器但运行时缺失时显示「不可用」
      const health = envInfo?.agentHealth?.[agent.agentType];
      // 有 health 或拿到版本号才算「测到了」；否则是检测没覆盖到，不能武断显示「未安装」
      const detected = !!health || version !== null;
      const installed = health?.installed ?? !!version;
      const available = health?.available ?? !!version;
      const latestVer = envInfo?.agentLatestVersions?.[agent.agentType] ?? latestVersions[agent.agentType]?.version ?? null;
      const latestRelease = latestVersions[agent.agentType]?.releaseTime ?? null;
      const updateCheck = updateChecking[agent.agentType] ?? false;
      // 形态：ACP 看适配器是否内置；兼容模式本身就是 CLI
      const acp = isAcpAgent(agent.agentType);
      const form = (acp ? marketForms.acp : marketForms.cli)[agent.agentType];
      const adapterEmbedded = acp ? (form?.adapterEmbedded ?? true) : true;
      // 兼容模式与内置适配器统一称为「宿主」：CLI 本体就是宿主
      const targetLabel: DependencyStatus['targetLabel'] = acp && !adapterEmbedded ? '适配器' : '宿主';
      return {
        name: agent.displayName,
        key: agent.agentType,
        version,
        installed,
        detected,
        available,
        healthReason: health?.reason ?? null,
        broken: installed ? (health?.broken ?? null) : null,
        // 安装/卸载命令来自市集下发的 lifecycle；历史手工 Agent 若市集有同名可安装预置，
        // 按「接管」处理（后端真正执行前会先把预置规格写回本地）
        installable: !!agent.lifecycle || form?.supported === true,
        adapterEmbedded,
        targetLabel,
        uninstallLabel: !acp ? '卸载宿主' : adapterEmbedded ? '卸载宿主' : '卸载适配器',
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
      // 后端按适配器形态分流：
      // - 独立适配器：写配置 + 装/升级适配器（原子）；
      // - 内置适配器/兼容模式：只装/升级宿主，不写配置。
      await invoke('agent_environment_install', {
        agentType: key,
        allowUnverified,
        update: mode === 'update',
      });
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
    const acp = isAcpAgent(key);
    const embedded = acp ? (marketForms.acp[key]?.adapterEmbedded ?? true) : true;
    const target: '适配器' | '宿主' = !acp || embedded ? '宿主' : '适配器';
    // 卸载对象按形态说清楚：独立适配器、内置适配器的宿主、兼容模式 CLI 都按宿主处理
    const uninstallTarget = !acp
      ? '宿主'
      : embedded
        ? '宿主（含内置适配器）'
        : 'ACP 适配器';
    const title =
      action === 'uninstall' ? '确认卸载'
      : action === 'update' ? `确认更新${target}`
      : action === 'reinstall' ? `确认重装${target}`
      : `确认安装${target}`;
    const message =
      action === 'uninstall'
        ? `确定要卸载 ${name} 的${uninstallTarget}吗？Agent 配置会保留，卸载后需重新安装才能用于会话。`
        : action === 'update' ? `确定要把 ${name} 的${target}更新到最新版本吗？${acp && !embedded ? '会同时写入市场配置。' : '不会改动本地配置。'}`
        : action === 'reinstall' ? `确定要重新安装 ${name} 的${target}吗？${acp && !embedded ? '会同时写入市场配置。' : '不会改动本地配置。'}`
        : `确定要安装 ${name} 的${target}吗？${acp && !embedded ? '会同时写入市场配置。' : '不会改动本地配置。'}`;
    // 说清楚这一步到底动了什么，避免用户误判「装完就能用」：
    // - 独立适配器：适配器与本体分离，本体可能还要单独安装/登录；
    // - 内置适配器：适配器在宿主里，升级宿主即升级适配器；
    // - 兼容模式：没有适配器，装的就是本机 CLI。
    const formCaution =
      action !== 'uninstall' && acp && !embedded
        ? '\n\n注意：独立 ACP 适配器与 Agent 本体是分开的——这里会安装/更新适配器并写入市场配置；本体若需要单独安装命令行或登录，请自行完成，否则建立会话会在握手阶段失败。'
        : action !== 'uninstall' && acp && embedded
          ? '\n\n注意：该适配器内置于宿主——这一步安装/更新的是宿主本体，不会改动本地配置。'
          : action !== 'uninstall' && !acp
            ? '\n\n注意：该 Agent 以命令行方式接入——这一步安装/更新的是本机 CLI 宿主，不会改动本地配置。'
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
      message: `${message}${formCaution}${integrityCaution}`,
      confirmText: unverified ? '仍然安装' : confirmText,
      danger: action === 'uninstall',
    });
    if (!ok) return;
    if (action === 'uninstall') await handleUninstall(key);
    else await handleInstall(key, action === 'update' ? 'update' : 'install', unverified);
  };

  return (
    <div className="settings-page-stack">
      {envError && (
        <div
          className="flex items-center gap-2 rounded-md px-3 py-2 text-[11px]"
          style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
        >
          <span className="min-w-0 flex-1">环境检测失败：{envError}</span>
          <button
            type="button"
            className="pd-btn shrink-0 text-[10px]"
            style={{ color: 'var(--accent)' }}
            onClick={() => void fetchEnvWithLog()}
            disabled={envLoading}
          >
            重新检测
          </button>
        </div>
      )}
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
            // 还没拿到任何检测结果时不下「未安装」的结论
            const checked = installed || !!envInfo;
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
                      style={{
                        color: !checked
                          ? 'var(--text-tertiary)'
                          : installed
                            ? 'var(--text-secondary)'
                            : meta.tier === '必需'
                              ? '#EF4444'
                              : '#F59E0B',
                      }}
                    >
                      {detecting ? '检查中' : !checked ? '未检测' : (version ?? '未安装')}
                    </span>
                  </div>
                  <div className="text-[10px] mt-1 leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                    {meta.note}
                  </div>
                  {checked && !installed && !detecting && (
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
          {agentDependencies.length === 0 && (
            <p className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              没有需要检测的 Agent。API 提供商不参与本地检测，请在「提供商」页管理。
            </p>
          )}
          {agentDependencies.map((dep) => (
            <SettingsCard key={dep.name} highlight={dep.hasUpdate && dep.available !== false}>
              <div className="flex items-center gap-2 min-w-0">
                <SettingsStatusIcon installed={dep.detected && dep.installed} />
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
                      {dep.name}
                    </span>
                    {dep.detecting && <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
                    {dep.hasUpdate && dep.available !== false && (
                      <ArrowUpCircle size={13} style={{ color: '#F59E0B' }} />
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    <span
                      className="text-[10px]"
                      style={{
                        color: !dep.detected
                          ? 'var(--text-tertiary)'
                          : !dep.installed || dep.available === false
                            ? '#EF4444'
                            : 'var(--text-secondary)',
                      }}
                      title={!dep.detecting ? (dep.healthReason ?? undefined) : undefined}
                    >
                      {dep.detecting
                        ? '检查中'
                        : !dep.detected
                          ? '未检测'
                          : !dep.installed
                            ? '未安装'
                            : dep.broken
                              ? '安装异常'
                              : dep.available === false
                                ? '不可用'
                                : dep.version
                                  ? `v${dep.version}`
                                  : '可用（版本未知）'}
                    </span>
                    {/* 不可用/安装异常的原因直接展示：登录/授权、版本探测失败这类问题藏在 hover 里用户根本发现不了 */}
                    {!dep.detecting && dep.installed && (dep.available === false || dep.broken) && dep.healthReason && (
                      <span
                        className="text-[10px] truncate max-w-[240px]"
                        style={{ color: dep.broken ? 'var(--status-warning)' : 'var(--status-danger)' }}
                        title={dep.healthReason}
                      >
                        {dep.healthReason}
                      </span>
                    )}
                    {dep.updateChecking && (
                      <Loader2 size={10} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
                    )}
                    {dep.latestVersion && !dep.updateChecking && (
                      <span className="text-[10px]" style={{ color: dep.hasUpdate ? 'var(--status-warning)' : 'var(--text-tertiary)' }}>
                        最新 v{dep.latestVersion}
                      </span>
                    )}
                    {dep.latestVersion && dep.latestReleaseTime && !dep.updateChecking && (
                      <span className="text-[10px]" style={{ color: dep.hasUpdate ? 'var(--status-warning)' : 'var(--text-tertiary)' }}>
                        ({dep.latestReleaseTime})
                      </span>
                    )}
                  </div>
                </div>
              </div>

              {/* Right: action buttons */}
              <div className="flex items-center gap-1.5 shrink-0">
                {dep.detected && !dep.installed && dep.installable !== false && (
                  <SettingsButton
                    onClick={() => requestEnvAction(dep.key, 'install')}
                    disabled={!!dep.installing}
                    variant="primary"
                  >
                    {dep.installing ? (
                      <><Loader2 size={11} className="animate-spin" /> 安装中</>
                    ) : (
                      <><Download size={11} /> 安装{dep.targetLabel}</>
                    )}
                  </SettingsButton>
                )}
                {dep.detected && !dep.installed && dep.installable === false && (
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    手工配置
                  </span>
                )}
                {/* 不可用（如 ACP 适配器在、宿主 CLI 缺失）时不给「更新/已是最新」结论：
                    版本再新也起不来，只提供「重装」这条修复路径 */}
                {dep.hasUpdate && dep.installed && dep.available !== false && !dep.broken && dep.installable !== false && (
                  <>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'update')}
                      disabled={!!dep.installing}
                      variant="warning"
                    >
                      {dep.installing ? (
                        <><Loader2 size={11} className="animate-spin" /> 更新中</>
                      ) : (
                        <><ArrowUpCircle size={11} /> 更新{dep.targetLabel}</>
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
                {dep.installed && !dep.hasUpdate && dep.latestVersion && dep.available !== false && !dep.broken && (
                  <>
                    <span className="text-[10px]" style={{ color: 'var(--status-success)' }}>
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
                {dep.installed && !dep.updateChecking && (dep.available === false || !!dep.broken || (!dep.hasUpdate && !dep.latestVersion)) && (
                  <>
                    <SettingsButton
                      onClick={() => requestEnvAction(dep.key, 'reinstall')}
                      disabled={!!dep.installing}
                      variant="secondary"
                      icon={dep.installing ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
                    >
                      {dep.installing ? '重装中…' : '重装'}
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
