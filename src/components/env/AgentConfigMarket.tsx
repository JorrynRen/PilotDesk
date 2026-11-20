import { useCallback, useEffect, useMemo, useState } from 'react';
import { Download, RefreshCw, Search, CheckCircle2, AlertCircle, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

import { SettingsCard, SettingsButton } from '../settings';
import { AgentIcon } from '../common/AgentIcon';
import { MarketLoadError } from '../market/MarketLoadError';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import type { LifecycleSpec } from '../../types';

/**
 * AgentConfigMarket — Agent 市集。
 *
 * 两类接入方式同架陈列：`ACP 协议接入`（官方目录 + 官方缺失的预置）与
 * `兼容模式接入`（直接以命令行子进程运行）。
 * 市集只负责「配置版本」与独立适配器的安装/升级：
 * - 独立适配器：安装/升级 = 写配置 + 装/升级适配器（原子）；
 * - 内置适配器/兼容模式：只拉/写配置，不执行安装命令（宿主安装由环境检测负责）。
 */

type MarketCategory = 'acp' | 'cli';

interface MarketPackageView {
  role: string;
  package: string;
  args?: string[];
}

interface MarketAgent {
  agentType: string;
  displayName: string;
  description: string;
  category: MarketCategory | string;
  transport: string;
  kind: string;
  version: string;
  icon?: string | null;
  source: 'acp-registry' | 'preset' | string;
  repository?: string | null;
  website?: string | null;
  license?: string | null;
  authors: string[];
  packages: MarketPackageView[];
  /** 完整安装规格：集成配置表单按同名条目预填用 */
  lifecycle?: LifecycleSpec;
  supported: boolean;
  unsupportedReason?: string | null;
  installed: boolean;
  /** 本机版本：独立适配器 = 探测到的适配器版本；内置适配器/兼容模式 = 本地配置版本 */
  installedVersion?: string | null;
  pkgStatus?: string | null;
  /** 适配器是否内置于宿主（内置时适配器与宿主版本同源） */
  adapterEmbedded?: boolean;
  /** 市场配置版本高于本机版本（方向感知，不会因"不相等"误报） */
  updateAvailable: boolean;
  /** 内置适配器/兼容模式：本地配置清单有新版（只同步配置） */
  configUpdateAvailable?: boolean;
}

const CATEGORY_TABS: { key: MarketCategory; label: string; hint: string }[] = [
  { key: 'acp', label: 'ACP 协议接入', hint: '通过 Agent Client Protocol 适配器接入' },
  { key: 'cli', label: '兼容模式接入', hint: '无需 ACP 适配器，直接以命令行子进程方式接入' },
];

/** 包状态文案：市场草稿的安装进度对用户可见。 */
function statusLabel(a: MarketAgent): { text: string; tone: 'ok' | 'warn' | 'error' } | null {
  if (!a.installed) return null;
  if (a.pkgStatus === 'failed') return { text: '安装失败', tone: 'error' };
  if (a.pkgStatus === 'pending_install') return { text: '待安装', tone: 'warn' };
  return { text: '已配置', tone: 'ok' };
}

interface MarketCategoryState {
  agents: MarketAgent[];
  loading: boolean;
  error: string | null;
}

const MARKET_STATE_INITIAL: Record<MarketCategory, MarketCategoryState> = {
  acp: { agents: [], loading: false, error: null },
  cli: { agents: [], loading: false, error: null },
};

export function AgentConfigMarket() {
  /** 全局 Agent 列表是单例缓存，获取后必须主动刷新，否则配置页看不到新条目。 */
  const { fetchAgents } = useAgentRegistry();
  /** 安装/更新后刷新全局环境信息（运行与策略页共用同一份 envInfo） */
  const { refresh: refreshEnv } = useEnvInfo();
  // 两个市场各自独立保存加载态：任一来源超时只影响自己的 Tab，不会让另一个市场空白。
  const [market, setMarket] = useState<Record<MarketCategory, MarketCategoryState>>(MARKET_STATE_INITIAL);
  // 动作级忙碌状态：`动作:agentType`，避免同条目的其他按钮一起转圈
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [category, setCategory] = useState<MarketCategory>('acp');
  const [keyword, setKeyword] = useState('');

  /** 拉取单个市场（`force` 只对 ACP 官方目录有意义）。 */
  const fetchCategory = useCallback(async (cat: MarketCategory, force = false) => {
    setMarket((prev) => ({ ...prev, [cat]: { ...prev[cat], loading: true, error: null } }));
    try {
      const list = await invoke<MarketAgent[]>('agent_market_list', { category: cat, force });
      setMarket((prev) => ({ ...prev, [cat]: { agents: list, loading: false, error: null } }));
    } catch (err) {
      setMarket((prev) => ({
        ...prev,
        [cat]: { ...prev[cat], loading: false, error: errorMessage(err) },
      }));
    }
  }, []);

  const refreshAll = useCallback(
    async (forceRegistry: boolean) => {
      await Promise.all([
        fetchCategory('acp', forceRegistry),
        // CLI 市场只读预置，官方目录刷新与它无关
        fetchCategory('cli', false),
      ]);
    },
    [fetchCategory]
  );

  useEffect(() => {
    // 不在 effect 体内同步 setState：推到微任务，避免级联渲染告警
    void Promise.resolve().then(() => refreshAll(false));
  }, [refreshAll]);

  const current = market[category];

  const counts = useMemo(
    () => ({ acp: market.acp.agents.length, cli: market.cli.agents.length }),
    [market]
  );

  const visible = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return current.agents.filter(
      (a) =>
        !kw ||
        a.displayName.toLowerCase().includes(kw) ||
        a.agentType.toLowerCase().includes(kw) ||
        a.description.toLowerCase().includes(kw) ||
        a.packages.some((p) => p.package.toLowerCase().includes(kw))
    );
  }, [current.agents, keyword]);

  const refreshCategoryOf = async (agent: MarketAgent) => {
    await fetchCategory(agent.category === 'cli' ? 'cli' : 'acp');
    void refreshEnv();
  };

  /** 「安装」：独立适配器连带安装；内置适配器/兼容模式只落配置（后端按形态分流）。 */
  const handleInstall = async (agent: MarketAgent) => {
    setBusyAction(`install:${agent.agentType}`);
    try {
      await invoke('agent_market_install', { agentType: agent.agentType });
      await fetchAgents();
      showToast(
        agent.category === 'acp' && !agent.adapterEmbedded
          ? `${agent.displayName} 的适配器已安装`
          : `${agent.displayName} 的配置已获取`,
        'success'
      );
    } catch (err) {
      showToast(`安装失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    await refreshCategoryOf(agent);
  };

  /** 「升级配置」：独立适配器 = 写配置 + 升级适配器（原子）；内置/兼容模式 = 只同步配置。 */
  const handleUpgrade = async (agent: MarketAgent) => {
    const independent = agent.category === 'acp' && !agent.adapterEmbedded;
    setBusyAction(`upgrade:${agent.agentType}`);
    try {
      await invoke(independent ? 'agent_market_update' : 'agent_market_apply_config', {
        agentType: agent.agentType,
      });
      await fetchAgents();
      showToast(`${agent.displayName} 的配置已升级`, 'success');
    } catch (err) {
      showToast(`升级失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    await refreshCategoryOf(agent);
  };

  /** 「重装」：独立适配器未探测到版本时的兜底入口，按当前市场清单重新安装。 */
  const handleReinstall = async (agent: MarketAgent) => {
    setBusyAction(`reinstall:${agent.agentType}`);
    try {
      await invoke('agent_market_install', { agentType: agent.agentType });
      await fetchAgents();
      showToast(`${agent.displayName} 已重新安装`, 'success');
    } catch (err) {
      showToast(`重装失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    await refreshCategoryOf(agent);
  };

  const renderAction = (agent: MarketAgent) => {
    if (!agent.supported) {
      return (
        <SettingsButton variant="secondary" disabled title={agent.unsupportedReason || '暂不支持'}>
          暂不支持
        </SettingsButton>
      );
    }
    const anyBusy = busyAction?.endsWith(`:${agent.agentType}`) ?? false;
    const installing = busyAction === `install:${agent.agentType}`;
    const upgrading = busyAction === `upgrade:${agent.agentType}`;
    const reinstalling = busyAction === `reinstall:${agent.agentType}`;
    const independent = agent.category === 'acp' && !agent.adapterEmbedded;
    // 未获取：独立适配器会连带安装，内置/兼容模式只落配置
    if (!agent.installed) {
      return (
        <SettingsButton
          variant="primary"
          icon={installing ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
          disabled={anyBusy}
          onClick={() => handleInstall(agent)}
          title={independent ? '获取配置并安装适配器' : '获取配置（不执行安装命令）'}
        >
          {installing ? '安装中' : '获取'}
        </SettingsButton>
      );
    }
    // 安装失败/待安装：重试安装
    if (agent.pkgStatus === 'pending_install' || agent.pkgStatus === 'failed') {
      return (
        <SettingsButton
          variant="primary"
          icon={installing ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
          disabled={anyBusy}
          onClick={() => handleInstall(agent)}
          title={independent ? '重新安装适配器' : '重新写入配置'}
        >
          {installing ? '处理中' : agent.pkgStatus === 'failed' ? (independent ? '重试安装' : '写入配置') : '完成安装'}
        </SettingsButton>
      );
    }
    if (agent.updateAvailable || agent.configUpdateAvailable) {
      return (
        <SettingsButton
          variant="update"
          icon={upgrading ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleUpgrade(agent)}
          title={
            independent
              ? `升级配置到 v${agent.version}，并同步升级适配器`
              : `升级本地配置到 v${agent.version}（不执行安装命令）`
          }
        >
          {upgrading ? '升级中' : '升级配置'}
        </SettingsButton>
      );
    }
    // 独立适配器没探测到本机版本：无法判断是否最新，给一个明确的重装入口
    if (independent && !agent.installedVersion) {
      return (
        <SettingsButton
          variant="secondary"
          icon={reinstalling ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleReinstall(agent)}
          title="未探测到本机适配器版本；按当前市场配置重新安装（Agent 配置保留）"
        >
          {reinstalling ? '重装中' : '重装'}
        </SettingsButton>
      );
    }
    return <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>已是最新</span>;
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* 分类 + 搜索 + 刷新目录：同一行（页面标题由「资源市集」页签承载，这里不再重复） */}
      <div className="pd-market-toolbar">
        {/* 二级分类：组合开关（一个凹槽容器 + 选中段高亮），与一级页签的 pd-tab 区分开 */}
        <div
          className="inline-flex items-center gap-0.5 p-0.5 rounded-full shrink-0"
          style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
          role="tablist"
        >
          {CATEGORY_TABS.map((t) => {
            const active = category === t.key;
            return (
              <button
                key={t.key}
                role="tab"
                aria-selected={active}
                onClick={() => setCategory(t.key)}
                className="pd-btn px-2.5 py-1 rounded-full text-[11px] inline-flex items-center gap-1 transition-colors"
                style={{
                  backgroundColor: active ? 'var(--accent-light)' : 'transparent',
                  color: active ? 'var(--accent)' : 'var(--text-secondary)',
                }}
                title={t.hint}
              >
                {t.label}
                <span
                  className="text-[10px]"
                  style={{ color: active ? 'var(--accent)' : 'var(--text-tertiary)' }}
                >
                  {counts[t.key]}
                </span>
              </button>
            );
          })}
        </div>
        <div className="relative flex-1 min-w-[160px] max-w-md">
          <Search
            size={13}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none"
            style={{ color: 'var(--text-tertiary)' }}
          />
          <input
            type="text"
            className="pd-market-search"
            placeholder="搜索名称 / 标识 / 包名"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
          />
        </div>
        <button
          type="button"
          className="pd-market-icon-button pd-btn ml-auto"
          onClick={() => refreshAll(true)}
          disabled={market.acp.loading || market.cli.loading}
          title="刷新两个市场的数据源（ACP 官方目录缓存 24 小时，过期自动后台更新）"
          aria-label="刷新 Agent 市场目录"
        >
          <RefreshCw size={11} className={market.acp.loading || market.cli.loading ? 'animate-spin' : undefined} />
        </button>
      </div>

      <div className="shrink-0 px-4 pb-1 pt-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
        {category === 'acp'
          ? '获取后会在本机安装对应适配器（npm 全局包）；安装完成前该 Agent 不可用于会话。'
          : '兼容模式配置通过命令行子进程调用，需确保本机已安装对应的 CLI Agent 且可用。'}
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3 pd-scroll-stable">
        <div className="space-y-2">
          {/* 每个市场独立报错：一侧数据源故障不影响另一侧的浏览与获取 */}
          {current.error && (
            <MarketLoadError
              error={current.error}
              fallbackTitle={`${category === 'acp' ? 'ACP 市场' : '兼容模式市场'}加载失败`}
              onRetry={() => { void fetchCategory(category, category === 'acp'); }}
            />
          )}
          {visible.length === 0 ? (
            <div className="text-xs py-2" style={{ color: 'var(--text-secondary)' }}>
              {current.loading ? '正在加载 Agent 配置...' : current.error ? '' : '没有匹配的条目'}
            </div>
          ) : (
            visible.map((agent) => {
              const status = statusLabel(agent);
              // ACP 官方目录的图标均为透明底黑色图形：固定放在浅色底板上，
              // 否则深色主题下几乎不可见、浅色主题下也会与卡片底色混在一起。
              const isRemoteIcon = !!agent.icon && /^https?:/i.test(agent.icon);
              return (
                <SettingsCard key={`market-${agent.agentType}`}>
                  <div className="flex items-start gap-3 w-full">
                    <div
                      className="w-7 h-7 rounded-md shrink-0 flex items-center justify-center overflow-hidden"
                      style={{
                        backgroundColor: isRemoteIcon ? '#F6F6F8' : 'var(--bg-secondary)',
                        border: '1px solid var(--border)',
                      }}
                    >
                      {/* 图标三种形态（远程 URL / 本地 file: / 文本）统一由 AgentIcon 处理 */}
                      <AgentIcon
                        icon={agent.icon || undefined}
                        size={20}
                        // 外层容器本身已是一块浅色底板（远程图标固定 #F6F6F8），再垫一层会变成双底
                        plate="none"
                        fallback={
                          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
                            {agent.displayName.slice(0, 1).toUpperCase()}
                          </span>
                        }
                      />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5 flex-wrap">
                        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                          {agent.displayName}
                        </span>
                        {/* 市场配置版本：统一无前缀，直接对应 agents-config/registry 条目的版本号 */}
                        <span
                          className="text-[10px]"
                          style={{ color: 'var(--text-tertiary)' }}
                          title="市场配置版本"
                        >
                          v{agent.version || '—'}
                        </span>
                        {/* 本机版本必须带前缀，避免和市场版本混在一起无法比较。
                            独立适配器 = 探测到的适配器版本；内置/兼容模式 = 本地配置版本 */}
                        <span
                          className="text-[10px]"
                          style={{
                            color: agent.installed
                              ? 'var(--text-secondary)'
                              : 'var(--text-tertiary)',
                          }}
                          title={
                            agent.adapterEmbedded || agent.category !== 'acp'
                              ? '本地配置版本'
                              : '本机探测到的适配器版本'
                          }
                        >
                          本机 {agent.installed ? (agent.installedVersion ? `v${agent.installedVersion}` : '未检测') : '未集成'}
                        </span>
                        {/* 来源徽标：ACP 官方用强调色，预置用中性色，避免与状态标记混在一起 */}
                        <span
                          className="text-[10px] px-1.5 py-0.5 rounded-full shrink-0"
                          style={
                            agent.source === 'acp-registry'
                              ? { backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }
                              : { backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }
                          }
                          title={agent.source === 'acp-registry' ? '来自 ACP 官方目录' : 'PilotDesk 内置预置'}
                        >
                          {agent.source === 'acp-registry' ? 'ACP 官方' : '预置'}
                        </span>
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          {agent.kind}
                        </span>
                        {status && (
                          <span
                            className="text-[10px] inline-flex items-center gap-0.5"
                            style={{
                              color:
                                status.tone === 'ok'
                                  ? 'var(--success, #16a34a)'
                                  : status.tone === 'error'
                                    ? 'var(--danger, #dc2626)'
                                    : 'var(--warning, #d97706)',
                            }}
                          >
                            {status.tone === 'ok' ? <CheckCircle2 size={10} /> : <AlertCircle size={10} />}
                            {status.text}
                          </span>
                        )}
                        {(agent.updateAvailable || agent.configUpdateAvailable) && (
                          <span
                            className="text-[10px] px-1.5 py-0.5 rounded-full shrink-0"
                            style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                            title={
                              agent.adapterEmbedded || agent.category !== 'acp'
                                ? `本地配置 v${agent.installedVersion || '未知'}，市场配置 v${agent.version}`
                                : `本机适配器 v${agent.installedVersion || '未知'}，市场 v${agent.version}（升级会同时安装适配器）`
                            }
                          >
                            配置可升级
                          </span>
                        )}
                      </div>
                      {agent.description && (
                        <div className="text-[10px] mt-0.5 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>
                          {agent.description}
                        </div>
                      )}
                      {agent.packages.length > 0 && (
                        <div className="text-[10px] mt-0.5 font-mono truncate" style={{ color: 'var(--text-tertiary)' }}>
                          {agent.kind === 'npm' ? 'npm' : agent.kind} · {agent.packages.map((p) => p.package).join(' + ')}
                        </div>
                      )}
                    </div>
                    <div className="shrink-0">{renderAction(agent)}</div>
                  </div>
                </SettingsCard>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
