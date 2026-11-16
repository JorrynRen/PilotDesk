import { useCallback, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  Store, Workflow, Bot, Lightbulb, Search, X, RefreshCw, ArrowRight, Package,
} from 'lucide-react';
import { WorkbenchAssetPage } from '../layout';
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
 *   Agent 配置   → 在线 Agent 市场（拉取/覆盖 Agent 配置）
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
  { key: 'agents', label: 'Agent 配置', icon: Bot },
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

  // 支持 /market?tab=xxx 深链（旧入口跳转过来时落到对应 tab）
  const [searchParams] = useSearchParams();
  const urlTab = searchParams.get('tab') as MarketTab | null;
  const isValidTab = (t: string | null): t is MarketTab => !!t && TABS.some((x) => x.key === t);
  const [activeTab, setActiveTab] = useState<MarketTab>(isValidTab(urlTab) ? urlTab : 'plugins');
  /** 首次访问后再保持挂载：搜索、筛选与滚动位置不会因切换页签丢失。 */
  const [mountedTabs, setMountedTabs] = useState<Set<MarketTab>>(
    () => new Set<MarketTab>([isValidTab(urlTab) ? urlTab : 'plugins']),
  );
  // URL 深链变化时对齐状态：用「渲染期修正」而非 effect（同 SettingsPage，避免级联渲染）
  const [syncedTab, setSyncedTab] = useState<string | null>(urlTab);
  if (urlTab !== syncedTab) {
    setSyncedTab(urlTab);
    if (isValidTab(urlTab)) {
      setActiveTab(urlTab);
      setMountedTabs((prev) => (prev.has(urlTab) ? prev : new Set(prev).add(urlTab)));
    }
  }

  const activateTab = (tab: MarketTab) => {
    setActiveTab(tab);
    setMountedTabs((prev) => (prev.has(tab) ? prev : new Set(prev).add(tab)));
  };

  const mine = MINE[activeTab];

  return (
    <WorkbenchAssetPage contextLabel="资源市集" activeAsset="market">
      <div className="h-full flex flex-col overflow-hidden">

      {/* 页面标题 + tab 切换 + 「我的在哪」出口
          结构与「工作流定义」页的 tab 行保持一致：容器 pt-1，行高由 pd-tab（36px）决定，
          标题靠 items-center 垂直居中 —— 顶部留白与其它页统一，不贴住 TitleBar */}
      <div className="pd-page-head shrink-0">
        <div className="flex items-center gap-2 shrink-0">
          <Store size={14} style={{ color: 'var(--accent)' }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>资源市集</span>
        </div>

        <div className="pd-page-tabs">
          {TABS.map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.key}
                onClick={() => activateTab(tab.key)}
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
          className="pd-page-head-action pd-btn text-[11px] px-2 py-1 rounded flex items-center gap-1 shrink-0"
          style={{
            border: '1px solid var(--border)',
            backgroundColor: 'var(--bg-field)',
            color: 'var(--text-secondary)',
          }}
          title={`前往${mine.where}`}
        >
          {mine.label}
          <span className="pd-page-head-action-context" style={{ color: 'var(--text-tertiary)' }}>· {mine.where}</span>
          <ArrowRight size={11} />
        </button>
      </div>

      {/* 内容区 */}
      <div className="flex-1 overflow-hidden">
        {mountedTabs.has('plugins') && (
          <div className="h-full" hidden={activeTab !== 'plugins'}>
            <PluginMarketTab />
          </div>
        )}

        {mountedTabs.has('workflow') && (
          <div className="h-full" hidden={activeTab !== 'workflow'}>
            <WorkflowTemplateMarket
              // 安装结果（成功/失败）由市场组件自己 toast；父级只负责「装完去哪看」
              onUseTemplate={() => navigate('/workflow')}
            />
          </div>
        )}

        {mountedTabs.has('agents') && (
          <div className="h-full" hidden={activeTab !== 'agents'}>
            <AgentConfigMarket />
          </div>
        )}

        {mountedTabs.has('inspiration') && (
          <div className="h-full" hidden={activeTab !== 'inspiration'}>
            <InspirationMarketTab />
          </div>
        )}
      </div>

      </div>
    </WorkbenchAssetPage>
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
  const [refreshKey, setRefreshKey] = useState(0);

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
      // 索引更新完成后让子组件重新读取，否则当前页面仍展示刷新前的内容。
      setRefreshKey((value) => value + 1);
    } catch (err) {
      showToast(`刷新插件索引失败：${errorMessage(err)}`, 'error');
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="pd-market-toolbar">
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
            className="pd-market-search"
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
          className="pd-market-icon-button pd-btn"
          title="刷新插件索引"
          aria-label="刷新插件索引"
        >
          <RefreshCw size={11} className={refreshing ? 'pd-animate-spin' : ''} />
        </button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3 pd-scroll-stable">
        <OnlinePluginStore
          searchQuery={query}
          onStatsChange={handleStatsChange}
          refreshKey={refreshKey}
        />
      </div>
    </div>
  );
}
