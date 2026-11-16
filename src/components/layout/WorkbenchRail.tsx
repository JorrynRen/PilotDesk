import {
  Boxes,
  FolderKanban,
  Library,
  Lightbulb,
  MessageSquare,
  PanelsTopLeft,
  PanelLeftClose,
  PanelLeftOpen,
  Settings,
  Store,
  Terminal,
  Users,
  Workflow,
  type LucideIcon,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type FocusEvent } from 'react';
import type { ViewMode } from '../../TerminalManager';
import { useI18n } from '../../hooks/useI18n';
import { useWorkbenchPendingIndicators } from '../../hooks/useWorkbenchPendingIndicators';
import { groupCustomTabs, useCustomTabsStore } from '../../stores/customTabsStore';
import { TabIcon } from '../common/TabIcon';

interface WorkbenchRailProps {
  mode: ViewMode;
  activeAsset?: WorkbenchAssetKey;
  onModeChange: (mode: ViewMode) => void;
  onOpenKnowledge: () => void;
  onOpenEnclave: () => void;
  onOpenMarket: () => void;
  onOpenInspirations: () => void;
  onOpenPortal: () => void;
  onOpenProjects: () => void;
  onOpenSettings: () => void;
  sessionListOpen: boolean;
  onToggleSessionList: () => void;
}

export type WorkbenchAssetKey =
  | 'workflow'
  | 'knowledge'
  | 'enclave'
  | 'market'
  | 'inspiration'
  | 'portal'
  | 'projects'
  | 'settings';

const RAIL_COLLAPSED_KEY = 'pilotdesk.workbench.railCollapsed.v1';

function readRailCollapsed(): boolean {
  try {
    const stored = localStorage.getItem(RAIL_COLLAPSED_KEY);
    return stored === null ? true : stored === '1';
  } catch {
    return true;
  }
}

interface RailItem {
  key: string;
  label: string;
  title: string;
  icon: LucideIcon;
  active?: boolean;
  badgeCount?: number;
  onClick: () => void;
}

export function WorkbenchRail({
  mode,
  activeAsset,
  onModeChange,
  onOpenKnowledge,
  onOpenEnclave,
  onOpenMarket,
  onOpenInspirations,
  onOpenPortal,
  onOpenProjects,
  onOpenSettings,
  sessionListOpen,
  onToggleSessionList,
}: WorkbenchRailProps) {
  const { t } = useI18n();
  const customTabs = useCustomTabsStore((s) => s.tabs);
  const activeCustomTabId = useCustomTabsStore((s) => s.activeTabId);
  const mountedTabIds = useCustomTabsStore((s) => s.mountedTabIds);
  const setActiveCustomTab = useCustomTabsStore((s) => s.setActiveTab);
  const [collapsed, setCollapsed] = useState(readRailCollapsed);
  const {
    knowledge: knowledgePending,
    enclave: enclavePending,
    enclaveAssigned,
    enclaveReview,
  } = useWorkbenchPendingIndicators();
  const [portalFlyoutPosition, setPortalFlyoutPosition] = useState<{ top: number; left: number } | null>(null);
  const portalItemRef = useRef<HTMLDivElement>(null);
  const portalGroups = useMemo(() => groupCustomTabs(customTabs), [customTabs]);
  const mountedTabIdSet = useMemo(() => new Set(mountedTabIds), [mountedTabIds]);

  useEffect(() => {
    try {
      localStorage.setItem(RAIL_COLLAPSED_KEY, collapsed ? '1' : '0');
    } catch {
      /* localStorage may be unavailable in hardened webviews */
    }
  }, [collapsed]);

  const openPortalFlyout = useCallback(() => {
    const rect = portalItemRef.current?.getBoundingClientRect();
    if (!rect) return;
    setPortalFlyoutPosition({
      left: rect.right,
      top: Math.max(8, Math.min(rect.top, window.innerHeight - 320)),
    });
  }, []);

  const closePortalFlyout = useCallback(() => setPortalFlyoutPosition(null), []);

  const handlePortalBlur = useCallback((event: FocusEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
      closePortalFlyout();
    }
  }, [closePortalFlyout]);

  const openCustomTab = useCallback((id: string) => {
    closePortalFlyout();
    setActiveCustomTab(id);
    onModeChange('custom');
  }, [closePortalFlyout, onModeChange, setActiveCustomTab]);

  const workItems: RailItem[] = [
    {
      key: 'session',
      label: t('titleBar.session', '会话'),
      title: t('titleBar.session.title', '切换到会话模式'),
      icon: MessageSquare,
      active: !activeAsset && mode === 'session',
      onClick: () => onModeChange('session'),
    },
    {
      key: 'groupchat',
      label: t('titleBar.groupchat', '圆桌'),
      title: t('titleBar.groupchat.title', '多 Agent 圆桌协作'),
      icon: Users,
      active: !activeAsset && mode === 'groupchat',
      onClick: () => onModeChange('groupchat'),
    },
    {
      key: 'workflow',
      label: t('titleBar.workflow', '工作流'),
      title: t('titleBar.workflow.title', '工作流管理'),
      icon: Workflow,
      active: activeAsset ? activeAsset === 'workflow' : mode === 'workflow',
      onClick: () => onModeChange('workflow'),
    },
    {
      key: 'terminal',
      label: t('titleBar.terminal', '终端'),
      title: t('titleBar.terminal.title', '切换到终端模式'),
      icon: Terminal,
      active: !activeAsset && mode === 'terminal',
      onClick: () => onModeChange('terminal'),
    },
  ];

  const enclavePendingParts = [
    enclaveAssigned > 0 ? `${enclaveAssigned} 项待开始` : '',
    enclaveReview > 0 ? `${enclaveReview} 项待确认` : '',
  ].filter(Boolean);

  const assetItems: RailItem[] = [
    {
      key: 'knowledge',
      label: t('titleBar.knowledge', '知识库'),
      title: knowledgePending > 0
        ? t('titleBar.knowledge.title.pending', `个人知识库（${knowledgePending} 条待核实）`, {
            count: knowledgePending,
          })
        : t('titleBar.knowledge.title', '个人知识库'),
      icon: Library,
      active: activeAsset === 'knowledge',
      badgeCount: knowledgePending,
      onClick: onOpenKnowledge,
    },
    {
      key: 'enclave',
      label: t('titleBar.enclave', '飞地'),
      title: enclavePending > 0
        ? `团队内网协作空间（${enclavePendingParts.join('，')}）`
        : t('titleBar.enclave.title', '团队内网协作空间'),
      icon: Boxes,
      active: activeAsset === 'enclave',
      badgeCount: enclavePending,
      onClick: onOpenEnclave,
    },
    {
      key: 'market',
      label: t('titleBar.market', '资源市集'),
      title: t('titleBar.market.title', '浏览并获取插件、工作流模板、Agent 配置与灵感'),
      icon: Store,
      active: activeAsset === 'market',
      onClick: onOpenMarket,
    },
    {
      key: 'inspiration',
      label: t('titleBar.inspirations', '灵感库'),
      title: t('titleBar.inspirations.title', '管理本地保存的灵感与提示词'),
      icon: Lightbulb,
      active: activeAsset === 'inspiration',
      onClick: onOpenInspirations,
    },
    {
      key: 'portal',
      label: t('titleBar.portal', '门户'),
      title: t('titleBar.portal.title', '门户广场'),
      icon: PanelsTopLeft,
      active: activeAsset ? activeAsset === 'portal' : mode === 'custom',
      onClick: onOpenPortal,
    },
  ];

  const manageItems: RailItem[] = [
    {
      key: 'projects',
      label: t('workbench.projects', '项目'),
      title: t('workbench.projects.title', '工作归属与关联'),
      icon: FolderKanban,
      active: activeAsset === 'projects',
      onClick: onOpenProjects,
    },
  ];

  const renderItem = (item: RailItem) => {
    const Icon = item.icon;
    const badgeCount = item.badgeCount ?? 0;
    return (
      <button
        key={item.key}
        type="button"
        className="workbench-rail-item"
        data-active={item.active ? 'true' : undefined}
        aria-current={item.active ? 'page' : undefined}
        onClick={item.onClick}
        title={item.title}
        aria-label={badgeCount > 0 ? `${item.label}，${badgeCount} 项待处理` : item.label}
      >
        <Icon className="workbench-rail-icon" size={16} strokeWidth={1.9} />
        <span className="workbench-rail-label">{item.label}</span>
        {badgeCount > 0 && (
          <span className="workbench-rail-badge" aria-hidden="true">
            {badgeCount > 99 ? '99+' : badgeCount}
          </span>
        )}
      </button>
    );
  };

  const renderPortalItem = (item: RailItem) => {
    const Icon = item.icon;
    return (
      <div
        key={item.key}
        ref={portalItemRef}
        className="workbench-rail-portal"
        onMouseEnter={openPortalFlyout}
        onMouseLeave={closePortalFlyout}
        onFocusCapture={openPortalFlyout}
        onBlurCapture={handlePortalBlur}
      >
        <button
          type="button"
          className="workbench-rail-item"
          data-active={item.active ? 'true' : undefined}
          aria-current={item.active ? 'page' : undefined}
          aria-haspopup="menu"
          aria-expanded={portalFlyoutPosition !== null}
          onClick={() => {
            closePortalFlyout();
            item.onClick();
          }}
          title={item.title}
        >
          <Icon className="workbench-rail-icon" size={16} strokeWidth={1.9} />
          <span className="workbench-rail-label">{t('titleBar.portalSquare', '门户广场')}</span>
        </button>

        {portalFlyoutPosition && (
          <div
            className="workbench-portal-flyout"
            role="menu"
            aria-label={t('workbench.portalTabs', '门户标签')}
            style={{ top: portalFlyoutPosition.top, left: portalFlyoutPosition.left }}
          >
            <div className="workbench-portal-flyout-surface">
              <div className="workbench-portal-flyout-header">
                <span>{t('workbench.portalTabs', '门户标签')}</span>
                <span className="workbench-portal-flyout-count">{customTabs.length}</span>
              </div>

              {customTabs.length === 0 ? (
                <div className="workbench-portal-flyout-empty">
                  {t('workbench.portalTabs.empty', '暂无已配置标签')}
                </div>
              ) : (
                <div className="workbench-portal-flyout-groups">
                  {portalGroups.map((group) => (
                    <section key={group.group ?? '__ungrouped__'} className="workbench-portal-flyout-group">
                      <div className="workbench-portal-flyout-group-label">
                        {group.group ?? t('workbench.portalTabs.ungrouped', '未分组')}
                      </div>
                      {group.items.map((tab) => (
                        <button
                          key={tab.id}
                          type="button"
                          role="menuitem"
                          className="workbench-portal-flyout-item"
                          data-active={activeCustomTabId === tab.id ? 'true' : undefined}
                          onClick={() => openCustomTab(tab.id)}
                          title={tab.url}
                        >
                          <TabIcon icon={tab.icon} size={13} className="shrink-0" />
                          <span className="workbench-portal-flyout-item-label">{tab.label}</span>
                          {mountedTabIdSet.has(tab.id) && (
                            <span
                              className="workbench-portal-flyout-active-dot"
                              title={t('workbench.portalTabs.mounted', '已挂载')}
                              aria-label={t('workbench.portalTabs.mounted', '已挂载')}
                            />
                          )}
                        </button>
                      ))}
                    </section>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    );
  };

  return (
    <aside
      className="workbench-rail"
      data-collapsed={collapsed ? 'true' : 'false'}
      aria-label={t('workbench.navigation', '工作台导航')}
    >
      <div className="workbench-rail-scroll">
        <button
          type="button"
          className="workbench-rail-item workbench-rail-collapse"
          onClick={() => setCollapsed((value) => !value)}
          title={collapsed ? '展开左侧栏' : '收起左侧栏'}
          aria-label={collapsed ? '展开左侧栏' : '收起左侧栏'}
          aria-expanded={!collapsed}
        >
          {collapsed
            ? <PanelLeftOpen className="workbench-rail-icon" size={16} strokeWidth={1.9} />
            : <PanelLeftClose className="workbench-rail-icon" size={16} strokeWidth={1.9} />}
          <span className="workbench-rail-label">{collapsed ? '展开侧栏' : '收起侧栏'}</span>
        </button>

        {!activeAsset && mode === 'session' && (
          <button
            type="button"
            className="workbench-rail-item workbench-rail-session-toggle"
            data-active={sessionListOpen ? 'true' : undefined}
            aria-expanded={sessionListOpen}
            onClick={onToggleSessionList}
            title={sessionListOpen ? '收起会话列表' : '展开会话列表'}
          >
            {sessionListOpen
              ? <PanelLeftClose className="workbench-rail-icon" size={16} strokeWidth={1.9} />
              : <PanelLeftOpen className="workbench-rail-icon" size={16} strokeWidth={1.9} />}
            <span className="workbench-rail-label">会话列表</span>
          </button>
        )}

        <div className="workbench-rail-section">
          <div className="workbench-rail-section-label">{t('workbench.section.work', '工作区')}</div>
          <nav className="workbench-rail-nav" aria-label={t('workbench.section.work', '工作区')}>
            {workItems.map(renderItem)}
          </nav>
        </div>

        <div className="workbench-rail-section">
          <div className="workbench-rail-section-label">
            {t('workbench.section.assetsResources', '资产 / 资源')}
          </div>
          <nav
            className="workbench-rail-nav"
            aria-label={t('workbench.section.assetsResources', '资产 / 资源')}
          >
            {assetItems.map((item) => item.key === 'portal' ? renderPortalItem(item) : renderItem(item))}
          </nav>
        </div>

        <div className="workbench-rail-section">
          <div className="workbench-rail-section-label">
            {t('workbench.section.manage', '管理')}
          </div>
          <nav className="workbench-rail-nav" aria-label={t('workbench.section.manage', '管理')}>
            {manageItems.map(renderItem)}
          </nav>
        </div>
      </div>

      <div className="workbench-rail-footer">
        <button
          type="button"
          className="workbench-rail-item"
          data-active={activeAsset === 'settings' ? 'true' : undefined}
          aria-current={activeAsset === 'settings' ? 'page' : undefined}
          onClick={onOpenSettings}
          title={t('titleBar.settings', '设置')}
        >
          <Settings className="workbench-rail-icon" size={16} strokeWidth={1.9} />
          <span className="workbench-rail-label">{t('titleBar.settings', '设置')}</span>
        </button>
      </div>
    </aside>
  );
}
