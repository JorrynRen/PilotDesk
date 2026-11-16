import React, { useRef, useEffect, useState, useCallback } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Plus, X, Monitor, ChevronDown, Palette, Terminal as TerminalIcon, Command, Keyboard, Sparkles } from 'lucide-react';
import '@xterm/xterm/css/xterm.css';
import { useTerminal, registerTerminalInjector } from '../TerminalManager';

// ── Preset Themes ──

const TERMINAL_THEMES = {
  'Catppuccin Dark': {
    background: '#1e1e2e',
    foreground: '#cdd6f4',
    cursor: '#f5e0dc',
    cursorAccent: '#1e1e2e',
    selectionBackground: '#585b7066',
    black: '#45475a', red: '#f38ba8', green: '#a6e3a1',
    yellow: '#f9e2af', blue: '#89b4fa', magenta: '#f5c2e7',
    cyan: '#94e2d5', white: '#bac2de',
    ui: {
      text: '#D9E0F7',
      muted: '#A6ADC8',
      subtle: '#9096AD',
      border: 'rgba(205,214,244,0.14)',
      surface: 'rgba(205,214,244,0.055)',
      surfaceStrong: 'rgba(205,214,244,0.11)',
      chrome: 'rgba(17,17,27,0.34)',
      hover: 'rgba(205,214,244,0.08)',
      active: 'rgba(137,180,250,0.16)',
      accent: '#89B4FA',
      accentText: '#11111B',
      accentSoft: 'rgba(137,180,250,0.17)',
      shadow: 'rgba(17,17,27,0.32)',
      success: '#A6E3A1',
      danger: '#F38BA8',
      warning: '#F9E2AF',
      info: '#89B4FA',
    },
  },
  'One Dark': {
    background: '#282c34',
    foreground: '#abb2bf',
    cursor: '#528bff',
    cursorAccent: '#282c34',
    selectionBackground: '#3e4451',
    black: '#282c34', red: '#e06c75', green: '#98c379',
    yellow: '#e5c07b', blue: '#61afef', magenta: '#c678dd',
    cyan: '#56b6c2', white: '#abb2bf',
    ui: {
      text: '#D7DAE0',
      muted: '#ABB2BF',
      subtle: '#8D949E',
      border: 'rgba(171,178,191,0.16)',
      surface: 'rgba(171,178,191,0.055)',
      surfaceStrong: 'rgba(171,178,191,0.11)',
      chrome: 'rgba(16,19,24,0.36)',
      hover: 'rgba(171,178,191,0.08)',
      active: 'rgba(97,175,239,0.16)',
      accent: '#61AFEF',
      accentText: '#11151A',
      accentSoft: 'rgba(97,175,239,0.17)',
      shadow: 'rgba(10,12,16,0.34)',
      success: '#98C379',
      danger: '#E06C75',
      warning: '#E5C07B',
      info: '#61AFEF',
    },
  },
  'Dracula': {
    background: '#282a36',
    foreground: '#f8f8f2',
    cursor: '#f8f8f2',
    cursorAccent: '#282a36',
    selectionBackground: '#44475a',
    black: '#21222c', red: '#ff5555', green: '#50fa7b',
    yellow: '#f1fa8c', blue: '#bd93f9', magenta: '#ff79c6',
    cyan: '#8be9fd', white: '#f8f8f2',
    ui: {
      text: '#F8F8F2',
      muted: '#C8C8D1',
      subtle: '#9697AD',
      border: 'rgba(248,248,242,0.15)',
      surface: 'rgba(248,248,242,0.055)',
      surfaceStrong: 'rgba(248,248,242,0.11)',
      chrome: 'rgba(17,18,25,0.34)',
      hover: 'rgba(248,248,242,0.08)',
      active: 'rgba(189,147,249,0.17)',
      accent: '#BD93F9',
      accentText: '#1D1E27',
      accentSoft: 'rgba(189,147,249,0.18)',
      shadow: 'rgba(15,16,22,0.34)',
      success: '#50FA7B',
      danger: '#FF5555',
      warning: '#F1FA8C',
      info: '#8BE9FD',
    },
  },
  'Solarized Dark': {
    background: '#002b36',
    foreground: '#839496',
    cursor: '#93a1a1',
    cursorAccent: '#002b36',
    selectionBackground: '#073642',
    black: '#073642', red: '#dc322f', green: '#859900',
    yellow: '#b58900', blue: '#268bd2', magenta: '#d33682',
    cyan: '#2aa198', white: '#eee8d5',
    ui: {
      text: '#EEE8D5',
      muted: '#93A1A1',
      subtle: '#7C969E',
      border: 'rgba(147,161,161,0.18)',
      surface: 'rgba(238,232,213,0.05)',
      surfaceStrong: 'rgba(238,232,213,0.1)',
      chrome: 'rgba(0,20,27,0.42)',
      hover: 'rgba(238,232,213,0.07)',
      active: 'rgba(42,161,152,0.18)',
      accent: '#2AA198',
      accentText: '#002B36',
      accentSoft: 'rgba(42,161,152,0.18)',
      shadow: 'rgba(0,20,27,0.36)',
      success: '#859900',
      danger: '#DC322F',
      warning: '#B58900',
      info: '#268BD2',
    },
  },
  'Light': {
    background: '#fafafa',
    foreground: '#383a42',
    cursor: '#526eff',
    cursorAccent: '#fafafa',
    selectionBackground: '#e0e0e066',
    black: '#383a42', red: '#e45649', green: '#50a14f',
    yellow: '#c18401', blue: '#4078f2', magenta: '#a626a4',
    cyan: '#0184bc', white: '#383a42',
    ui: {
      text: '#2F333B',
      muted: '#5F6673',
      subtle: '#6B7280',
      border: 'rgba(47,51,59,0.14)',
      surface: 'rgba(47,51,59,0.045)',
      surfaceStrong: 'rgba(47,51,59,0.085)',
      chrome: 'rgba(47,51,59,0.045)',
      hover: 'rgba(47,51,59,0.06)',
      active: 'rgba(64,120,242,0.12)',
      accent: '#4078F2',
      accentText: '#FFFFFF',
      accentSoft: 'rgba(64,120,242,0.12)',
      shadow: 'rgba(47,51,59,0.1)',
      success: '#2F7D32',
      danger: '#C62828',
      warning: '#9A6700',
      info: '#2F65A7',
    },
  },
  'Blue': {
    background: '#0a1628',
    foreground: '#c8d6e5',
    cursor: '#54a0ff',
    cursorAccent: '#0a1628',
    selectionBackground: '#54a0ff22',
    black: '#0a1628', red: '#ff6b6b', green: '#51cf66',
    yellow: '#ffd43b', blue: '#54a0ff', magenta: '#cc5de8',
    cyan: '#20c997', white: '#c8d6e5',
    ui: {
      text: '#D5E3F2',
      muted: '#9FB3C8',
      subtle: '#738BA4',
      border: 'rgba(200,214,229,0.14)',
      surface: 'rgba(200,214,229,0.055)',
      surfaceStrong: 'rgba(200,214,229,0.11)',
      chrome: 'rgba(4,13,26,0.44)',
      hover: 'rgba(200,214,229,0.08)',
      active: 'rgba(84,160,255,0.17)',
      accent: '#54A0FF',
      accentText: '#07121F',
      accentSoft: 'rgba(84,160,255,0.17)',
      shadow: 'rgba(2,8,18,0.4)',
      success: '#51CF66',
      danger: '#FF6B6B',
      warning: '#FFD43B',
      info: '#54A0FF',
    },
  },
};

const THEME_NAMES = Object.keys(TERMINAL_THEMES) as (keyof typeof TERMINAL_THEMES)[];
const utf8Encoder = new TextEncoder();

function terminalStyle(name: string): React.CSSProperties {
  const theme = TERMINAL_THEMES[name as keyof typeof TERMINAL_THEMES] ?? TERMINAL_THEMES['Catppuccin Dark'];
  return {
    '--terminal-bg': theme.background,
    '--terminal-text': theme.ui.text,
    '--terminal-muted': theme.ui.muted,
    '--terminal-subtle': theme.ui.subtle,
    '--terminal-border': theme.ui.border,
    '--terminal-surface': theme.ui.surface,
    '--terminal-surface-strong': theme.ui.surfaceStrong,
    '--terminal-chrome': theme.ui.chrome,
    '--terminal-hover': theme.ui.hover,
    '--terminal-active': theme.ui.active,
    '--terminal-accent': theme.ui.accent,
    '--terminal-accent-text': theme.ui.accentText,
    '--terminal-accent-soft': theme.ui.accentSoft,
    '--terminal-shadow': theme.ui.shadow,
    '--terminal-success': theme.ui.success,
    '--terminal-danger': theme.ui.danger,
    '--terminal-warning': theme.ui.warning,
    '--terminal-info': theme.ui.info,
    background: theme.background,
    color: theme.ui.text,
  } as React.CSSProperties;
}

const SHELL_OPTIONS = [
  { value: 'powershell', label: 'PowerShell' },
  { value: 'cmd', label: 'CMD' },
];

// ── Types ──

interface TerminalPanelProps {
  onClose?: () => void;
}

// ── Main Component ──
// The panel stays mounted so terminal sessions survive view switches.
// Global shortcuts still need to ignore input while another workbench mode is visible.

export const TerminalPanel: React.FC<TerminalPanelProps> = () => {
  const {
    viewMode,
    terminalTabs, setTerminalTabs,
    activeTerminalTabId, setActiveTerminalTabId,
    terminalShellType, setTerminalShellType,
    terminalTheme, setTerminalTheme,
  } = useTerminal();

  const [showShellMenu, setShowShellMenu] = useState(false);
  const [showThemeMenu, setShowThemeMenu] = useState(false);
  const [terminalDims, setTerminalDims] = useState<{ cols: number; rows: number }>({ cols: 0, rows: 0 });

  // ── Helpers ──

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return bytes + 'B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + 'KB';
    return (bytes / (1024 * 1024)).toFixed(1) + 'MB';
  }

  function formatUptime(seconds: number): string {
    if (seconds < 60) return seconds + 's';
    if (seconds < 3600) return Math.floor(seconds / 60) + 'm ' + (seconds % 60) + 's';
    return Math.floor(seconds / 3600) + 'h ' + Math.floor((seconds % 3600) / 60) + 'm';
  }

  // terminal instance map: tabId -> { term, fitAddon }
  const terminalRef = useRef<Map<string, { term: Terminal; fitAddon: FitAddon }>>(new Map());
  const containerRef = useRef<HTMLDivElement>(null);
  // Tauri event listener unlisten functions: tabId -> UnlistenFn[]
  const unlistenRefs = useRef<Map<string, UnlistenFn[]>>(new Map());
  // Guard against keyboard/button double-submit while a session is being created.
  const creatingRef = useRef(false);
  const mountedRef = useRef(true);
  // Monotonic sequence per mounted terminal panel, so closing an earlier tab
  // never makes a later tab reuse its display name.
  const tabSequenceRef = useRef(0);
  // Ref to scale wrapper div — tab divs must be appended here, not to containerRef
  const scaleWrapperRef = useRef<HTMLDivElement>(null);
  // Session metadata: creation time, exit time, and live status.
  const sessionMetaRef = useRef<Map<string, { createdAt: number; exitedAt?: number; alive: boolean }>>(new Map());
  // Byte counters for transfer tracking
  const bytesTrackerRef = useRef<Map<string, { rx: number; tx: number }>>(new Map());

  // ── 1s 轮询状态 ──
  const [displayUptime, setDisplayUptime] = useState(0);
  const [displayAlive, setDisplayAlive] = useState(true);
  // Per-second throughput: poll reads + resets counters, displays last-second values
  const [displaySecRx, setDisplaySecRx] = useState(0);
  const [displaySecTx, setDisplaySecTx] = useState(0);

  // ── Setup event listeners ──

  const setupTerminalListeners = useCallback((tabId: string, term: Terminal): Promise<void> => {
    const unlistens: UnlistenFn[] = [];
    // Register the container before awaiting listeners. If the tab is closed while
    // listen() is still resolving, the final ownership check below removes them.
    unlistenRefs.current.set(tabId, unlistens);

    const p1 = listen<string>(`terminal://output/${tabId}`, (event) => {
      const payload = event.payload;
      if (payload) {
        const tracker = bytesTrackerRef.current.get(tabId);
        if (tracker) {
          tracker.rx += utf8Encoder.encode(payload).byteLength;
        }
      }
      term.write(payload);
    }).then((fn) => {
      unlistens.push(fn);
    });

    const p2 = listen<{ session_id: string }>('terminal://exited', (event) => {
      if (event.payload?.session_id === tabId) {
        const meta = sessionMetaRef.current.get(tabId);
        if (meta) {
          meta.alive = false;
          meta.exitedAt ??= Date.now();
        }
        term.writeln('\r\n\x1b[90m[Process exited]\x1b[0m');
      }
    }).then((fn) => { unlistens.push(fn); });

    term.onData((data) => {
      const tracker = bytesTrackerRef.current.get(tabId);
      if (tracker) {
        tracker.tx += utf8Encoder.encode(data).byteLength;
      }
      invoke('terminal_write', {
        sessionId: tabId,
        data,
      }).catch((err) => {
        console.error('[Terminal] Write error:', err);
      });
    });

    return Promise.allSettled([p1, p2]).then((results) => {
      // 注册期间该标签页就被关掉的话，unlistens 已从 ref 摘除（数组对象本身还在），
      // 关标签那条路径遍历的是当时已 resolve 的项——这里补一次反注册，
      // 避免监听器跟着已关闭的终端残留。
      if (unlistenRefs.current.get(tabId) !== unlistens) {
        unlistens.forEach((fn) => fn());
      }
      const failed = results.find((result): result is PromiseRejectedResult => result.status === 'rejected');
      if (failed) {
        unlistens.forEach((fn) => fn());
        unlistenRefs.current.delete(tabId);
        throw failed.reason;
      }
    });
  }, []);

  // ── Create terminal ──

  const createTerminal = useCallback(async () => {
    if (creatingRef.current) return; // Already creating

    creatingRef.current = true;
    try {
    // 1. Create terminal in actual DOM, fit to measure exact cols, then create ConPTY.
    const scaleWrapper = scaleWrapperRef.current;
    if (!scaleWrapper) {
      console.error('[Terminal] Scale wrapper not ready');
      return;
    }
    const container = containerRef.current;
    if (!container) {
      console.error('[Terminal] Container not ready');
      return;
    }
    // Fit against the visible viewport. The ResizeObserver below keeps each active
    // terminal aligned when the window or either side panel changes width.
    const availableWidth = Math.max(320, container.clientWidth - 16);
    const availableHeight = Math.max(180, container.clientHeight - 24);

    // Create tab div first (placeholder id, will be updated after ConPTY creation)
    const tabDiv = document.createElement('div');
    tabDiv.style.width = '100%';
    tabDiv.style.height = `${availableHeight}px`;
    scaleWrapper.querySelectorAll('[id^="xterm-tab-"]').forEach((el) => {
      (el as HTMLElement).style.display = 'none';
    });
    tabDiv.style.display = 'block';
    scaleWrapper.style.width = `${availableWidth}px`;
    scaleWrapper.style.height = `${availableHeight}px`;
    scaleWrapper.appendChild(tabDiv);

    // FitAddon measures tabDiv, which now matches the visible terminal viewport.
    const term = new Terminal({
      theme: TERMINAL_THEMES[terminalTheme as keyof typeof TERMINAL_THEMES],
      fontFamily: 'Cascadia Code, Consolas, "Courier New", monospace',
      fontSize: 14, lineHeight: 1.2,
      cols: 80, rows: 24,
      cursorBlink: true, cursorStyle: 'bar',
      scrollback: 50000, allowProposedApi: true, convertEol: false,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(tabDiv);

    // Fit to actual container (rAF ensures layout complete)
    await new Promise(r => requestAnimationFrame(r));
    fitAddon.fit();
    const dims = { cols: term.cols, rows: term.rows };

    // 2. Create backend ConPTY session at the fitted size
    let sessionId: string;
    let shellType: string;
    let pid: number | undefined;
    try {
      const result = await invoke<{ session_id: string; shell_type: string; pid?: number }>('terminal_create', {
        shellType: terminalShellType,
        cols: dims.cols,
        rows: dims.rows,
      });
      sessionId = result.session_id;
      shellType = result.shell_type;
      pid = result.pid;
    } catch (err) {
      console.error('[Terminal] Failed to create session:', err);
      // 创建失败时清理刚挂载的 xterm DOM，避免空输入块残留覆盖后续欢迎页按钮
      try {
        term.dispose();
      } catch { /* ignore */ }
      tabDiv?.parentNode?.removeChild(tabDiv);
      return;
    }

    // 3. Update tabDiv id with real sessionId and store in refs
    tabDiv.id = `xterm-tab-${sessionId}`;
    terminalRef.current.set(sessionId, { term, fitAddon });

    // Initialize session metadata and byte tracking
    sessionMetaRef.current.set(sessionId, { createdAt: Date.now(), alive: true });
    bytesTrackerRef.current.set(sessionId, { rx: 0, tx: 0 });
    const abandonSession = () => {
      const unlistens = unlistenRefs.current.get(sessionId) ?? [];
      unlistens.forEach((fn) => fn());
      unlistenRefs.current.delete(sessionId);
      sessionMetaRef.current.delete(sessionId);
      bytesTrackerRef.current.delete(sessionId);
      terminalRef.current.delete(sessionId);
      try {
        term.dispose();
      } catch { /* ignore */ }
      tabDiv.remove();
      void invoke('terminal_close', { sessionId }).catch(() => {});
    };

    if (!mountedRef.current) {
      abandonSession();
      return;
    }

    // 5. Register listeners before attaching the backend read loop, then expose
    // the tab only after attach succeeds. This prevents a visible tab from being
    // closed while its listener/session setup is still in flight.
    try {
      await setupTerminalListeners(sessionId, term);
    } catch (err) {
      console.error('[Terminal] Listener setup failed:', err);
      abandonSession();
      return;
    }
    if (!mountedRef.current) {
      abandonSession();
      return;
    }

    try {
      await invoke('terminal_attach', { sessionId });
    } catch (err) {
      console.error('[Terminal] Attach failed:', err);
      abandonSession();
      return;
    }
    if (!mountedRef.current) {
      abandonSession();
      return;
    }

    // 6. Add tab to state
    tabSequenceRef.current += 1;
    const shellLabel = shellType === 'powershell' || shellType === 'pwsh' ? 'PowerShell' : 'CMD';
    const tab: import('../TerminalManager').TerminalTabData = {
      id: sessionId,
      shellType,
      title: `${shellLabel} ${tabSequenceRef.current}`,
      pid,
    };
    setTerminalTabs((prev) => [...prev, tab]);
    setActiveTerminalTabId(sessionId);
    setTerminalDims({ cols: dims.cols, rows: dims.rows });
    term.focus();

    } finally {
      creatingRef.current = false;
    }
  }, [terminalShellType, terminalTheme, setupTerminalListeners, setTerminalTabs, setActiveTerminalTabId]);

  // ── Close terminal tab ──
  //    Uses functional state updates to avoid stale closure issues with activeTerminalTabId.

  const closeTerminal = useCallback((tabId: string, event?: React.MouseEvent) => {
    if (event) event.stopPropagation();

    const entry = terminalRef.current.get(tabId);
    if (entry) {
      entry.term.dispose();
      terminalRef.current.delete(tabId);
    }

    const unlistens = unlistenRefs.current.get(tabId);
    if (unlistens) {
      unlistens.forEach((fn) => fn());
      unlistenRefs.current.delete(tabId);
    }

    void invoke('terminal_close', { sessionId: tabId }).catch((err) => {
      console.error('[Terminal] Failed to close session:', err);
    });

    // Clean up tracking refs
    sessionMetaRef.current.delete(tabId);
    bytesTrackerRef.current.delete(tabId);

    const tabDiv = document.getElementById(`xterm-tab-${tabId}`);
    if (tabDiv?.parentNode) {
      tabDiv.parentNode.removeChild(tabDiv);
    }

    // 最后一个终端关闭后重置尺寸，避免透明容器覆盖默认引导页。
    const wrapper = scaleWrapperRef.current;
    if (wrapper && wrapper.querySelectorAll('[id^="xterm-tab-"]').length === 0) {
      wrapper.style.width = '';
      wrapper.style.height = '';
      wrapper.style.marginTop = '';
    }

    setTerminalTabs((prev) => prev.filter((t) => t.id !== tabId));
  }, [setTerminalTabs]);

  // ── Tab switch: show the selected xterm, then let the resize effect fit it ──

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !activeTerminalTabId) return;

    container.querySelectorAll('[id^="xterm-tab-"]').forEach((el) => {
      (el as HTMLElement).style.display = 'none';
    });
    const activeDiv = document.getElementById(`xterm-tab-${activeTerminalTabId}`);
    if (activeDiv) activeDiv.style.display = 'block';

    const entry = terminalRef.current.get(activeTerminalTabId);
    if (entry) {
      entry.term.focus();
      setTerminalDims({ cols: entry.term.cols, rows: entry.term.rows });
    }
  }, [activeTerminalTabId]);

  // Keep the selection valid when the active tab disappears. This is kept
  // outside the terminalTabs updater so the updater stays pure.
  useEffect(() => {
    if (
      activeTerminalTabId
      && terminalTabs.some((tab) => tab.id === activeTerminalTabId)
    ) {
      return;
    }
    setActiveTerminalTabId(terminalTabs[terminalTabs.length - 1]?.id ?? null);
  }, [activeTerminalTabId, setActiveTerminalTabId, terminalTabs]);

  // ── 注入桥注册：把「向当前活跃终端输入点注入文本」能力暴露给外部组件（如灵感面板） ──
  useEffect(() => {
    const entry = activeTerminalTabId ? terminalRef.current.get(activeTerminalTabId) : undefined;
    if (entry) {
      const term = entry.term;
      registerTerminalInjector((text: string) => {
        term.paste(text);
        return true;
      });
    } else {
      registerTerminalInjector(null);
    }
    // terminalTabs 参与依赖：创建/关闭 tab 后以最新 terminalRef 重算活跃注入目标
  }, [activeTerminalTabId, terminalTabs]);

  // ── Resize the active terminal when the available panel area changes ──
  useEffect(() => {
    if (viewMode !== 'terminal' || !activeTerminalTabId) return;

    const container = containerRef.current;
    const wrapper = scaleWrapperRef.current;
    const entry = terminalRef.current.get(activeTerminalTabId);
    if (!container || !wrapper || !entry) return;

    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const fit = () => {
      if (!container.isConnected || container.clientWidth <= 0 || container.clientHeight <= 0) return;

      const availableWidth = Math.max(320, container.clientWidth - 16);
      const availableHeight = Math.max(180, container.clientHeight - 24);
      const activeDiv = document.getElementById(`xterm-tab-${activeTerminalTabId}`);
      if (!activeDiv) return;

      wrapper.style.width = `${availableWidth}px`;
      wrapper.style.height = `${availableHeight}px`;
      activeDiv.style.width = '100%';
      activeDiv.style.height = `${availableHeight}px`;

      const previousCols = entry.term.cols;
      const previousRows = entry.term.rows;
      try {
        entry.fitAddon.fit();
      } catch (err) {
        console.error('[Terminal] Fit failed:', err);
        return;
      }

      const screen = activeDiv.querySelector('.xterm-screen') as HTMLElement | null;
      const renderedHeight = screen?.offsetHeight || availableHeight;
      wrapper.style.height = `${renderedHeight}px`;
      activeDiv.style.height = `${renderedHeight}px`;

      const cols = entry.term.cols;
      const rows = entry.term.rows;
      if (cols !== previousCols || rows !== previousRows) {
        setTerminalDims({ cols, rows });
        void invoke('terminal_resize', { sessionId: activeTerminalTabId, cols, rows }).catch((err) => {
          console.error('[Terminal] Resize failed:', err);
        });
      }
    };
    const scheduleFit = () => {
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(fit, 80);
    };

    const observer = new ResizeObserver(scheduleFit);
    observer.observe(container);
    scheduleFit();

    return () => {
      window.clearTimeout(resizeTimer);
      observer.disconnect();
    };
  }, [activeTerminalTabId, viewMode]);

  // ── 1s poll: refs own the hot-path counters; React updates only once per second ──
  useEffect(() => {
    let running = true;
    const poll = () => {
      if (!running) return;
      const activeId = activeTerminalTabId;

      let activeRx = 0;
      let activeTx = 0;
      bytesTrackerRef.current.forEach((tracker, tabId) => {
        if (tabId === activeId) {
          activeRx = tracker.rx;
          activeTx = tracker.tx;
        }
        tracker.rx = 0;
        tracker.tx = 0;
      });
      // The panel stays mounted outside terminal mode to preserve sessions.
      // Avoid forcing a React render every second while it is hidden.
      if (viewMode === 'terminal') {
        setDisplaySecRx(activeRx);
        setDisplaySecTx(activeTx);
      }

      if (activeId && viewMode === 'terminal') {
        const meta = sessionMetaRef.current.get(activeId);
        if (meta) {
          setDisplayAlive(meta.alive);
          setDisplayUptime(
            Math.floor(((meta.exitedAt ?? Date.now()) - meta.createdAt) / 1000),
          );
        }
      }
      setTimeout(poll, 1000);
    };
    poll();
    return () => { running = false; };
  }, [activeTerminalTabId, viewMode]);

  // ── Keyboard shortcut: Ctrl+Shift+T ──

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (viewMode !== 'terminal') return;
      if (e.ctrlKey && e.shiftKey && e.key === 'T') {
        e.preventDefault();
        createTerminal();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [createTerminal, viewMode]);

  // ── Theme update ──

  useEffect(() => {
    const theme = TERMINAL_THEMES[terminalTheme as keyof typeof TERMINAL_THEMES];
    terminalRef.current.forEach(({ term }) => {
      term.options.theme = theme;
    });
  }, [terminalTheme]);

  // Route changes (for example, opening Settings) unmount this panel. Close the
  // backend sessions instead of leaving shells alive with no xterm attached.
  useEffect(() => {
    const terminals = terminalRef.current;
    const listenerMap = unlistenRefs.current;
    const sessionMeta = sessionMetaRef.current;
    const byteTrackers = bytesTrackerRef.current;
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      registerTerminalInjector(null);

      terminals.forEach(({ term }) => {
        try {
          term.dispose();
        } catch { /* ignore */ }
      });
      terminals.clear();

      listenerMap.forEach((unlistens) => {
        unlistens.forEach((fn) => fn());
      });
      listenerMap.clear();

      const sessionIds = Array.from(sessionMeta.keys());
      sessionMeta.clear();
      byteTrackers.clear();
      sessionIds.forEach((sessionId) => {
        void invoke('terminal_close', { sessionId }).catch(() => {});
      });

      setTerminalTabs([]);
      setActiveTerminalTabId(null);
    };
  }, [setActiveTerminalTabId, setTerminalTabs]);

  // ── JSX ──

  const activeTab = terminalTabs.find((t) => t.id === activeTerminalTabId);
  const activeTheme = TERMINAL_THEMES[terminalTheme as keyof typeof TERMINAL_THEMES] ?? TERMINAL_THEMES['Catppuccin Dark'];

  return (
    <div className="terminal-shell h-full flex flex-col" style={terminalStyle(terminalTheme)}>
      {/* Tab bar */}
      <div
        className="terminal-chrome flex items-center h-9 shrink-0 px-1 gap-1"
      >
        {/* Shell selector + New button */}
        <div className="flex items-center gap-1 mr-2">
          <div className="relative">
            <button
              type="button"
              onClick={() => setShowShellMenu(!showShellMenu)}
              className="terminal-toolbar-button flex items-center gap-1 px-2 py-1 rounded text-xs"
              aria-label="选择终端类型"
              aria-expanded={showShellMenu}
            >
              {terminalShellType === 'powershell' ? (
                <Command className="w-3.5 h-3.5" />
              ) : (
                <TerminalIcon className="w-3.5 h-3.5" />
              )}
              <span>{terminalShellType === 'powershell' ? 'PowerShell' : 'CMD'}</span>
              <ChevronDown className="w-3 h-3" />
            </button>
            {showShellMenu && (
              <>
                <div className="fixed inset-0 z-10" onClick={() => setShowShellMenu(false)} />
                <div
                  role="menu"
                  className="terminal-popover absolute top-full left-0 mt-1 z-20 min-w-[140px]"
                >
                  {SHELL_OPTIONS.map((opt) => (
                    <button
                      type="button"
                      key={opt.value}
                      onClick={() => {
                        setTerminalShellType(opt.value);
                        setShowShellMenu(false);
                      }}
                      className="pd-menu-item terminal-menu-item"
                      data-active={opt.value === terminalShellType ? 'true' : undefined}
                    >
                      {opt.value === 'powershell' ? (
                        <Command className="w-3.5 h-3.5 shrink-0 opacity-80" />
                      ) : (
                        <TerminalIcon className="w-3.5 h-3.5 shrink-0 opacity-80" />
                      )}
                      {opt.label}
                      {opt.value === terminalShellType && (
                        <span className="terminal-menu-state text-[10px] ml-auto">active</span>
                      )}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
          <button
            type="button"
            onClick={() => createTerminal()}
            className="terminal-toolbar-button flex items-center gap-1 px-2 py-1 text-xs rounded"
            title="New Terminal (Ctrl+Shift+T)"
            aria-label="新建终端"
          >
            <Plus className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* Tabs */}
        <div className="flex-1 flex items-center overflow-x-auto">
          {terminalTabs.map((tab) => (
            <div
              key={tab.id}
              role="tab"
              tabIndex={0}
              aria-selected={tab.id === activeTerminalTabId}
              onClick={() => setActiveTerminalTabId(tab.id)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  setActiveTerminalTabId(tab.id);
                }
              }}
              className="terminal-tab flex items-center gap-1 px-3 py-1 text-xs rounded shrink-0"
              data-active={tab.id === activeTerminalTabId}
            >
              <Monitor className="w-3 h-3 shrink-0" />
              <span className="truncate max-w-[100px]">{tab.title}</span>
              <button
                type="button"
                onClick={(e) => closeTerminal(tab.id, e)}
                className="terminal-tab-close ml-1 p-0.5 rounded shrink-0"
                aria-label={`关闭 ${tab.title} 标签页`}
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          ))}
        </div>

        {/* Theme picker */}
        <div className="relative">
          <button
            type="button"
            onClick={() => setShowThemeMenu(!showThemeMenu)}
            className="terminal-toolbar-button flex items-center p-1 rounded"
            title="Theme"
            aria-label="选择终端主题"
            aria-expanded={showThemeMenu}
          >
            <Palette className="w-3.5 h-3.5" />
          </button>
          {showThemeMenu && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setShowThemeMenu(false)} />
              <div
                role="menu"
                className="terminal-popover absolute top-full right-0 mt-1 z-20 min-w-[160px]"
              >
                {THEME_NAMES.map((name) => (
                  <button
                    type="button"
                    key={name}
                    onClick={() => {
                      setTerminalTheme(name);
                      setShowThemeMenu(false);
                    }}
                    className="pd-menu-item terminal-menu-item"
                    data-active={name === terminalTheme ? 'true' : undefined}
                  >
                    <span
                      className="w-3 h-3 rounded-full shrink-0 border"
                      style={{
                        background: TERMINAL_THEMES[name].background,
                        borderColor: TERMINAL_THEMES[name].foreground,
                      }}
                    />
                    {name}
                    {name === terminalTheme && (
                      <span className="terminal-menu-state text-[10px] ml-auto">active</span>
                    )}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      </div>

      {/* Terminal content area — scrollable parent for oversized terminal; xterm handles scrollback internally */}
      <div
        className="terminal-viewport-area flex-1 relative overflow-hidden"
        style={{ background: activeTheme.background }}
      >
        {/* Empty state — Terminal 欢迎首页：零状态不自动创建终端，由用户手动发起 */}
        {terminalTabs.length === 0 && (
          <div className="terminal-empty-state absolute inset-0 flex justify-center p-5 overflow-y-auto">
            <div className="terminal-empty-content w-full max-w-xl flex flex-col items-center gap-4 my-auto text-center">
              {/* Logo / Title */}
              <div className="flex flex-col items-center gap-2.5">
                <div className="terminal-empty-logo w-14 h-14 rounded-2xl flex items-center justify-center">
                  <TerminalIcon className="w-7 h-7" strokeWidth={2} />
                </div>
                <div className="flex flex-col items-center gap-1">
                  <h2 className="terminal-empty-title text-xl font-semibold tracking-tight">虚拟终端</h2>
                  <p className="terminal-empty-description text-xs">
                    选择终端类型并创建标签页，即可开始执行命令
                  </p>
                </div>
              </div>

              {/* Primary CTA and default shell stay on one compact action row. */}
              <div className="flex flex-col items-center gap-2">
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <button
                    type="button"
                    onClick={() => createTerminal()}
                    className="terminal-primary-button group h-10 shrink-0 inline-flex items-center gap-2 px-5 rounded-lg font-medium text-sm whitespace-nowrap active:scale-[.98]"
                    title="新建终端 (Ctrl + Shift + T)"
                  >
                    <Plus className="w-4 h-4" />
                    新建终端
                  </button>

                  <div className="terminal-segmented h-10 shrink-0 flex items-center gap-1.5 p-1 rounded-lg">
                    <span className="terminal-empty-label text-[10px] px-1.5 pr-1">使用</span>
                    {SHELL_OPTIONS.map((s) => (
                      <button
                        type="button"
                        key={s.value}
                        onClick={() => setTerminalShellType(s.value as 'powershell' | 'cmd')}
                        className="terminal-segment h-full inline-flex items-center gap-1.5 px-2.5 rounded-md text-xs whitespace-nowrap"
                        data-active={terminalShellType === s.value}
                        title={`选择默认终端类型：${s.label}`}
                      >
                        {s.value === 'powershell' ? <Command className="w-3 h-3" /> : <TerminalIcon className="w-3 h-3" />}
                        {s.label}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="terminal-shortcut-hint inline-flex items-center gap-1.5 text-[10px]">
                  <Keyboard className="terminal-empty-label w-3 h-3" />
                  <kbd className="terminal-kbd px-1 py-0.5 rounded border">Ctrl</kbd>
                  <span className="terminal-empty-label">+</span>
                  <kbd className="terminal-kbd px-1 py-0.5 rounded border">Shift</kbd>
                  <span className="terminal-empty-label">+</span>
                  <kbd className="terminal-kbd px-1 py-0.5 rounded border">T</kbd>
                  <span className="terminal-empty-label ml-1">新建标签页</span>
                </div>
              </div>

              {/* Info cards: 主题 + 说明 */}
              <div className="grid grid-cols-2 gap-3 w-full">
                <div className="terminal-info-card p-3 rounded-lg text-left">
                  <div className="flex items-center gap-2 mb-1.5">
                    <Palette className="terminal-empty-label w-3.5 h-3.5" />
                    <span className="terminal-card-title text-xs font-medium">主题</span>
                  </div>
                  {/* 与右上角主题下拉同一数据源（THEME_NAMES 全量） */}
                  <div className="flex items-center gap-1.5 flex-wrap">
                    {THEME_NAMES.map((name) => {
                      const active = terminalTheme === name;
                      return (
                        <button
                          type="button"
                          key={name}
                          onClick={() => setTerminalTheme(name)}
                          className="w-6 h-6 rounded-md border transition-transform hover:scale-110"
                          style={{
                            background: TERMINAL_THEMES[name].background,
                            borderColor: active ? activeTheme.ui.accent : activeTheme.ui.border,
                            boxShadow: active ? `0 0 0 2px ${activeTheme.ui.accentSoft}` : 'none',
                          }}
                          title={`切换到 ${name}`}
                          aria-label={`切换到 ${name} 主题`}
                        />
                      );
                    })}
                  </div>
                </div>
                <div className="terminal-info-card p-3 rounded-lg text-left">
                  <div className="flex items-center gap-2 mb-1.5">
                    <Sparkles className="terminal-empty-label w-3.5 h-3.5" />
                    <span className="terminal-card-title text-xs font-medium">提示</span>
                  </div>
                  <p className="terminal-card-copy text-[11px] leading-relaxed">
                    每个标签页都是独立会话，可并行运行不同任务；关闭标签页会结束对应会话。
                  </p>
                </div>
              </div>
            </div>
          </div>
        )}
        
        {/* Scrollable container — at least full height so marginTop:auto bottom-aligns terminal */}
        <div ref={containerRef} style={{ minHeight: '100%', display: 'flex', flexDirection: 'column', padding: '12px 0 12px 16px' }}>
          {/* scaleWrapper: sized by JS after terminal render, bottom-aligned via marginTop:auto */}
          <div
            ref={scaleWrapperRef}
            className="relative"
          >
            {/* Tab content divs are managed imperatively by createTerminal */}
          </div>
        </div>
      </div>

      {/* Status bar — connection, shell, dims, scrollback, tab, transfer */}
      {activeTab && (
        <div className="terminal-statusbar terminal-chrome flex items-center h-6 shrink-0 px-3 gap-2.5 overflow-hidden whitespace-nowrap">
          {/* Connection status + uptime */}
          <span style={{ color: displayAlive ? 'var(--terminal-success)' : 'var(--terminal-danger)' }}>
            {displayAlive ? 'Live' : 'Exit'}
          </span>
          {displayUptime > 0 && <span className="terminal-status-muted text-[10px]">{formatUptime(displayUptime)}</span>}
          <span className="terminal-status-divider">|</span>
          {/* Dimensions */}
          <span>{terminalDims.cols} x {terminalDims.rows}</span>
          <span className="terminal-status-divider">|</span>
          {/* Per-second transfer volume */}
          <span title="每秒发送数据量">
            {'↑'} {formatBytes(displaySecTx)}/s
          </span>
          <span title="每秒接收数据量">
            {'↓'} {formatBytes(displaySecRx)}/s
          </span>
          <span className="terminal-status-theme">{terminalTheme}</span>
        </div>
      )}
    </div>
  );
};
