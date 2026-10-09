import { useCallback, useState } from 'react';
import { flushSync } from 'react-dom';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  Store, Workflow, Bot, Lightbulb, Search, X, RefreshCw, ArrowRight, Package,
} from 'lucide-react';
import { TitleBar, StatusBar } from '../layout';
import { useTerminal, type ViewMode } from '../../TerminalManager';
import { OnlinePluginStore } from '../plugin/OnlinePluginStore';
import { WorkflowTemplateMarket } from '../workflow/WorkflowTemplateMarket';
import { AgentConfigMarket } from '../env/AgentConfigMarket';
import { InspirationMarketTab } from './InspirationMarketTab';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/**
 * 资源市集（独立路由 /market）。
 *
 * 定位：把所有「从外部获取资源」的入口集中到一个页面，不再分散在设置 / 工作流 / 各面板里。
 * 4 个 tab 各管一类资源：
 *   插件         → 在线插件商店（安装/更新插件）
 *   工作流模板   → 在线模板市场（浏览/安装工作流模板）
 *   CLI Agent 配置 → 在线 Agent 市场（拉取/覆盖 Agent 配置）
 *   灵感         → 在线灵感市场（浏览提示词，一键抄进自己的灵感库）
 *
 * 每个 tab 都给出「我的在哪」出口：市集只负责"获取"，自己的数据仍在各自的原生地管理
 * （已装插件在设置›插件、工作流在工作流定义、Agent 在设置›Agent、灵感在灵感库）。
 *
 * 入口：顶栏右侧图标区（与指挥中心、通知中心同排）。
 */

type MarketTab = 'plugins' | 'workflow' | 'agents' | 'inspiration';

const TABS: { key: MarketTab; label: string; icon: typeof Store }[] = [
  // 插件用 Package（与设置›插件「已安装插件」标题同源），避免和页面标题的 Store 图标撞脸
  { key: 'plugins', label: '插件', icon: Package },
  { key: 'workflow', label: '工作流模板', icon: Workflow },
  { key: 'agents', label: 'CLI Agent 配置', icon: Bot },
  // 灵感用 Lightbulb（💡），不用 Sparkles —— 后者在工作流模板「精选」等处已密集使用
  { key: 'inspiration', label: '灵感', icon: Lightbulb },
];

/** 各 tab 对应的「我的」出口：市集是"获取"，管理仍在原生地 */
const MINE: Record<MarketTab, { label: string; where: string; to: string }> = {
  plugins: { label: '我的插件', where: '设置 › 插件', to: '/settings?tab=plugins' },
  workflow: { label: '我的工作流', where: '工作流定义', to: '/workflow' },
  agents: { label: '我的 Agent', where: '设置 › Agent', to: '/settings?tab=agents' },
  inspiration: { label: '我的灵感', where: '灵感库', to: '/inspirations' },
};

export function ResourceMarketPage() {
  const navigate = useNavigate();
  const { viewMode, setMode } = useTerminal();

  // 支持 /market?tab=xxx 深链（旧入口跳转过来时落到对应 tab）
  const [searchParams] = useSearchParams();
  const urlTab = searchParams.get('tab') as MarketTab | null;
  const isValidTab = (t: string | null): t is MarketTab => !!t && TABS.some((x) => x.key === t);
  const [activeTab, setActiveTab] = useState<MarketTab>(isValidTab(urlTab) ? urlTab : 'plugins');
  // URL 深链变化时对齐状态：用「渲染期修正」而非 effect（同 SettingsPage，避免级联渲染）
  const [syncedTab, setSyncedTab] = useState<string | null>(urlTab);
  if (urlTab !== syncedTab) {
    setSyncedTab(urlTab);
    if (isValidTab(urlTab)) setActiveTab(urlTab);
  }

  /**
   * 从资源市集切到某个模式（组合菜单点击）。
   *
   * 关键在顺序：必须**先把模式同步落地，再发导航**。
   * navigate 走的是 React Router 的 transition（低优先级）；若与 setMode（紧急更新）
   * 同批提交，个别运行环境下这次 transition 会一直挂着不提交 —— 表现为
   * "点了模式按钮页面没走，得再点一次别的入口才到"。
   * 用 flushSync 把模式更新先同步刷完，剩下的导航就是唯一的待提交工作，稳定落地。
   * （可直接对照 `/workflow` 的 ModeRedirect：它在 effect 里 setMode 后 navigate，天然是两步。）
   */
  const handleModeChange = (mode: ViewMode) => {
    flushSync(() => setMode(mode));
    navigate('/');
  };

  const mine = MINE[activeTab];

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        noActiveSegment
        marketOpen
        onOpenMarket={() => navigate('/')}
        onOpenSettings={() => navigate('/settings')}
        onOpenKnowledge={() => navigate('/knowledge')}
      />
      <div className="flex-1 min-h-0 px-2">
        <div className="h-full rounded-lg overflow-hidden flex flex-col" style={{ backgroundColor: 'var(--bg-content)' }}>

      {/* 页面标题 + tab 切换 + 「我的在哪」出口
          结构与「工作流定义」页的 tab 行保持一致：容器 pt-1，行高由 pd-tab（36px）决定，
          标题靠 items-center 垂直居中 —— 顶部留白与其它页统一，不贴住 TitleBar */}
      <div
        className="shrink-0 px-4 pt-1 flex items-center gap-3 overflow-x-clip"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2 shrink-0">
          <Store size={14} style={{ color: 'var(--accent)' }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>资源市集</span>
        </div>

        <div className="flex items-center gap-0.5 overflow-x-clip">
          {TABS.map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.key}
                onClick={() => setActiveTab(tab.key)}
                className={'pd-tab' + (activeTab === tab.key ? ' pd-tab-active' : '')}
              >
                <Icon size={12} />
                {tab.label}
              </button>
            );
          })}
        </div>

        <div className="flex-1" />

        {/* 我的在哪：市集只负责获取，自己的数据仍在原生地管理 */}
        <button
          onClick={() => navigate(mine.to)}
          className="pd-btn text-[11px] px-2 py-1 rounded flex items-center gap-1 shrink-0"
          style={{
            border: '1px solid var(--border)',
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
          }}
          title={`前往${mine.where}`}
        >
          {mine.label}
          <span style={{ color: 'var(--text-tertiary)' }}>· {mine.where}</span>
          <ArrowRight size={11} />
        </button>
      </div>

      {/* 内容区 */}
      <div className="flex-1 overflow-hidden">
        {activeTab === 'plugins' && <PluginMarketTab />}

        {activeTab === 'workflow' && (
          <WorkflowTemplateMarket
            // 安装结果（成功/失败）由市场组件自己 toast；父级只负责「装完去哪看」
            onUseTemplate={() => navigate('/workflow')}
          />
        )}

        {activeTab === 'agents' && <AgentConfigMarket />}

        {activeTab === 'inspiration' && <InspirationMarketTab />}
      </div>

        </div>
      </div>

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}

/**
 * 插件 tab：搜索 + 统计 + 刷新 的头部行，加上在线插件商店列表。
 *
 * 头部行原本在 PluginManager 的「商店态」里，随商店一起搬到市集。
 */
function PluginMarketTab() {
  const [query, setQuery] = useState('');
  const [stats, setStats] = useState<{ total: number; filtered: number } | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  /**
   * ⚠️ 必须用 useCallback 固定引用。
   *
   * OnlinePluginStore 把这个回调放进了 useEffect 的依赖数组。若每次渲染都传一个新的
   * 箭头函数，就会形成死循环：
   *   渲染 → effect 调用回调 → setStats(新对象) → 本组件重渲染 → 新回调 → effect 再触发 …
   * 页面因此持续重渲染，导航（React Router 的 transition，低优先级）会被无休止的紧急
   * 更新挤掉 —— 症状正是「插件 tab 里点任何跨路由入口都到不了，其它 tab 正常」。
   */
  const handleStatsChange = useCallback((total: number, filtered: number) => {
    // 值没变时返回原引用，让 React 直接 bail out，省掉这次多余渲染
    setStats((prev) => (prev && prev.total === total && prev.filtered === filtered
      ? prev
      : { total, filtered }));
  }, []);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      // 强制刷新索引缓存（OnlinePluginStore 挂载时读的就是这份缓存）
      await invoke('plugin_store_fetch_index', { forceRefresh: true });
    } catch (err) {
      showToast(`刷新插件索引失败：${errorMessage(err)}`, 'error');
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div
        className="shrink-0 px-4 py-2 flex items-center gap-2"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        <div className="relative flex-1 min-w-0 max-w-md">
          <Search
            size={13}
            style={{
              position: 'absolute',
              left: 10,
              top: '50%',
              transform: 'translateY(-50%)',
              color: 'var(--text-tertiary)',
              pointerEvents: 'none',
            }}
          />
          <input
            type="text"
            placeholder="搜索插件名称、描述、作者..."
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="search-input w-full"
            style={{ paddingLeft: 32, backgroundColor: 'var(--bg-secondary)' }}
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              className="pd-btn"
              style={{
                position: 'absolute',
                right: 8,
                top: '50%',
                transform: 'translateY(-50%)',
                color: 'var(--text-tertiary)',
                padding: 2,
              }}
            >
              <X size={12} />
            </button>
          )}
        </div>

        {stats && (
          <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            共{stats.filtered !== stats.total ? `${stats.filtered}/${stats.total}` : stats.total}个
          </span>
        )}

        <div className="flex-1" />

        <button
          onClick={handleRefresh}
          disabled={refreshing}
          className="pd-btn text-[10px] px-2 py-1 rounded flex items-center gap-1 shrink-0"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
        >
          <RefreshCw size={11} className={refreshing ? 'pd-animate-spin' : ''} />
          刷新
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 pd-scroll-stable">
        <OnlinePluginStore
          searchQuery={query}
          onStatsChange={handleStatsChange}
        />
      </div>
    </div>
  );
}
