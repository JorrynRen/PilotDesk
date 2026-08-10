import { useState, useCallback, useEffect, useRef } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { PanelRightOpen, PanelRightClose, Minus, Square, X, Copy, ArrowLeft, Workflow, Terminal, MessageSquare } from 'lucide-react';

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
  /** 虚拟控制台开关 */
  onToggleVirtualConsole?: () => void;
  isVirtualConsoleOpen?: boolean;
}

/** 标题栏状态提示徽标组件 */
function StatusHintBadge({ hint }: { hint: StatusHint }) {
  const config: Record<StatusHintState, { icon: string; color: string; bg: string; defaultText: string }> = {
    loading:     { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: '加载中...' },
    ready:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: '已就绪' },
    error:       { icon: '✗', color: 'var(--status-danger)',  bg: 'var(--status-danger-bg)',        defaultText: '加载失败' },
    saving:      { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: '保存中...' },
    saved:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: '已保存' },
    'save-error': { icon: '✗', color: 'var(--status-danger)', bg: 'var(--status-danger-bg)',        defaultText: '保存失败' },
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

export function TitleBar({ onOpenSettings, onOpenWorkflow, onToggleRightPanel, rightPanelOpen, showBackButton, titleText, onBack, statusHint, onToggleVirtualConsole, isVirtualConsoleOpen }: TitleBarProps) {
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;
  const [isMaximized, setIsMaximized] = useState(false);
  const [tauriReady, setTauriReady] = useState(true);

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

  const handleHeaderMouseMove = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
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
      className="flex items-center justify-between px-3 h-12 shrink-0 select-none"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
      onMouseDown={handleHeaderMouseDown}
      onMouseUp={handleHeaderMouseUp}
      onMouseMove={handleHeaderMouseMove}
      onMouseLeave={handleHeaderMouseLeave}
    >
      {/* Left: logo + app name (always) + optional back button + title */}
      <div className="flex items-center gap-2 h-full">
        {/* 项目 Logo（所有页面统一显示） */}
        <img
          src="/logo.png"
          alt=""
          className="w-5 h-5 rounded pointer-events-none"
          draggable={false}
          onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
        />
        {/* 项目名称（所有页面统一显示） */}
        <span className="text-xs font-medium pointer-events-none">PilotDesk</span>

        {/* 分隔符 + 返回按钮 + 页面标题（当 showBackButton=true 时显示） */}
        {showBackButton && (
          <>
            <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
            <button
              onClick={onBack || onOpenSettings}
              className="pd-btn p-1 rounded transition-colors hover:opacity-80"
              style={{ color: 'var(--text-secondary)' }}
              title="返回"
            >
              <ArrowLeft size={16} />
            </button>
            <span className="text-xs font-medium pointer-events-none" style={{ color: 'var(--text-primary)' }}>{titleText || '设置'}</span>
            {statusHint && statusHint.state !== 'idle' && (
              <StatusHintBadge hint={statusHint} />
            )}
          </>
        )}
      </div>

      {/* Right: 三段一体化滑轨控件
         *  ── 布局（总 ~210px） ──────────────────────────────────────────
         * │[🔧 工作流] ││[📝 客户端]  [⌨  终端]│
         * │  永久紫段   ││  ←     紫色 thumb 在这里滑   → │
         * └────────────┴┴─────────────────────────────────┘
         *              ↑ 加粗阻断线（语义区分：左=跳转CTA，右=状态切换）
         *  外框、描边、圆角、内阴影完全统一，高度与两侧其它按钮严格对齐。
         */}
      <div className="flex items-center h-full">
        {/* ✦ 内层 stretch 包容器：组合开关 + 分隔符 + 折叠按钮 三者高度自动完全对齐，
            无需手动计算 border/padding 像素；外层 items-center 保证整组在 header 垂直居中。 */}
        <div className="flex items-stretch">
          {!showBackButton && (onOpenWorkflow || onToggleVirtualConsole) && (
            <div
              className="relative flex items-center select-none"
              role="group"
              aria-label="工作流 / 视图模式"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                border: '1px solid var(--border-strong, rgba(0,0,0,0.12))',
                padding: 2,
                borderRadius: 6,
                boxShadow: 'inset 0 1px 1px rgba(0,0,0,0.04)',
                minHeight: 22,
                width: onOpenWorkflow && onToggleVirtualConsole
                  ? 210
                  : onToggleVirtualConsole
                    ? 130   // 只显示视图切换（退化模式：宽 130）
                    : 82,   // 只显示工作流（退化模式：宽 82）
              }}
            >
              {/* ─── 段 1：工作流 CTA（永久紫段，语义：跳转） ─── */}
              {onOpenWorkflow && (
                <div
                  className="relative flex items-center h-full shrink-0"
                  style={{
                    width: onToggleVirtualConsole ? '38%' : '100%',
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
                    title="工作流管理"
                  >
                    <Workflow size={12} />
                    工作流
                  </button>
                </div>
              )}

              {/* ─── 阻断线：区分左侧跳转 CTA / 右侧状态切换 ───
                   只有两个区都存在时才渲染 */}
              {onOpenWorkflow && onToggleVirtualConsole && (
                <div
                  aria-hidden
                  className="shrink-0 h-full flex items-center"
                  style={{ width: 3 }}
                >
                  <div
                    className="rounded-full"
                    style={{
                      width: 1,
                      height: 14,
                      backgroundColor: 'rgba(0,0,0,0.2)',
                      boxShadow: '0 0 0 1px rgba(255,255,255,0.08)',
                    }}
                  />
                </div>
              )}

              {/* ─── 段 2+3：客户端 / 终端 互斥切换（语义：状态） ─── */}
              {onToggleVirtualConsole && (
                <div
                  className="relative flex items-center h-full flex-1 min-w-0"
                  role="radiogroup"
                  aria-label="视图模式切换"
                >
                  {/* 段 2：客户端 */}
                  <button
                    onClick={() => { if (isVirtualConsoleOpen) onToggleVirtualConsole(); }}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: !isVirtualConsoleOpen ? '#fff' : 'var(--text-secondary)',
                      fontWeight: !isVirtualConsoleOpen ? 600 : 500,
                    }}
                    title="切换到客户端模式"
                  >
                    <MessageSquare size={11} />
                    客户端
                  </button>
                  {/* 段 3：终端 */}
                  <button
                    onClick={() => { if (!isVirtualConsoleOpen) onToggleVirtualConsole(); }}
                    className="relative z-10 flex items-center justify-center gap-1 flex-1 h-full rounded-[4px] text-[11px] transition-colors"
                    style={{
                      paddingTop: 4,
                      paddingBottom: 4,
                      color: isVirtualConsoleOpen ? '#fff' : 'var(--text-secondary)',
                      fontWeight: isVirtualConsoleOpen ? 600 : 500,
                    }}
                    title="切换到终端模式"
                  >
                    <Terminal size={11} />
                    终端
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
                      left: isVirtualConsoleOpen ? 'calc(50% + 1px)' : 0,
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
          {/* 分组分隔符：功能导航 vs 布局操作（侧边栏折叠） — 设置按钮已移至 StatusBar 最左端 */}
          {onToggleRightPanel && (!showBackButton && (onOpenWorkflow || onToggleVirtualConsole)) && (
            <div className="w-px h-full mx-1 shrink-0 flex items-center" style={{ backgroundColor: 'transparent' }}>
              <div className="w-px h-4 shrink-0" style={{ backgroundColor: 'var(--border)' }} />
            </div>
          )}
          {onToggleRightPanel && (
            <button
              onClick={onToggleRightPanel}
              className="flex items-center justify-center hover:opacity-80 transition-all shrink-0"
              style={{
                /* 在父级 flex items-stretch 容器中，此按钮自动撑满与组合开关相同的高度，
                   含 border + padding 全尺寸完全匹配，无需再手动写死 height 值。 */
                width: 26,
                padding: 0,
                border: '1px solid var(--border)',
                borderRadius: 6,
                color: rightPanelOpen ? 'var(--accent)' : 'var(--text-secondary)',
                background: rightPanelOpen ? 'var(--border)' : 'transparent',
              }}
              title={rightPanelOpen ? '关闭侧边栏' : '打开侧边栏'}
            >
              <PanelIcon size={13} />
            </button>
          )}
        </div>

        {/* Separator */}
        <div
          className="w-px h-4 mx-1.5"
          style={{ backgroundColor: 'var(--border)' }}
        />

        {/* Window controls */}
        {tauriReady && (
          <>
            <button
              onClick={handleMinimize}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
              title="最小化"
            >
              <Minus size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
            <button
              onClick={handleToggleMaximize}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
              title={isMaximized ? '还原' : '最大化'}
            >
              {isMaximized ? (
                <Copy size={11} style={{ color: 'var(--text-secondary)' }} />
              ) : (
                <Square size={11} style={{ color: 'var(--text-secondary)' }} />
              )}
            </button>
            <button
              onClick={handleClose}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-red-500 hover:text-white"
              title="关闭"
            >
              <X size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
          </>
        )}
      </div>
    </header>
  );
}