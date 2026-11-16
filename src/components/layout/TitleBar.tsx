import { useState, useCallback, useEffect, useRef, type ReactNode } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Activity, PanelRightOpen, PanelRightClose, Minus, Square, X, Copy, ArrowLeft, Workflow, Terminal, MessageSquare, Users, Settings, Bell, Library, LayoutGrid, Boxes, ChevronRight } from 'lucide-react';
import type { ViewMode } from '../../TerminalManager';
import { useCustomTabsStore, PORTAL_TAB_ID } from '../../stores/customTabsStore';
import { useNotificationStore, countUnread } from '../../stores/notificationStore';
import { useCommandCenterStore } from '../../stores/commandCenterStore';
import { useI18n } from '../../hooks/useI18n';
import { useRuntimeSummary } from '../../hooks/useRuntimeSummary';
import { useWorkbenchPendingIndicators } from '../../hooks/useWorkbenchPendingIndicators';
import { AccountStatusEntry } from './AccountStatusEntry';

export type StatusHintState = 'loading' | 'ready' | 'error' | 'saving' | 'saved' | 'save-error' | 'idle';

export interface StatusHint {
  state: StatusHintState;
  /** 状态文本，留空则使用默认文本 */
  text?: string;
  /** idle状态时自动清除定时器（毫秒），默认常驻 */
  autoDismiss?: number;
}

interface TitleBarProps {
  /** 主界面 Workbench：模式与资产入口移到左侧工作台栏，顶栏只保留上下文和全局工具 */
  workbench?: boolean;
  /** 工作台面包屑的末级文本；独立资产页用它覆盖按 mode 推导出的上下文 */
  workbenchContext?: string;
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
  /**
   * 飞地（独立路由 /enclave）：与知识库同为一等段（点击即跳 /enclave）。
   *
   * 飞地是「团队业务数据的入口」——任务/产出/组织知识库都在内网主机侧，
   * 与知识库（个人知识资产）并列，故同样给一个常驻段而非收进右侧图标区。
   */
  onOpenEnclave?: () => void;
  /** 飞地页传 true：thumb 定位到「飞地」段 */
  enclaveOpen?: boolean;
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

export function TitleBar({ workbench, workbenchContext: workbenchContextOverride, onOpenSettings, onOpenWorkflow, onToggleRightPanel, rightPanelOpen, showBackButton, titleText, onBack, statusHint, onToggleTerminal, isTerminalOpen, mode, onModeChange, settingsOpen, onOpenKnowledge, knowledgeOpen, onOpenEnclave, enclaveOpen, noActiveSegment }: TitleBarProps) {
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;
  const { t } = useI18n();
  const activeCustomTabId = useCustomTabsStore((s) => s.activeTabId);
  const setActiveCustomTab = useCustomTabsStore((s) => s.setActiveTab);
  const openCommandCenter = useCommandCenterStore((s) => s.openCenter);
  const commandCenterOpen = useCommandCenterStore((s) => s.open);
  const { pendingCount, runningCount } = useRuntimeSummary();
  const [isMaximized, setIsMaximized] = useState(false);
  const [tauriReady, setTauriReady] = useState(true);
  // 通知中心：铃铛 + 未读徽标（跨全部模式常驻，见 NotificationCenter）
  const notificationItems = useNotificationStore((s) => s.items);
  const notificationOpen = useNotificationStore((s) => s.open);
  const setNotificationOpen = useNotificationStore((s) => s.setOpen);
  const unreadCount = countUnread(notificationItems);
  const runtimeTitle = pendingCount > 0
    ? t('titleBar.runtime.pending', `运行与待办（${pendingCount} 项待处理）`, { count: pendingCount })
    : runningCount > 0
      ? t('titleBar.runtime.running', `运行与待办（${runningCount} 项运行中）`, { count: runningCount })
      : t('titleBar.runtime', '运行与待办');

  const { knowledge: kbPending } = useWorkbenchPendingIndicators();

  // 一级导航段：只放「工作模式」（互斥切换，共用滑动 thumb）。
  // 知识库 / 飞地 / 门户是「跳转目的地」，与模式语义不同，单独渲染为二级资产入口；
  // 门户标签属于用户内容，用更轻的 chip 呈现；设置只保留右侧齿轮入口。
  const segments: { key: string; icon: ReactNode; label: string; title: string }[] = [
    { key: 'session', icon: <MessageSquare size={16} />, label: t('titleBar.session', '会话'), title: t('titleBar.session.title', '切换到会话模式') },
    { key: 'groupchat', icon: <Users size={16} />, label: t('titleBar.groupchat', '圆桌'), title: t('titleBar.groupchat.title', '多 Agent 圆桌协作') },
    { key: 'workflow', icon: <Workflow size={16} />, label: t('titleBar.workflow', '工作流'), title: t('titleBar.workflow.title', '工作流管理') },
    { key: 'terminal', icon: <Terminal size={16} />, label: t('titleBar.terminal', '终端'), title: t('titleBar.terminal.title', '切换到终端模式') },
  ];
  /**
   * 二级资产入口：独立路由，点击即跳转（不参与模式 thumb）。
   * 选中态用轻量底色表达，与一级模式的实心胶囊形成层级差。
   */
  const assetTabs: { key: string; icon: ReactNode; label: string; title: string; onClick: () => void }[] = [
    {
      key: 'knowledge',
      icon: <Library size={16} />,
      label: t('titleBar.knowledge', '知识库'),
      title: kbPending > 0
        ? t('titleBar.knowledge.title.pending', `个人知识库（${kbPending} 条待核实）`, { count: kbPending })
        : t('titleBar.knowledge.title', '个人知识库'),
      onClick: () => onOpenKnowledge?.(),
    },
    { key: 'enclave', icon: <Boxes size={16} />, label: t('titleBar.enclave', '飞地'), title: t('titleBar.enclave.title', '团队内网协作空间'), onClick: () => onOpenEnclave?.() },
    {
      key: 'portal',
      icon: <LayoutGrid size={16} />,
      label: t('titleBar.portal', '门户'),
      title: t('titleBar.portal.title', '门户广场'),
      onClick: () => {
        // 门户是 custom 模式下的特殊页：用哨兵 id 激活
        setActiveCustomTab(PORTAL_TAB_ID);
        onModeChange?.('custom');
      },
    },
  ];
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
      : enclaveOpen
        ? 'enclave'
        : settingsOpen
          ? 'settings'
          : (mode === 'custom'
            ? (activeCustomTabId === PORTAL_TAB_ID
              ? 'portal'
              : activeCustomTabId ? `custom:${activeCustomTabId}` : 'custom')
            : mode);
  const modeIndex = mode && activeSegmentKey ? segments.findIndex((s) => s.key === activeSegmentKey) : -1;
  const workbenchContext = workbenchContextOverride ?? (mode === 'custom'
    ? t('titleBar.portal', '门户')
    : segments.find((segment) => segment.key === mode)?.label);

  const handleModeKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const current = Math.max(0, modeIndex);
    const next = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? segments.length - 1
        : event.key === 'ArrowRight'
          ? (current + 1) % segments.length
          : (current - 1 + segments.length) % segments.length;
    onModeChange?.(segments[next].key as ViewMode);
    requestAnimationFrame(() => segRefs.current[next]?.focus());
  };

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
  }, [syncThumb]);

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
      className={`pd-titlebar relative z-[40] flex items-center gap-4 px-3 h-[52px] shrink-0 select-none ${workbench ? 'is-workbench' : ''}`}
      // 壳层风格：顶栏不再自成一条"带子"——去掉底色与下边框，直接画在壳层那片底上（见 App.tsx 工作区）
      // 不能加 overflow-hidden：账号弹层挂在顶栏内向下展开，会被裁剪成不可见；
      // 需要裁剪的导航区/门户标签各自已有 overflow-hidden。
      // z-[40] 只保证顶栏高于普通页面内容；z-50 及以上的弹窗遮罩必须完整覆盖顶栏。
      style={{ color: 'var(--text-primary)' }}
      onMouseDown={handleHeaderMouseDown}
      onMouseUp={handleHeaderMouseUp}
      onMouseMove={handleHeaderMouseMove}
      onMouseLeave={handleHeaderMouseLeave}
    >
      {/* 左：身份区 —— 品牌 +（子页面）返回与页面标题。固定宽度，不与导航争空间。 */}
      <div className="flex items-center gap-2 h-full min-w-0 shrink-0">
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

        {workbench && workbenchContext && (
          <>
            <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
            <span className="text-xs pointer-events-none shrink-0" style={{ color: 'var(--text-tertiary)' }}>
              {t('titleBar.workbench', '工作台')}
            </span>
            <ChevronRight
              aria-hidden="true"
              className="w-3 h-3 pointer-events-none shrink-0"
              strokeWidth={1.75}
              style={{ color: 'var(--text-tertiary)' }}
            />
            <span className="text-xs font-medium pointer-events-none shrink-0" style={{ color: 'var(--text-primary)' }}>
              {workbenchContext}
            </span>
          </>
        )}

        {/* 分隔符 + 返回按钮 + 页面标题（当 showBackButton=true 时显示） */}
        {showBackButton && (
          <>
            <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
            <button
              onClick={onBack || onOpenSettings}
              className="pd-btn flex items-center justify-center shrink-0 rounded-lg transition-colors text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]"
              style={{ width: 32, height: 32, padding: 0 }}
              title={t('titleBar.back', '返回')}
              aria-label={t('titleBar.back', '返回')}
            >
              <ArrowLeft size={18} />
            </button>
            <span className="text-[13px] font-medium pointer-events-none truncate" style={{ color: 'var(--text-primary)' }}>{titleText || t('titleBar.settings', '设置')}</span>
            {statusHint && statusHint.state !== 'idle' && (
              <StatusHintBadge hint={statusHint} />
            )}
          </>
        )}
      </div>

      {/* 中：导航区 —— 一级工作模式 + 二级资产入口 + 门户标签（用户内容，权重最低）。
          导航区左对齐并占据剩余宽度：一级 tab 因此有独立的视觉位置，不再与右侧工具抢层级。 */}
      <div className="flex items-center gap-3 h-full min-w-0 flex-1 overflow-hidden">
          {!showBackButton && !workbench && (mode && onModeChange) && (
            <div
              ref={modeGroupRef}
              className="relative flex items-center select-none shrink-0"
              role="radiogroup"
              aria-label={t('titleBar.modeGroup', '工作模式切换')}
              aria-orientation="horizontal"
              onKeyDown={handleModeKeyDown}
              style={{
                // 浅色主题的壳层底就是 --bg-tertiary，分段控件若也用它会和顶栏糊成一片，
                // 选中滑块看起来像"凭空浮着一块色"。这里改用 --bg-primary 并补一条淡描边，
                // 让控件本身有清晰边界（深色族下 primary 比 canvas 略亮，同样成立）。
                backgroundColor: 'var(--bg-primary)',
                // 用 inset 阴影画 1px 边：不参与布局，滑块的 offsetLeft/height 计算保持不变。
                boxShadow: 'inset 0 0 0 1px var(--border)',
                padding: 3,
                borderRadius: 10,
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
                    onClick={() => onModeChange(seg.key as ViewMode)}
                    role="radio"
                    aria-checked={isActive}
                    aria-label={seg.label}
                    tabIndex={isActive || (activeSegmentKey === null && index === 0) ? 0 : -1}
                    className={`relative z-10 flex items-center justify-center gap-2 rounded-md transition-colors ${
                      isActive
                        ? ''
                        : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'
                    }`}
                    style={{
                      // 一级 tab 要比页内按钮（28px）明显高一档：32px + 13px 字 + 16px 图标
                      padding: '6px 11px',
                      fontSize: 13,
                      lineHeight: '20px',
                      color: isActive ? 'var(--text-on-accent)' : undefined,
                      fontWeight: isActive ? 600 : 500,
                    }}
                    title={seg.title}
                  >
                    {seg.icon}
                    <span className="truncate max-w-[100px]">{seg.label}</span>
                  </button>
                );
              })}
              {/* 滑动 thumb：像素定位，跟随当前段实际位置 */}
              <div
                ref={thumbRef}
                aria-hidden
                data-role="mode-thumb"
                className="absolute top-[3px] rounded-md"
                style={{
                  height: 'calc(100% - 6px)',
                  width: 0,
                  left: 0,
                  opacity: 0,
                  backgroundColor: 'var(--accent)',
                  transition: 'left 180ms cubic-bezier(.22,.61,.36,1), width 180ms cubic-bezier(.22,.61,.36,1), opacity 120ms',
                  boxShadow: '0 1px 2px rgba(0,0,0,0.12)',
                  zIndex: 0,
                }}
              />
            </div>
          )}

        {/* T2 资产入口：独立路由的跳转目的地，不参与模式 thumb；选中态用轻量底色，弱于一级 tab */}
        {!showBackButton && !workbench && (mode && onModeChange) && (
          <>
            <div className="w-px h-4 shrink-0" style={{ backgroundColor: 'var(--border)' }} />
            <div className="flex items-center gap-0.5 h-full min-w-0 shrink-0">
              {assetTabs.map((asset) => {
                const isActive = activeSegmentKey === asset.key;
                return (
                  <button
                    key={asset.key}
                    onClick={asset.onClick}
                    aria-label={asset.label}
                    aria-current={isActive ? 'page' : undefined}
                    className={`relative flex items-center gap-1.5 rounded-lg transition-colors ${
                      isActive
                        ? 'bg-[var(--accent-light)] text-[var(--accent)]'
                        : 'text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]'
                    }`}
                    style={{
                      // 与一级 tab 同高（32px）：层级差由「分段控件的实心滑块 + 字重」表达，
                      // 尺寸上则一起高于页内按钮（28px），避免出现"顶栏里还有更小的按钮"。
                      padding: '7px 10px',
                      fontSize: 12,
                      lineHeight: '18px',
                      fontWeight: isActive ? 600 : 500,
                    }}
                    title={asset.title}
                  >
                    {asset.icon}
                    <span className="truncate max-w-[92px]">{asset.label}</span>
                    {/* 知识库待核实角标：绝对定位，不改变 tab 宽度 */}
                    {asset.key === 'knowledge' && kbPending > 0 && (
                      <span
                        className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                        style={{
                          top: -3,
                          right: -2,
                          minWidth: 13,
                          height: 13,
                          padding: '0 3px',
                          backgroundColor: 'var(--status-warning)',
                          color: 'var(--text-inverse)',
                          lineHeight: 1,
                        }}
                      >
                        {kbPending > 99 ? '99+' : kbPending}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>

          </>
        )}
          {/* 旧用法：工作流 CTA + 会话/终端 切换（mode 未启用时回退） */}
          {!showBackButton && !workbench && !(mode && onModeChange) && (onOpenWorkflow || onToggleTerminal) && (
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
                      color: 'var(--text-on-accent)',
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
                    role="radio"
                    aria-checked={!isTerminalOpen}
                    aria-label={t('titleBar.client', '客户端')}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: !isTerminalOpen ? 'var(--text-on-accent)' : 'var(--text-secondary)',
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
                    role="radio"
                    aria-checked={isTerminalOpen}
                    aria-label={t('titleBar.terminal', '终端')}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: isTerminalOpen ? 'var(--text-on-accent)' : 'var(--text-secondary)',
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

      {/* 右：工具动作（统一无边框图标）+ 窗口控制（独立形态，不与应用动作混排） */}
      <div className="flex items-center gap-1 h-full shrink-0">
          {onToggleRightPanel && (
            <button
              onClick={onToggleRightPanel}
              className={`flex items-center justify-center shrink-0 rounded-lg transition-colors ${
                rightPanelOpen
                  ? 'bg-[var(--accent-light)] text-[var(--accent)]'
                  : 'text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]'
              }`}
              style={{
                width: 32,
                height: 32,
                alignSelf: 'center',
                padding: 0,
                border: 'none',
                borderRadius: 8,
              }}
              title={rightPanelOpen ? t('titleBar.sidebar.close', '关闭侧边栏') : t('titleBar.sidebar.open', '打开侧边栏')}
              aria-label={rightPanelOpen ? t('titleBar.sidebar.close', '关闭侧边栏') : t('titleBar.sidebar.open', '打开侧边栏')}
              aria-pressed={rightPanelOpen}
            >
              <PanelIcon size={16} />
            </button>
          )}
          {/* 运行与待办：全局唯一入口。图标只承担状态提示，完整信息与操作在浮层内。 */}
          <button
            onClick={() => void openCommandCenter()}
            className={`relative flex items-center justify-center shrink-0 rounded-lg transition-colors ${
              commandCenterOpen
                ? 'bg-[var(--accent-light)] text-[var(--accent)]'
                : 'text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]'
            }`}
            style={{
              width: 32,
              height: 32,
              alignSelf: 'center',
              padding: 0,
              border: 'none',
              borderRadius: 8,
            }}
            title={runtimeTitle}
            aria-label={runtimeTitle}
            aria-expanded={commandCenterOpen}
            aria-haspopup="dialog"
          >
            <Activity size={16} />
            {pendingCount > 0 ? (
              <span
                className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                style={{
                  top: -3,
                  right: -3,
                  minWidth: 13,
                  height: 13,
                  padding: '0 3px',
                  backgroundColor: 'var(--status-warning)',
                  color: 'var(--text-inverse)',
                  lineHeight: 1,
                }}
              >
                {pendingCount > 99 ? '99+' : pendingCount}
              </span>
            ) : runningCount > 0 ? (
              <span
                aria-hidden
                className="absolute rounded-full"
                style={{
                  top: 5,
                  right: 5,
                  width: 6,
                  height: 6,
                  backgroundColor: 'var(--accent)',
                  boxShadow: '0 0 0 2px var(--bg-primary)',
                }}
              />
            ) : null}
          </button>
          {/* 设置：设置页的「当前位置」由左侧分组导航承担，这里只作为常驻工具入口（页面内在设置页时高亮）。 */}
          {onOpenSettings && !workbench && (
            <button
              onClick={onOpenSettings}
              className={`flex items-center justify-center shrink-0 rounded-lg transition-colors ${
                settingsOpen
                  ? 'bg-[var(--accent-light)] text-[var(--accent)]'
                  : 'text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]'
              }`}
              style={{
                width: 32,
                height: 32,
                alignSelf: 'center',
                padding: 0,
                border: 'none',
                borderRadius: 8,
              }}
              title={t('titleBar.settings', '设置')}
              aria-label={t('titleBar.settings', '设置')}
              aria-current={settingsOpen ? 'page' : undefined}
            >
              <Settings size={16} />
            </button>
          )}
          {/* 通知中心铃铛：固定尺寸并自居中 —— 不靠父级 stretch 决定高度，
              这样在"有模式开关"与"只剩铃铛"（编辑器页）的路由下渲染完全一致。 */}
          <button
            onClick={() => setNotificationOpen(!notificationOpen)}
            className={`relative flex items-center justify-center shrink-0 rounded-lg transition-colors ${
              notificationOpen
                ? 'bg-[var(--accent-light)] text-[var(--accent)]'
                : 'text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]'
            }`}
            style={{
              width: 32,
              height: 32,
              alignSelf: 'center',
              padding: 0,
              border: 'none',
              borderRadius: 8,
            }}
            title={unreadCount > 0
              ? t('titleBar.notifications.unread', `通知中心（${unreadCount} 条未读）`, { count: unreadCount })
              : t('titleBar.notifications', '通知中心')}
            aria-label={unreadCount > 0
              ? t('titleBar.notifications.unread', `通知中心（${unreadCount} 条未读）`, { count: unreadCount })
              : t('titleBar.notifications', '通知中心')}
            aria-expanded={notificationOpen}
            aria-haspopup="dialog"
          >
            <Bell size={16} />
            {unreadCount > 0 && (
              <span
                className="absolute flex items-center justify-center rounded-full text-[9px] font-medium"
                style={{
                  top: -3,
                  right: -3,
                  minWidth: 13,
                  height: 13,
                  padding: '0 3px',
                  backgroundColor: 'var(--status-danger)',
                  color: 'var(--text-inverse)',
                  lineHeight: 1,
                }}
              >
                {unreadCount > 99 ? '99+' : unreadCount}
              </span>
            )}
          </button>

          {/* 账号头像：顶栏高度（52px）足够呈现 24px 圆形头像，昵称/等级/权益收在弹层里 */}
          <AccountStatusEntry />

          {/* Separator：应用动作区 与 窗口控制区 分道 */}
          <div
            className="w-px h-4 mx-1 shrink-0"
            style={{ backgroundColor: 'var(--border)' }}
          />

        {/* Window controls：32×32 **正方形** + 8px 圆角 + 彼此 4px 间距。
            原先做的是 32×满高（48px）的竖长条、hover 时整条铺灰 —— 在 52px 高的顶栏里显得又长又重。
            颜色挂在按钮上（class 而非内联 style）：内联 color 会压掉 `hover:text-*`，
            曾导致"关闭"的 hover 只变了底色、图标仍是灰的。
            shrink-0：窗口控制永不被分段控件挤压。 */}
        {tauriReady && (
          <div className="flex items-center gap-1 shrink-0">
            <button
              onClick={handleMinimize}
              className="pd-btn w-8 h-8 rounded-md flex items-center justify-center transition-colors text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]"
              title={t('titleBar.window.minimize', '最小化')}
              aria-label={t('titleBar.window.minimize', '最小化')}
            >
              <Minus size={16} />
            </button>
            <button
              onClick={handleToggleMaximize}
              className="pd-btn w-8 h-8 rounded-md flex items-center justify-center transition-colors text-[var(--text-secondary)] hover:bg-[var(--bg-tertiary)] hover:text-[var(--text-primary)]"
              title={isMaximized ? t('titleBar.window.restore', '还原') : t('titleBar.window.maximize', '最大化')}
              aria-label={isMaximized ? t('titleBar.window.restore', '还原') : t('titleBar.window.maximize', '最大化')}
            >
              {isMaximized ? <Copy size={14} /> : <Square size={14} />}
            </button>
            <button
              onClick={handleClose}
              className="pd-btn w-8 h-8 rounded-md flex items-center justify-center transition-colors text-[var(--text-secondary)] hover:bg-[var(--status-danger)] hover:text-[var(--text-on-accent)]"
              title={t('titleBar.window.close', '关闭')}
              aria-label={t('titleBar.window.close', '关闭')}
            >
              <X size={16} />
            </button>
          </div>
        )}
      </div>
    </header>
  );
}
