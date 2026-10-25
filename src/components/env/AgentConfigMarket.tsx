import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Download, RefreshCw, Search, CheckCircle2, AlertCircle, Loader2, ArrowRight } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

import { SettingsSection, SettingsCard, SettingsButton } from '../settings';
import { AgentIcon } from '../common/AgentIcon';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';

/**
 * AgentConfigMarket — Agent 市集。
 *
 * 两类接入方式同架陈列：`ACP 协议接入`（官方目录 + 官方缺失的预置）与
 * `本地 CLI 接入`（兼容模式预置）。
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
  supported: boolean;
  unsupportedReason?: string | null;
  installed: boolean;
  installedVersion?: string | null;
  pkgStatus?: string | null;
  updateAvailable: boolean;
}

const CATEGORY_TABS: { key: MarketCategory; label: string; hint: string }[] = [
  { key: 'acp', label: 'ACP 协议接入', hint: '通过 Agent Client Protocol 适配器接入' },
  { key: 'cli', label: '本地 CLI 接入', hint: '以本地命令行子进程方式接入（兼容模式）' },
];

/** 包状态文案：市场草稿的安装进度对用户可见。 */
function statusLabel(a: MarketAgent): { text: string; tone: 'ok' | 'warn' | 'error' } | null {
  if (!a.installed) return null;
  if (a.pkgStatus === 'ready') return { text: '已就绪', tone: 'ok' };
  if (a.pkgStatus === 'failed') return { text: '安装失败', tone: 'error' };
  if (a.pkgStatus === 'pending_install') return { text: '待安装', tone: 'warn' };
  return { text: '已配置', tone: 'ok' };
}

export function AgentConfigMarket() {
  const navigate = useNavigate();
  /** 全局 Agent 列表是单例缓存，获取后必须主动刷新，否则配置页看不到新条目。 */
  const { fetchAgents } = useAgentRegistry();
  const [agents, setAgents] = useState<MarketAgent[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyType, setBusyType] = useState<string | null>(null);
  const [category, setCategory] = useState<MarketCategory>('acp');
  const [keyword, setKeyword] = useState('');

  const fetchMarket = useCallback(async (force = false) => {
    setLoading(true);
    try {
      const list = await invoke<MarketAgent[]>('agent_market_list', { force });
      setAgents(list);
    } catch (err) {
      showToast(`无法加载 Agent 市集 (${errorMessage(err)})`, 'warning');
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    // 不在 effect 体内同步 setState：推到微任务，避免级联渲染告警
    void Promise.resolve().then(() => fetchMarket(false));
  }, [fetchMarket]);

  const counts = useMemo(() => {
    const acp = agents.filter((a) => a.category === 'acp').length;
    const cli = agents.filter((a) => a.category === 'cli').length;
    return { acp, cli };
  }, [agents]);

  const visible = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return agents
      .filter((a) => a.category === category)
      .filter(
        (a) =>
          !kw ||
          a.displayName.toLowerCase().includes(kw) ||
          a.agentType.toLowerCase().includes(kw) ||
          a.description.toLowerCase().includes(kw) ||
          a.packages.some((p) => p.package.toLowerCase().includes(kw))
      );
  }, [agents, category, keyword]);

  /** 「获取」：落草稿后立刻跳去集成配置完成安装（ACP 需要装适配器，不能在这里静默完成）。 */
  const handleAcquire = async (agent: MarketAgent) => {
    setBusyType(agent.agentType);
    try {
      await invoke('agent_market_acquire', { agentType: agent.agentType });
      // 先刷新全局 Agent 列表：配置页的深层链接要靠它找到这条草稿。
      await fetchAgents();
      showToast(`已获取 ${agent.displayName}，请继续完成安装`, 'success');
      navigate(`/settings?tab=agents&agent=${encodeURIComponent(agent.agentType)}`);
    } catch (err) {
      showToast(`获取失败: ${errorMessage(err)}`, 'error');
    }
    setBusyType(null);
  };

  const openInConfig = (agentType: string) => {
    navigate(`/settings?tab=agents&agent=${encodeURIComponent(agentType)}`);
  };

  /** 「更新」：用市场里的新版本规格覆盖本地配置并重新安装（仅 ACP 官方目录条目可用）。 */
  const handleUpdate = async (agent: MarketAgent) => {
    setBusyType(agent.agentType);
    try {
      await invoke('agent_market_update', { agentType: agent.agentType });
      showToast(`${agent.displayName} 已更新`, 'success');
    } catch (err) {
      showToast(`更新失败: ${errorMessage(err)}`, 'error');
    }
    setBusyType(null);
    await fetchMarket(false);
  };

  const renderAction = (agent: MarketAgent) => {
    if (!agent.supported) {
      return (
        <SettingsButton variant="secondary" disabled title={agent.unsupportedReason || '暂不支持'}>
          暂不支持
        </SettingsButton>
      );
    }
    const busy = busyType === agent.agentType;
    // 未获取：只是把配置拉下来
    if (!agent.installed) {
      return (
        <SettingsButton
          variant="primary"
          icon={busy ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
          disabled={busy}
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
    // 已就绪且市场有更新版本：直接更新
    if (agent.updateAvailable) {
      return (
        <SettingsButton
          variant="primary"
          icon={busy ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
          disabled={busy}
          onClick={() => handleUpdate(agent)}
          title={`更新到 v${agent.version}`}
        >
          {busy ? '更新中' : '更新'}
        </SettingsButton>
      );
    }
    return (
      <SettingsButton variant="secondary" onClick={() => openInConfig(agent.agentType)}>
        管理
      </SettingsButton>
    );
  };

  return (
    <div className="h-full overflow-y-auto p-4 pd-scroll-stable">
      <SettingsSection
        title="Agent 市集"
        actions={
          <SettingsButton
            variant="secondary"
            icon={<RefreshCw size={11} className={loading ? 'animate-spin' : undefined} />}
            onClick={() => fetchMarket(true)}
            disabled={loading}
            title="刷新 ACP 官方目录（本地缓存 24 小时，过期自动后台更新）"
          >
            刷新目录
          </SettingsButton>
        }
      >
        {/* 分类 + 搜索 */}
        <div className="flex items-center gap-2 mb-3 flex-wrap">
          <div className="flex items-center gap-1">
            {CATEGORY_TABS.map((t) => (
              <button
                key={t.key}
                onClick={() => setCategory(t.key)}
                className={'pd-tab' + (category === t.key ? ' pd-tab-active' : '')}
                title={t.hint}
              >
                {t.label}
                <span className="ml-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {counts[t.key]}
                </span>
              </button>
            ))}
          </div>
          <div className="relative flex-1 min-w-[160px] max-w-md">
            <Search
              size={13}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none"
              style={{ color: 'var(--text-tertiary)' }}
            />
            <input
              type="text"
              className="block w-full pl-8 pr-3 rounded-md text-[11px] outline-none"
              style={{
                height: 28, /* 对齐同行 Tab 组高度 */
                backgroundColor: 'var(--bg-field)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
              placeholder="搜索名称 / 标识 / 包名"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
            />
          </div>
        </div>

        <div className="text-[10px] mb-2" style={{ color: 'var(--text-tertiary)' }}>
          {category === 'acp'
            ? '获取后会在本机安装对应适配器（npm 全局包）；安装完成前该 Agent 不可用于会话。'
            : '兼容模式配置通过命令行子进程调用，获取后需在本机安装对应的 CLI 工具。'}
        </div>

        <div className="space-y-2">
          {visible.length === 0 ? (
            <div className="text-xs py-2" style={{ color: 'var(--text-secondary)' }}>
              {loading ? '正在加载 Agent 市集...' : '没有匹配的条目'}
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
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          v{agent.version || '—'}
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
                        {agent.updateAvailable && (
                          <span
                            className="text-[10px] px-1.5 py-0.5 rounded-full shrink-0"
                            style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                            title={
                              agent.installedVersion
                                ? `当前 v${agent.installedVersion}，市场 v${agent.version}`
                                : `市场 v${agent.version}`
                            }
                          >
                            可更新
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
      </SettingsSection>
    </div>
  );
}
