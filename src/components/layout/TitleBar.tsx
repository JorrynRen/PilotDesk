import { useState, useCallback, useEffect, useMemo, useRef, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { PanelRightOpen, PanelRightClose, Minus, Square, X, Copy, ArrowLeft, Workflow, Terminal, MessageSquare, Users, Settings, Bell, LayoutDashboard, Library, Store, LayoutGrid } from 'lucide-react';
import type { ViewMode } from '../../TerminalManager';
import { useCustomTabsStore, PORTAL_TAB_ID } from '../../stores/customTabsStore';
import { useNotificationStore, countUnread } from '../../stores/notificationStore';
import { useCommandCenterStore } from '../../stores/commandCenterStore';
import { useI18n } from '../../hooks/useI18n';
import { Select, type SelectGroup } from '../common/Select';
import { TabIcon } from '../common/TabIcon';

/**
 * 顶栏门户标签的平铺渲染数量由用户配置（设置 → 门户标签「顶部显示个数」，0–3，默认 3）。
 * 超出部分收进「更多」下拉，避免标签变多时顶栏分段控件被无限撑宽（P0：多了就崩）。
 * 取值从 `useCustomTabsStore.titleBarLimit` 读取，见下方组件内。
 */

export type StatusHintState = 'loading' | 'ready' | 'error' | 'saving' | 'saved' | 'save-error' | 'idle';

export interface StatusHint {
  state: StatusHintState;
  /** 状态文本，留空则使用默认文本 */
  text?: string;
  /** idle状态时自动清除定时器（毫秒），默认常驻 */
  autoDismiss?: number;
}

interface TitleBarProps {
  onOpenSettings?: () => void;
  onOpenWorkflow?: () => void;
  onToggleRightPanel?: () => void;
  rightPanelOpen?: boolean;
  showBackButton?: boolean;
  titleText?: string;
  onBack?: () => void;
  /** 标题栏状态提示 */
  statusHint?: StatusHint | null;
  /** 虚拟控制台开关（兼容旧用法：仅切换会话/终端） */
  onToggleTerminal?: () => void;
  isTerminalOpen?: boolean;
  /** 四模式组合开关（工作流/会话/群聊/终端） */
  mode?: ViewMode;
  onModeChange?: (mode: ViewMode) => void;
  /** 设置在组合开关中作为独立段显示（设置页传 true，thumb 定位到该段） */
  settingsOpen?: boolean;
  /** 知识库一直是独立路由（设置段那种「进入后才出现」不适用），点击即跳 /knowledge */
  onOpenKnowledge?: () => void;
  /** 知识库页传 true：thumb 定位到「知识库」段 */
  knowledgeOpen?: boolean;
  /** 资源市集（独立路由 /market）：入口固定放在右侧图标区（与指挥中心/通知中心同排） */
  onOpenMarket?: () => void;
  /** 资源市集页传 true：图标高亮 */
  marketOpen?: boolean;
  /**
   * 该路由不对应任何模式段（如资源市集）：组合菜单照常可点击跳转，但不显示任何段的选中态。
   * 不传则按 mode/knowledgeOpen/settingsOpen 正常高亮。
   */
  noActiveSegment?: boolean;
}

/** 标题栏状态提示徽标组件 */
function StatusHintBadge({ hint }: { hint: StatusHint }) {
  const { t } = useI18n();
  const config: Record<StatusHintState, { icon: string; color: string; bg: string; defaultText: string }> = {
    loading:     { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: t('statusHint.loading', '加载中...') },
    ready:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: t('statusHint.ready', '已就绪') },
    error:       { icon: '✗', color: 'var(--status-danger)',  bg: 'var(--status-danger-bg)',        defaultText: t('statusHint.error', '加载失败') },
    saving:      { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: t('statusHint.saving', '保存中...') },
    saved:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: t('statusHint.saved', '已保存') },
    'save-error': { icon: '✗', color: 'var(--status-danger)', bg: 'var(--status-danger-bg)',        defaultText: t('statusHint.saveError', '保存失败') },
    idle:        { icon: '',   color: 'var(--text-tertiary)', bg: 'transparent',                    defaultText: '' },
  };

  const c = config[hint.state];
  const isLoading = hint.state === 'loading' || hint.state === 'saving';

  return (
    <span
      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-medium pointer-events-none ml-2"
      style={{
        color: c.color,
        background: c.bg,
        border: `1px solid ${c.color}33`,
        whiteSpace: 'nowrap',
      }}
    >
      {isLoading && (
        <span className="inline-block" style={{ animation: 'pd-spin 1s linear infinite' }}>◎</span>
      )}
      {!isLoading && c.icon}
      <span>{hint.text || c.defaultText}</span>
    </span>
  );
}

export function TitleBar({ onOpenSettings, onOpenWorkflow, onToggleRightPanel, rightPanelOpen, showBackButton, titleText, onBack, statusHint, onToggleTerminal, isTerminalOpen, mode, onModeChange, settingsOpen, onOpenKnowledge, knowledgeOpen, onOpenMarket, marketOpen, noActiveSegment }: TitleBarProps) {
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;
  const { t } = useI18n();
  const customTabs = useCustomTabsStore((s) => s.tabs);
  const activeCustomTabId = useCustomTabsStore((s) => s.activeTabId);
  const setActiveCustomTab = useCustomTabsStore((s) => s.setActiveTab);
  // 顶栏平铺显示个数（用户可配置，0–3）：0 时不渲染任何门户标签段，但「更多」下拉仍保留
  const titleBarLimit = useCustomTabsStore((s) => s.titleBarLimit);
  // 按 order 排序（防御性：store 已归一化，这里再排一次保证顶栏顺序稳定）
  const sortedCustomTabs = useMemo(
    () => [...customTabs].sort((a, b) => (a.order ?? 0) - (b.order ?? 0)),
    [customTabs],
  );
  const [isMaximized, setIsMaximized] = useState(false);
  const [tauriReady, setTauriReady] = useState(true);
  // 通知中心：铃铛 + 未读徽标（跨全部模式常驻，见 NotificationCenter）
  const notificationItems = useNotificationStore((s) => s.items);
  const notificationOpen = useNotificationStore((s) => s.open);
  const setNotificationOpen = useNotificationStore((s) => s.setOpen);
  const unreadCount = countUnread(notificationItems);
  // 指挥中心入口：打开即刷新全部数据源（进行中/待处理/成本速览）
  const centerOpen = useCommandCenterStore((s) => s.open);
  const closeCenter = useCommandCenterStore((s) => s.closeCenter);
  const openCenter = useCommandCenterStore((s) => s.openCenter);
  // 「入口在这里」指引：首次关闭指挥中心后，图标脉冲 + 气泡告诉用户下次在哪打开
  const entryHint = useCommandCenterStore((s) => s.entryHint);
  const dismissEntryHint = useCommandCenterStore((s) => s.dismissEntryHint);
  const centerBtnRef = useRef<HTMLButtonElement | null>(null);
  /** 气泡位置（fixed 坐标）：气泡脱离顶栏渲染，避免被顶栏高度/邻近层裁掉或盖住 */
  const [hintPos, setHintPos] = useState<{ top: number; right: number } | null>(null);

  useEffect(() => {
    // 气泡只在 entryHint 为真时渲染（见下方 `entryHint && hintPos` 守卫），
    // 所以关闭时不必把位置清空 —— 那是一次 effect 体内的同步 setState，
    // 会多一轮级联渲染，而对渲染结果没有任何影响。
    if (!entryHint) return;
    const el = centerBtnRef.current;
    if (el) {
      const r = el.getBoundingClientRect();
      setHintPos({ top: r.bottom + 8, right: Math.max(8, window.innerWidth - r.right) });
    }
    // 8 秒足够读完一句话；到点自动收起，不让指引变成常驻装饰
    const timer = setTimeout(() => dismissEntryHint(), 8000);
    return () => clearTimeout(timer);
  }, [entryHint, dismissEntryHint]);
  // 待处理徽标沿用通知中心的未决项（工具审批 / 工作流待人工输入），与铃铛口径一致
  const pendingCount = notificationItems.filter((i) => i.pending).length;

  /**
   * 知识库「待确认」角标：全部库的待核实候选总数。
   *
   * 为什么在这里单独拉而不复用 `knowledgeStore`：那个 store 是**懒加载**的（进知识库页才 loadBases），
   * 而角标要跨全部路由常驻。`kb_list_bases` 只做计数（子查询），开销很小，故轻量轮询即可。
   * 拿不到就不显示角标 —— 顶栏不该因为知识库模块出错而报错。
   */
  const [kbPending, setKbPending] = useState(0);
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try {
        const bases = await invoke<{ pendingCount: number }[]>('kb_list_bases');
        if (alive) setKbPending(bases.reduce((n, b) => n + (b.pendingCount ?? 0), 0));
      } catch {
        /* 忽略：保持上一次的值 */
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 30_000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  // 组合开关段：会话 / 群聊 / 工作流 / 知识库（独立路由，常驻）/ 终端 + 门户（固定）+ 门户标签（前 N 个参与 thumb 滑动）+ 设置段（仅设置页）
  const segments: { key: string; icon: ReactNode; label: string; title: string; tabId?: string }[] = [
    { key: 'session', icon: <MessageSquare size={11} />, label: t('titleBar.session', '会话'), title: t('titleBar.session.title', '切换到会话模式') },
    { key: 'groupchat', icon: <Users size={11} />, label: t('titleBar.groupchat', '群聊'), title: t('titleBar.groupchat.title', '多 Agent 群聊') },
    { key: 'workflow', icon: <Workflow size={11} />, label: t('titleBar.workflow', '工作流'), title: t('titleBar.workflow.title', '工作流管理') },
    {
      key: 'knowledge',
      icon: <Library size={11} />,
      label: t('titleBar.knowledge', '知识库'),
      title: kbPending > 0
        ? t('titleBar.knowledge.title.pending', `知识库（${kbPending} 条待核实）`, { count: kbPending })
        : t('titleBar.knowledge.title', '知识库：片段 / 文件知识 / 图谱'),
    },
    { key: 'terminal', icon: <Terminal size={11} />, label: t('titleBar.terminal', '终端'), title: t('titleBar.terminal.title', '切换到终端模式') },
    // 「门户」段：固定段（不受平铺个数影响），点击进入门户页（按分组查看全部标签）
    { key: 'portal', icon: <LayoutGrid size={11} />, label: t('titleBar.portal', '门户'), title: t('titleBar.portal.title', '门户：按分组查看全部标签') },
    // 只平铺前 N 个门户标签（N 来自用户配置，可为 0）；其余收进「更多」下拉（避免顶栏被撑爆）
    ...sortedCustomTabs.slice(0, titleBarLimit).map((t_) => ({
      key: `custom:${t_.id}`,
      tabId: t_.id,
      icon: <TabIcon icon={t_.icon} size={11} />,
      label: t_.label,
      title: t_.url,
    })),
    ...(settingsOpen
      ? [{ key: 'settings', icon: <Settings size={11} />, label: t('titleBar.settings', '设置'), title: t('titleBar.settings.title', '设置页面') }]
      : []),
  ];
  // 「更多」下拉：当门户标签总数 > 配置的平铺个数时渲染（0 个平铺 + 有标签时同样成立），
  // 列出全部门户标签（本项只用单组）。总数 ≤ N 时不渲染，保持既有行为。
  const hasMoreCustomTabs = sortedCustomTabs.length > titleBarLimit;
  const customTabGroups: SelectGroup[] = hasMoreCustomTabs
    ? [{
        label: t('titleBar.customTabs.group', '门户标签'),
        options: sortedCustomTabs.map((t_) => ({ value: t_.id, label: t_.label })),
      }]
    : [];
  // 点击「更多」中的某项：与顶栏段点击行为一致（切到该门户标签）
  const openCustomTab = useCallback((id: string) => {
    setActiveCustomTab(id);
    onModeChange?.('custom');
  }, [setActiveCustomTab, onModeChange]);
  /**
   * 当前激活段的 key。
   *
   * `noActiveSegment` = 该路由**不对应任何模式段**（如「资源市集」这类独立路由页）：
   * 组合菜单仍可点击跳转，但不该有任何一段显示为选中 —— 否则 thumb 会落到
   * `mode` 的残留值（通常是「会话」）上，像在说"你在会话模式"，是假的。
   */
  const activeSegmentKey = noActiveSegment
    ? null
    : knowledgeOpen
      ? 'knowledge'
      : settingsOpen
        ? 'settings'
        : (mode === 'custom'
          ? (activeCustomTabId === PORTAL_TAB_ID
            ? 'portal'
            : activeCustomTabId ? `custom:${activeCustomTabId}` : 'custom')
          : mode);
  const modeIndex = mode && activeSegmentKey ? segments.findIndex((s) => s.key === activeSegmentKey) : -1;

  // 组合开关 thumb 像素定位：按钮宽度自适应（左右内边距），按实际段位置滑动
  const segRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const thumbRef = useRef<HTMLDivElement>(null);
  const modeGroupRef = useRef<HTMLDivElement>(null);

  const syncThumb = useCallback(() => {
    const thumb = thumbRef.current;
    const el = modeIndex >= 0 ? segRefs.current[modeIndex] : null;
    if (thumb && el) {
      thumb.style.left = `${el.offsetLeft}px`;
      thumb.style.width = `${el.offsetWidth}px`;
      thumb.style.opacity = '1';
    } else if (thumb) {
      thumb.style.opacity = '0';
    }
  }, [modeIndex]);

  useEffect(() => {
    syncThumb();
    // titleBarLimit 变化会改变门户标签段的可见数量，进而移动当前段位置 → 需重算 thumb
  }, [syncThumb, customTabs, titleBarLimit]);

  /**
   * 段宽会随文案变化 —— 最典型的是**切换语言**（「会话」↔「Sessions」宽度差近一倍）。
   * 那一刻 modeIndex 与 customTabs 都没变，上面那个 effect 不会重跑，thumb 便停在旧位置，
   * 看起来就是"按钮背景框错位"。
   *
   * 所以直接盯容器的实际尺寸，凡是会引起重排的变化（切语言、字体加载完成、门户标签改名）
   * 都能自动跟上，而不用逐个去猜该把哪些值塞进依赖数组。
   */
  useEffect(() => {
    const group = modeGroupRef.current;
    if (!group || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(() => syncThumb());
    ro.observe(group);
    return () => ro.disconnect();
  }, [syncThumb]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const win = getCurrentWindow();
        setIsMaximized(await win.isMaximized());

        const fn = await win.onResized(async () => {
          try {
            const maximized = await win.isMaximized();
            setIsMaximized(maximized);
          } catch { /* window closed */ }
        });
        unlisten = fn;
      } catch (err) {
        console.warn('[TitleBar] Tauri window API not available:', err);
        setTauriReady(false);
      }
    })();

    return () => { unlisten?.(); };
  }, []);

  const handleMinimize = useCallback(async () => {
    try { await getCurrentWindow().minimize(); } catch { /* ignore */ }
  }, []);

  const handleToggleMaximize = useCallback(async () => {
    try { await getCurrentWindow().toggleMaximize(); } catch { /* ignore */ }
  }, []);

  const handleClose = useCallback(async () => {
    try { await getCurrentWindow().close(); } catch { /* ignore */ }
  }, []);

  // Drag + double-click: only startDragging on mousemove beyond threshold,
  // so rapid double-clicks never trigger drag (mouse doesn't move).
  const clickCountRef = useRef(0);
  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mouseDownPosRef = useRef<{ x: number; y: number } | null>(null);
  const draggingRef = useRef(false);

  const handleHeaderMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (
      target.closest('button') ||
      target.closest('a') ||
      target.closest('input') ||
      target.closest('select') ||
      target.tagName === 'BUTTON' ||
      target.tagName === 'INPUT' ||
      target.tagName === 'SELECT'
    ) {
      return;
    }
    e.preventDefault();
    mouseDownPosRef.current = { x: e.clientX, y: e.clientY };
    draggingRef.current = false;
  }, []);

  const handleHeaderMouseUp = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const pos = mouseDownPosRef.current;
    mouseDownPosRef.current = null;
    if (draggingRef.current) {
      draggingRef.current = false;
      return;
    }
    if (!pos) return;
    // Count as click only if mouse didn't move much (< 5px)
    const dx = Math.abs(e.clientX - pos.x);
    const dy = Math.abs(e.clientY - pos.y);
    if (dx < 5 && dy < 5) {
      clickCountRef.current += 1;
      if (clickCountRef.current >= 2) {
        clickCountRef.current = 0;
        if (clickTimerRef.current) {
          clearTimeout(clickTimerRef.current);
          clickTimerRef.current = null;
        }
        try { getCurrentWindow().toggleMaximize(); } catch { /* ignore */ }
        return;
      }
      if (clickTimerRef.current) clearTimeout(clickTimerRef.current);
      clickTimerRef.current = setTimeout(() => {
        clickCountRef.current = 0;
        clickTimerRef.current = null;
      }, 400);
    }
  }, []);

  const handleHeaderMouseMove = useCallback(() => {
    const pos = mouseDownPosRef.current;
    if (pos && !draggingRef.current) {
      // Start OS-level drag immediately to avoid cursor ghosting
      draggingRef.current = true;
      mouseDownPosRef.current = null;
      try { getCurrentWindow().startDragging(); } catch { /* ignore */ }
    }
  }, []);

  const handleHeaderMouseLeave = useCallback(() => {
    mouseDownPosRef.current = null;
    draggingRef.current = false;
  }, []);

  return (
    <header
      className="flex items-center justify-between px-3 h-12 shrink-0 select-none overflow-hidden"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
      onMouseDown={handleHeaderMouseDown}
      onMouseUp={handleHeaderMouseUp}
      onMouseMove={handleHeaderMouseMove}
      onMouseLeave={handleHeaderMouseLeave}
    >
      {/* Left: logo + app name (always) + optional back button + title */}
      <div className="flex items-center gap-2 h-full min-w-0">
        {/* 项目 Logo（所有页面统一显示） */}
        <img
          src="/logo.png"
          alt=""
          className="w-5 h-5 rounded pointer-events-none shrink-0"
          draggable={false}
          onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
        />
        {/* 项目名称（所有页面统一显示） */}
        <span className="text-xs font-medium pointer-events-none shrink-0">PilotDesk</span>

        {/* 分隔符 + 返回按钮 + 页面标题（当 showBackButton=true 时显示） */}
        {showBackButton && (
          <>
            <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
            <button
              onClick={onBack || onOpenSettings}
              className="pd-btn p-1 rounded transition-colors hover:opacity-80"
              style={{ color: 'var(--text-secondary)' }}
              title={t('titleBar.back', '返回')}
            >
              <ArrowLeft size={16} />
            </button>
            <span className="text-xs font-medium pointer-events-none truncate" style={{ color: 'var(--text-primary)' }}>{titleText || t('titleBar.settings', '设置')}</span>
            {statusHint && statusHint.state !== 'idle' && (
              <StatusHintBadge hint={statusHint} />
            )}
          </>
        )}
      </div>

      {/* Right: 动态分段滑动开关
         *  ── 布局 ──────────────────────────────────────
         * │[工作流] [会话] [群聊] [终端] [门户标签...]│
         * │  ←    紫色 thumb 在这里滑    →  │
         * └────────────────────────────┘
         *  固定 4 模式 + 门户标签（设置页「门户标签」tab 管理）动态并入，参与 thumb 滑动。
         *  外框、描边、圆角、内阴影完全统一，高度与两侧其它按钮严格对齐。
         *  兼容旧用法：未提供 mode/onModeChange 时回退到 工作流CTA+会话/终端切换。
         */}
      <div className="flex items-center h-full min-w-0">
        {/* ✦ 内层 stretch 包容器：组合开关 + 分隔符 + 折叠按钮 + 通知铃铛 的高度自动完全对齐，
            无需手动计算 border/padding 像素；外层 items-center 保证整组在 header 垂直居中。
            注：铃铛自带确定高度（见下），不参与自适应拉伸 —— 编辑器页等没有模式开关的路由，
            这一组只剩铃铛自己，若靠 stretch 撑高，同一颗钮子在不同路由就会不一样大。 */}
        <div className="flex items-stretch min-w-0">
          {/* 分段控件选区：min-w-0 + overflow-hidden 作溢出兜底 ——
              窗口极窄时才裁剪该区域，保证顶栏任何情况下都不横向溢出（右侧图标区不受影响）。 */}
          <div className="flex items-stretch min-w-0 overflow-hidden">
          {!showBackButton && (mode && onModeChange) && (
            <div
              ref={modeGroupRef}
              className="relative flex items-center select-none"
              role="radiogroup"
              aria-label={t('titleBar.modeGroup', '工作模式切换')}
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                border: '1px solid var(--border-strong, rgba(0,0,0,0.12))',
                padding: 2,
                borderRadius: 6,
                boxShadow: 'inset 0 1px 1px rgba(0,0,0,0.04)',
                minHeight: 22,
                width: 'fit-content',
              }}
            >
              {segments.map((seg, index) => {
                // 与 activeSegmentKey 单一来源比较：noActiveSegment 时它为 null，所有段一律不高亮
                const isActive = activeSegmentKey !== null && seg.key === activeSegmentKey;
                return (
                  <button
                    key={seg.key}
                    ref={(el) => { segRefs.current[index] = el; }}
                    onClick={() => {
                      if (seg.key === 'knowledge') {
                        onOpenKnowledge?.();
                        return;
                      }
                      if (seg.key === 'settings') {
                        onOpenSettings?.();
                        return;
                      }
                      if (seg.key === 'portal') {
                        // 门户是 custom 模式下的特殊页：用哨兵 id 激活
                        setActiveCustomTab(PORTAL_TAB_ID);
                        onModeChange('custom');
                        return;
                      }
                      if (seg.tabId) {
                        setActiveCustomTab(seg.tabId);
                        onModeChange('custom');
                      } else {
                        onModeChange(seg.key as ViewMode);
                      }
                    }}
                    className="relative z-10 flex items-center justify-center gap-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      paddingLeft: 10,
                      paddingRight: 10,
                      color: isActive ? '#fff' : 'var(--text-secondary)',
                      fontWeight: isActive ? 600 : 500,
                    }}
                    title={seg.title}
                  >
                    {seg.icon}
                    <span className="truncate max-w-[96px]">{seg.label}</span>
                    {/* 知识库待核实角标：绝对定位（不改变段宽 → 不影响 thumb 定位） */}
                    {seg.key === 'knowledge' && kbPending > 0 && (
                      <span
                        className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                        style={{
                          top: -4,
                          right: -2,
                          minWidth: 13,
                          height: 13,
                          padding: '0 3px',
                          backgroundColor: '#F59E0B',
                          color: '#fff',
                          lineHeight: 1,
                        }}
                      >
                        {kbPending > 99 ? '99+' : kbPending}
                      </span>
                    )}
                  </button>
                );
              })}
              {/* 滑动 thumb：像素定位，跟随当前段实际位置 */}
              <div
                ref={thumbRef}
                aria-hidden
                data-role="mode-thumb"
                className="absolute top-[2px] rounded-[4px]"
                style={{
                  height: 'calc(100% - 4px)',
                  width: 0,
                  left: 0,
                  opacity: 0,
                  backgroundColor: 'var(--accent)',
                  transition: 'left 180ms cubic-bezier(.22,.61,.36,1), width 180ms cubic-bezier(.22,.61,.36,1), opacity 120ms',
                  boxShadow:
                    '0 1px 2px rgba(0,0,0,0.12), 0 0 0 1px rgba(0,0,0,0.06), inset 0 1px 0 rgba(255,255,255,0.15)',
                  zIndex: 0,
                }}
              />
            </div>
          )}
          {/* 旧用法：工作流 CTA + 会话/终端 切换（mode 未启用时回退） */}
          {!showBackButton && !(mode && onModeChange) && (onOpenWorkflow || onToggleTerminal) && (
            <div
              className="relative flex items-center select-none"
              role="group"
              aria-label={t('titleBar.viewGroup', '工作流 / 视图模式')}
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                border: '1px solid var(--border-strong, rgba(0,0,0,0.12))',
                padding: 2,
                borderRadius: 6,
                boxShadow: 'inset 0 1px 1px rgba(0,0,0,0.04)',
                minHeight: 22,
                width: onOpenWorkflow && onToggleTerminal
                  ? 210
                  : onToggleTerminal
                    ? 130   // 只显示视图切换（退化模式：宽 130）
                    : 82,   // 只显示工作流（退化模式：宽 82）
              }}
            >
              {/* ─── 段 1：工作流 CTA（永久紫段，语义：跳转） ─── */}
              {onOpenWorkflow && (
                <div
                  className="relative flex items-center h-full shrink-0"
                  style={{
                    width: onToggleTerminal ? '38%' : '100%',
                  }}
                >
                  {/* 紫色 thumb 占满此段槽位 */}
                  <div
                    aria-hidden
                    data-role="workflow-thumb"
                    className="absolute inset-0 rounded-[4px]"
                    style={{
                      backgroundColor: 'var(--accent)',
                      boxShadow:
                        '0 1px 2px rgba(0,0,0,0.12), 0 0 0 1px rgba(0,0,0,0.06), inset 0 1px 0 rgba(255,255,255,0.15)',
                      zIndex: 0,
                      transition: 'background-color 160ms ease, transform 80ms ease',
                    }}
                  />
                  <button
                    onClick={onOpenWorkflow}
                    className="relative z-10 w-full flex items-center justify-center gap-1 rounded-[4px] text-[11px] transition-all active:scale-[.97]"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: '#fff',
                      fontWeight: 600,
                    }}
                    onMouseEnter={(e) => {
                      const thumb = (e.currentTarget.parentElement?.querySelector('[data-role="workflow-thumb"]') || null) as HTMLElement | null;
                      if (thumb) thumb.style.backgroundColor = 'color-mix(in srgb, var(--accent) 85%, #fff)';
                    }}
                    onMouseLeave={(e) => {
                      const thumb = (e.currentTarget.parentElement?.querySelector('[data-role="workflow-thumb"]') || null) as HTMLElement | null;
                      if (thumb) thumb.style.backgroundColor = 'var(--accent)';
                    }}
                    title={t('titleBar.workflow.title', '工作流管理')}
                  >
                    <Workflow size={12} />
                    {t('titleBar.workflow', '工作流')}
                  </button>
                </div>
              )}

              {/* 工作流 CTA 与状态切换区之间的间距（无可见竖线） */}
              {onOpenWorkflow && onToggleTerminal && (
                <div aria-hidden className="shrink-0 self-stretch" style={{ width: 3 }} />
              )}

              {/* ─── 段 2+3：客户端 / 终端 互斥切换（语义：状态） ─── */}
              {onToggleTerminal && (
                <div
                  className="relative flex items-center h-full flex-1 min-w-0"
                  role="radiogroup"
                  aria-label={t('titleBar.viewToggle', '视图模式切换')}
                >
                  {/* 段 2：客户端 */}
                  <button
                    onClick={() => { if (isTerminalOpen) onToggleTerminal(); }}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: !isTerminalOpen ? '#fff' : 'var(--text-secondary)',
                      fontWeight: !isTerminalOpen ? 600 : 500,
                    }}
                    title={t('titleBar.client.title', '切换到客户端模式')}
                  >
                    <MessageSquare size={11} />
                    {t('titleBar.client', '客户端')}
                  </button>
                  {/* 段 3：终端 */}
                  <button
                    onClick={() => { if (!isTerminalOpen) onToggleTerminal(); }}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: isTerminalOpen ? '#fff' : 'var(--text-secondary)',
                      fontWeight: isTerminalOpen ? 600 : 500,
                    }}
                    title={t('titleBar.terminal.title', '切换到终端模式')}
                  >
                    <Terminal size={11} />
                    {t('titleBar.terminal', '终端')}
                  </button>
                  {/* 滑动 thumb：只在右侧 62% 区域内移动 */}
                  <div
                    aria-hidden
                    data-role="mode-thumb"
                    className="absolute top-0 rounded-[4px]"
                    style={{
                      height: '100%',
                      width: 'calc(50% - 1px)',
                      backgroundColor: 'var(--accent)',
                      left: isTerminalOpen ? 'calc(50% + 1px)' : 0,
                      transition: 'left 180ms cubic-bezier(.22,.61,.36,1)',
                      boxShadow:
                        '0 1px 2px rgba(0,0,0,0.12), 0 0 0 1px rgba(0,0,0,0.06), inset 0 1px 0 rgba(255,255,255,0.15)',
                      zIndex: 0,
                    }}
                  />
                </div>
              )}
            </div>
          )}
          </div>
          {/* 「更多」下拉：门户标签数量 > N 时出现，复用统一 Select（含分组），列出全部门户标签。
              放在裁剪选区之外，极端窄屏下仍可见可点，作为被隐藏标签的兜底入口。
              与组合开关同显隐条件（工作流等带返回按钮的路由不显示）。 */}
          {hasMoreCustomTabs && !showBackButton && mode && onModeChange && (
            <Select
              value=""
              onChange={(id) => openCustomTab(id)}
              groups={customTabGroups}
              placeholder={t('titleBar.customTabs.more', '更多')}
              // 与组合开关段同高（28）：xs=24 比相邻的模式段矮 4px，看起来"凸不出来又凹进去"
              size="sm"
              className="shrink-0 ml-1"
              style={{ alignSelf: 'center' }}
              panelMinWidth={180}
              title={t('titleBar.customTabs.more.title', '更多门户标签')}
            />
          )}
          {/* 分组分隔符：功能导航 vs 布局操作（侧边栏折叠） — 设置按钮已移至 StatusBar 最左端 */}
          {onToggleRightPanel && (!showBackButton && ((mode && onModeChange) || onOpenWorkflow || onToggleTerminal)) && (
            <div className="w-px h-4 mx-1 shrink-0 self-center" style={{ backgroundColor: 'var(--border)' }} />
          )}
          {onToggleRightPanel && (
            <button
              onClick={onToggleRightPanel}
              className="flex items-center justify-center hover:opacity-80 transition-all shrink-0"
              style={{
                /* 与应用级动作按钮同尺寸（28×28 / 圆角 8）：此前一颗靠父级 stretch 撑高、
                   另两颗写死 26×30，三颗高度不齐且只隔 4px，看起来"贴成一坨" */
                width: 28,
                height: 28,
                alignSelf: 'center',
                padding: 0,
                border: '1px solid var(--border)',
                borderRadius: 8,
                color: rightPanelOpen ? 'var(--accent)' : 'var(--text-secondary)',
                background: rightPanelOpen ? 'var(--border)' : 'transparent',
              }}
              title={rightPanelOpen ? t('titleBar.sidebar.close', '关闭侧边栏') : t('titleBar.sidebar.open', '打开侧边栏')}
            >
              <PanelIcon size={13} />
            </button>
          )}
          {/* 资源市集：获取类入口（插件 / 工作流模板 / CLI Agent 配置 / 灵感）的聚合页，独立路由 /market */}
          {onOpenMarket && (
            <button
              onClick={onOpenMarket}
              className="relative flex items-center justify-center hover:opacity-80 transition-all shrink-0 ml-2"
              style={{
                width: 28,
                height: 28,
                alignSelf: 'center',
                padding: 0,
                border: '1px solid var(--border)',
                borderRadius: 8,
                color: marketOpen ? 'var(--accent)' : 'var(--text-secondary)',
                background: marketOpen ? 'var(--border)' : 'transparent',
              }}
              title={marketOpen
                ? t('titleBar.market.back', '资源市集：点击返回主界面')
                : t('titleBar.market', '资源市集：插件 / 工作流模板 / CLI Agent 配置 / 灵感')}
            >
              <Store size={13} />
            </button>
          )}
          {/* 指挥中心：进行中 / 待处理 / 成本速览 / 快捷入口 的聚合入口（置于铃铛之前） */}
          <button
            ref={centerBtnRef}
            onClick={() => { if (centerOpen) closeCenter(); else void openCenter(); }}
            className="relative flex items-center justify-center hover:opacity-80 transition-all shrink-0 ml-2"
            style={{
              width: 28,
              height: 28,
              alignSelf: 'center',
              padding: 0,
              border: '1px solid var(--border)',
              borderRadius: 8,
              color: centerOpen || entryHint ? 'var(--accent)' : 'var(--text-secondary)',
              background: centerOpen ? 'var(--border)' : 'transparent',
              // 指引期间图标本身也亮起来（与脉冲外圈一起，视线一眼落到入口）
              boxShadow: entryHint ? '0 0 0 1px var(--accent)' : undefined,
            }}
            title={pendingCount > 0
              ? t('titleBar.commandCenter.pending', `指挥中心（${pendingCount} 项待处理）`, { count: pendingCount })
              : t('titleBar.commandCenter', '指挥中心')}
          >
            {entryHint && (
              <span
                aria-hidden
                className="pd-animate-guide-ring absolute inset-0 rounded-[6px]"
                style={{ border: '2px solid var(--accent)', pointerEvents: 'none' }}
              />
            )}
            <LayoutDashboard size={13} />
            {pendingCount > 0 && (
              <span
                className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                style={{
                  top: -3,
                  right: -3,
                  minWidth: 13,
                  height: 13,
                  padding: '0 3px',
                  backgroundColor: '#F59E0B',
                  color: '#fff',
                  lineHeight: 1,
                }}
              >
                {pendingCount > 99 ? '99+' : pendingCount}
              </span>
            )}
          </button>
          {/* 设置：原先在底部状态栏最左，现按位置习惯移回顶部功能按钮区。
              样式与右侧其它图标按钮保持一致（28×28 / 1px 描边 / 圆角 8）。 */}
          {onOpenSettings && (
            <button
              onClick={onOpenSettings}
              className="flex items-center justify-center hover:opacity-80 transition-all shrink-0 ml-2"
              style={{
                width: 28,
                height: 28,
                alignSelf: 'center',
                padding: 0,
                border: '1px solid var(--border)',
                borderRadius: 8,
                color: 'var(--text-secondary)',
                background: 'transparent',
              }}
              title={t('titleBar.settings', '设置')}
            >
              <Settings size={13} />
            </button>
          )}
          {/* 通知中心铃铛：固定 26×30 并自居中 —— 不靠父级 stretch 决定高度，
              这样在"有模式开关"（组高 30.5）与"只剩铃铛"（编辑器页）的路由下渲染完全一致。 */}
          <button
            onClick={() => setNotificationOpen(!notificationOpen)}
            className="relative flex items-center justify-center hover:opacity-80 transition-all shrink-0 ml-2"
            style={{
              width: 28,
              height: 28,
              alignSelf: 'center',
              padding: 0,
              border: '1px solid var(--border)',
              borderRadius: 8,
              color: notificationOpen ? 'var(--accent)' : 'var(--text-secondary)',
              background: notificationOpen ? 'var(--border)' : 'transparent',
            }}
            title={unreadCount > 0
              ? t('titleBar.notifications.unread', `通知中心（${unreadCount} 条未读）`, { count: unreadCount })
              : t('titleBar.notifications', '通知中心')}
          >
            <Bell size={13} />
            {unreadCount > 0 && (
              <span
                className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                style={{
                  top: -3,
                  right: -3,
                  minWidth: 13,
                  height: 13,
                  padding: '0 3px',
                  backgroundColor: '#EF4444',
                  color: '#fff',
                  lineHeight: 1,
                }}
              >
                {unreadCount > 99 ? '99+' : unreadCount}
              </span>
            )}
          </button>
        </div>

        {/* Separator：应用动作区 与 窗口控制区 分道（窗口控制是无边框 32×满高，留白要够才不混成一排） */}
        <div
          className="w-px h-4 mx-2 shrink-0"
          style={{ backgroundColor: 'var(--border)' }}
        />

        {/* Window controls：shrink-0 —— 窗口控制永不被分段控件挤压 */}
        {tauriReady && (
          <>
            <button
              onClick={handleMinimize}
              className="pd-btn w-8 h-full shrink-0 flex items-center justify-center transition-colors hover:bg-black/5"
              title={t('titleBar.window.minimize', '最小化')}
            >
              <Minus size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
            <button
              onClick={handleToggleMaximize}
              className="pd-btn w-8 h-full shrink-0 flex items-center justify-center transition-colors hover:bg-black/5"
              title={isMaximized ? t('titleBar.window.restore', '还原') : t('titleBar.window.maximize', '最大化')}
            >
              {isMaximized ? (
                <Copy size={11} style={{ color: 'var(--text-secondary)' }} />
              ) : (
                <Square size={11} style={{ color: 'var(--text-secondary)' }} />
              )}
            </button>
            <button
              onClick={handleClose}
              className="pd-btn w-8 h-full shrink-0 flex items-center justify-center transition-colors hover:bg-red-500 hover:text-white"
              title={t('titleBar.window.close', '关闭')}
            >
              <X size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
          </>
        )}
      </div>

      {/* 「入口在这里」指引气泡：首次关闭指挥中心后出现（fixed 定位脱离顶栏，避免被裁/被盖），
          8 秒后或点「知道了」收起，用户再次打开面板也会立即收起 */}
      {entryHint && hintPos && (
        <div
          className="pd-animate-guide-pop fixed z-[99] rounded-lg px-3 py-2"
          style={{
            top: hintPos.top,
            right: hintPos.right,
            width: 208,
            backgroundColor: 'var(--bg-primary)',
            border: '1px solid var(--accent)',
            boxShadow: '0 8px 24px rgba(0,0,0,0.28)',
          }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {/* 指向图标的小箭头 */}
          <span
            aria-hidden
            className="absolute"
            style={{
              top: -5,
              right: 14,
              width: 8,
              height: 8,
              transform: 'rotate(45deg)',
              backgroundColor: 'var(--bg-primary)',
              borderLeft: '1px solid var(--accent)',
              borderTop: '1px solid var(--accent)',
            }}
          />
          <div className="text-[11px] font-medium" style={{ color: 'var(--accent)' }}>
            {t('titleBar.hint.title', '指挥中心的入口在这里')}
          </div>
          <div className="text-[10px] leading-relaxed mt-0.5" style={{ color: 'var(--text-secondary)' }}>
            {t('titleBar.hint.body', '下次要看「进行中 / 待处理 / 用量统计」，点这个图标就能再打开')}
          </div>
          <button
            onClick={dismissEntryHint}
            className="pd-btn mt-1.5 px-2 py-0.5 rounded text-[10px]"
            style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
          >
            {t('titleBar.hint.ok', '知道了')}
          </button>
        </div>
      )}
    </header>
  );
}