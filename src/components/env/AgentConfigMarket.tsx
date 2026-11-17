import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { ReactNode } from 'react';
import { Download, RefreshCw, Search, CheckCircle2, AlertCircle, Loader2, ArrowRight } from 'lucide-react';
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
 * 「获取」只落草稿（后端记 pending_install 并下载图标），随即跳转到
 * 「设置 › 集成配置」的该 Agent 上继续完成安装 —— 安装本身是本地动作，不在这里静默做。
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
  installedVersion?: string | null;
  pkgStatus?: string | null;
  /** 用于版本比较的目录版本：ACP 官方 registryVersion，或运行时探测到的最新版本 */
  catalogVersion?: string | null;
  /** 目录版本来源：registry / probe / none（版本未知） */
  versionSource?: 'registry' | 'probe' | 'none' | string;
  /** ACP 官方目录的适配器版本；预置条目为空 */
  registryVersion?: string | null;
  /** 适配器是否内置于宿主（内置时适配器版本 == 配置版本） */
  adapterEmbedded?: boolean;
  /** 宿主最新版本（运行时探测）；内置型 ACP 条目不重复导出 */
  hostLatestVersion?: string | null;
  /** 目录版本明确高于本机版本（方向感知，不会因"不相等"误报） */
  updateAvailable: boolean;
  /** 版本无法比较（非标准版本号 / 缺本机版本）：给「强制更新」入口 */
  forceUpdateAvailable?: boolean;
  /** 市场配置清单有新版：只同步配置，与宿主/适配器版本更新无关 */
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
  // 没有 registryVersion、也没有可用的最新版本查询：不谎报「已是最新」。
  if (a.versionSource === 'none') return { text: '已安装 · 版本未知', tone: 'warn' };
  if (a.pkgStatus === 'ready') return { text: '已就绪', tone: 'ok' };
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
  const navigate = useNavigate();
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

  /** 「获取」：落草稿后立刻跳去集成配置完成安装（ACP 需要装适配器，不能在这里静默完成）。 */
  const handleAcquire = async (agent: MarketAgent) => {
    setBusyAction(`acquire:${agent.agentType}`);
    try {
      await invoke('agent_market_acquire', { agentType: agent.agentType });
      // 先刷新全局 Agent 列表：配置页的深层链接要靠它找到这条草稿。
      await fetchAgents();
      showToast(`已获取 ${agent.displayName}，请继续完成安装`, 'success');
      navigate(`/settings?tab=agents&agent=${encodeURIComponent(agent.agentType)}`);
    } catch (err) {
      showToast(`获取失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
  };

  const openInConfig = (agentType: string) => {
    navigate(`/settings?tab=agents&agent=${encodeURIComponent(agentType)}`);
  };

  /** 「升级」：用市场里的新版本规格覆盖本地配置并重新安装（ACP 适配器 / 宿主 CLI）。 */
  const handleUpdate = async (agent: MarketAgent) => {
    setBusyAction(`update:${agent.agentType}`);
    try {
      await invoke('agent_market_update', { agentType: agent.agentType });
      showToast(`${agent.displayName} 已升级`, 'success');
    } catch (err) {
      showToast(`升级失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    // 更新只影响该条目所在市场，按需刷新即可；
    // 同时刷新全局环境信息，否则「运行与策略」还显示更新前的版本。
    await fetchCategory(agent.category === 'cli' ? 'cli' : 'acp');
    void refreshEnv();
  };

  /** 「升级配置」：只同步市场清单里的配置，不安装、不升级宿主或适配器。 */
  const handleApplyConfig = async (agent: MarketAgent) => {
    setBusyAction(`config:${agent.agentType}`);
    try {
      await invoke('agent_market_apply_config', { agentType: agent.agentType });
      await fetchAgents();
      showToast(`${agent.displayName} 的配置已升级`, 'success');
    } catch (err) {
      showToast(`配置升级失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    await fetchCategory(agent.category === 'cli' ? 'cli' : 'acp');
    void refreshEnv();
  };

  /** 「重装」：版本未知时的唯一操作入口，按当前清单重新执行安装命令（不以「更新」名义执行）。 */
  const handleReinstall = async (agent: MarketAgent) => {
    setBusyAction(`reinstall:${agent.agentType}`);
    try {
      await invoke('agent_market_install', { agentType: agent.agentType });
      showToast(`${agent.displayName} 已重新安装`, 'success');
    } catch (err) {
      showToast(`重装失败: ${errorMessage(err)}`, 'error');
    }
    setBusyAction(null);
    await fetchCategory(agent.category === 'cli' ? 'cli' : 'acp');
    void refreshEnv();
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
    const acquiring = busyAction === `acquire:${agent.agentType}`;
    const updating = busyAction === `update:${agent.agentType}`;
    const configuring = busyAction === `config:${agent.agentType}`;
    const reinstalling = busyAction === `reinstall:${agent.agentType}`;
    // 未获取：只是把配置拉下来
    if (!agent.installed) {
      return (
        <SettingsButton
          variant="primary"
          icon={acquiring ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
          disabled={anyBusy}
          onClick={() => handleAcquire(agent)}
        >
          获取
        </SettingsButton>
      );
    }
    // 已获取但依赖未就绪：去配置页完成安装（pkgStatus 为空表示手工/历史配置，视为已就绪）
    if (agent.pkgStatus === 'pending_install' || agent.pkgStatus === 'failed') {
      return (
        <SettingsButton
          variant="primary"
          icon={<ArrowRight size={11} />}
          onClick={() => openInConfig(agent.agentType)}
          title="前往集成配置完成本机依赖安装"
        >
          {agent.pkgStatus === 'failed' ? '重试安装' : '完成配置'}
        </SettingsButton>
      );
    }
    // 已就绪：宿主/适配器升级、配置升级、重装相互独立，可能同时出现，避免互相顶掉
    const upgradeTarget = agent.category === 'acp' ? '适配器' : '宿主';
    const actions: ReactNode[] = [];
    if (agent.updateAvailable) {
      actions.push(
        <SettingsButton
          key="update"
          variant="update"
          icon={updating ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleUpdate(agent)}
          title={`将${upgradeTarget}升级到 v${agent.catalogVersion || agent.version}`}
        >
          {updating ? '升级中' : `升级${upgradeTarget}`}
        </SettingsButton>
      );
    } else if (agent.forceUpdateAvailable) {
      // 版本无法比较（非标准版本号 / 本机没有版本记录）：仍给一个明确标注的强制重装入口，
      // 否则这类条目会因"无法判定是否最新"而彻底失去更新手段。
      actions.push(
        <SettingsButton
          key="force"
          variant="secondary"
          icon={updating ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleUpdate(agent)}
          title={
            `无法比较版本（市场 v${agent.catalogVersion || agent.version}，本机 ${agent.installedVersion ? `v${agent.installedVersion}` : '未知'}）；` +
            '点击按市场规格强制升级（Agent 配置保留）'
          }
        >
          {updating ? '升级中' : '强制升级'}
        </SettingsButton>
      );
    }
    // 配置清单有新版：只更新本地配置快照，不重装本机 Agent
    if (agent.configUpdateAvailable) {
      actions.push(
        <SettingsButton
          key="config"
          variant="update"
          icon={configuring ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleApplyConfig(agent)}
          title={`升级本地配置清单到 ${agent.version}，不重装、不升级本机 Agent`}
        >
          {configuring ? '升级中' : '升级配置'}
        </SettingsButton>
      );
    }
    // 版本未知（无 registryVersion、也没有可用的最新版本探测）：不判断更新，只提供重装
    if (agent.versionSource === 'none') {
      actions.push(
        <SettingsButton
          key="reinstall"
          variant="secondary"
          icon={reinstalling ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={anyBusy}
          onClick={() => handleReinstall(agent)}
          title="无法获知最新版本；按当前市场清单重新安装（覆盖本机安装）"
        >
          {reinstalling ? '重装中' : '重装'}
        </SettingsButton>
      );
    }
    if (actions.length === 0) {
      return (
        <SettingsButton variant="secondary" onClick={() => openInConfig(agent.agentType)}>
          管理
        </SettingsButton>
      );
    }
    return <div className="flex items-center gap-1.5">{actions}</div>;
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
                        {/* 配置版本：ACP 条目取适配器版本，预置条目取清单版本 */}
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          v{agent.version || '—'}
                        </span>
                        {/* 独立适配器且目录版本与配置版本不一致时，单独标注适配器版本 */}
                        {!agent.adapterEmbedded &&
                          agent.registryVersion &&
                          agent.registryVersion !== agent.version && (
                            <span
                              className="text-[10px]"
                              style={{ color: 'var(--text-tertiary)' }}
                              title="ACP 适配器版本"
                            >
                              适配器 v{agent.registryVersion}
                            </span>
                          )}
                        {/* 宿主最新版本：预置条目与独立适配器条目展示，避免与配置版本混淆 */}
                        {agent.hostLatestVersion && (
                          <span
                            className="text-[10px]"
                            style={{ color: 'var(--text-tertiary)' }}
                            title="宿主最新版本"
                          >
                            宿主 v{agent.hostLatestVersion}
                          </span>
                        )}
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
                        {agent.updateAvailable && (
                          <span
                            className="text-[10px] px-1.5 py-0.5 rounded-full shrink-0"
                            style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                            title={
                              agent.installedVersion
                                ? `当前 v${agent.installedVersion}，市场 v${agent.catalogVersion || agent.version}`
                                : `市场 v${agent.catalogVersion || agent.version}`
                            }
                          >
                            可升级
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
