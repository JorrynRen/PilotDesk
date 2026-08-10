import React, { useRef, useEffect, useState, useCallback } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Plus, X, Monitor, ChevronDown, Palette, Terminal as TerminalIcon, Command, Keyboard, Sparkles } from 'lucide-react';
import '@xterm/xterm/css/xterm.css';
import { useVirtualConsole } from '../VirtualConsoleManager';

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
  },
};

const THEME_NAMES = Object.keys(TERMINAL_THEMES) as (keyof typeof TERMINAL_THEMES)[];

const SHELL_OPTIONS = [
  { value: 'powershell', label: 'PowerShell' },
  { value: 'cmd', label: 'CMD' },
];

// ── Types ──

interface VirtualConsolePanelProps {
  onClose?: () => void;
}

// ── Main Component ──
// Design principle: this component has ZERO awareness of visibility / mode switching.
// It is always mounted. The parent controls display via CSS.
// The only "auto" behavior is: create a terminal when no tabs exist and container has size.

export const VirtualConsolePanel: React.FC<VirtualConsolePanelProps> = () => {
  const {
    terminalTabs, setTerminalTabs,
    activeTerminalTabId, setActiveTerminalTabId,
    terminalShellType, setTerminalShellType,
    terminalTheme, setTerminalTheme,
  } = useVirtualConsole();

  const [showShellMenu, setShowShellMenu] = useState(false);
  const [showThemeMenu, setShowThemeMenu] = useState(false);
  const [terminalDims, setTerminalDims] = useState<{ cols: number; rows: number }>({ cols: 0, rows: 0 });
  const [consoleConfig, setConsoleConfig] = useState<{ consoleType: string; bufferSize: number; maxLines: number | null } | null>(null);
  // Terminal cols/rows are FIXED at creation time — never changed by resize.
  // Initial size is measured from the container to fill the content area exactly.

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
  // Guard against ResizeObserver triggering createTerminal while one is in-flight
  const creatingRef = useRef(false);
  const observerRef = useRef<ResizeObserver | null>(null);
  // Ref to scale wrapper div — tab divs must be appended here, not to containerRef
  const scaleWrapperRef = useRef<HTMLDivElement>(null);
  // Store the pixel dimensions of the initial terminal render (for layout sizing)
  const terminalOrigSizeRef = useRef<{ w: number; h: number } | null>(null);
  // Session metadata: createdAt + alive status
  const sessionMetaRef = useRef<Map<string, { createdAt: number; alive: boolean }>>(new Map());
  // Byte counters for transfer tracking
  const bytesTrackerRef = useRef<Map<string, { rx: number; tx: number }>>(new Map());
  // Previous snapshot for rate calculation (1s interval)
  // Transmission state machine: idle | sending | waiting | receiving
  const txStateRef = useRef<Map<string, { state: 'idle' | 'sending' | 'waiting' | 'receiving'; lastRxTime: number }>>(new Map());


  // Mirror tabs count in a ref for ResizeObserver callback (avoids stale closure)
  const tabsCountRef = useRef(terminalTabs.length);
  tabsCountRef.current = terminalTabs.length;

  // ── Create terminal ──

  const createTerminal = useCallback(async () => {
    if (creatingRef.current) return; // Already creating
    const container = containerRef.current;

    creatingRef.current = true;
    try {
    // 1. Create terminal in actual DOM, fit to measure exact cols, then create ConPTY.
    const scaleWrapper = scaleWrapperRef.current;
    if (!scaleWrapper) {
      console.error('[Terminal] Scale wrapper not ready');
      return;
    }
    // Create tab div first (placeholder id, will be updated after ConPTY creation)
    const tabDiv = document.createElement('div');
    tabDiv.style.cssText = '';
    scaleWrapper.querySelectorAll('[id^="xterm-tab-"]').forEach((el) => {
      (el as HTMLElement).style.display = 'none';
    });
    tabDiv.style.display = 'block';
    scaleWrapper.appendChild(tabDiv);

    // Create xterm with placeholder cols (80), rows fixed at 150
    const term = new Terminal({
      theme: TERMINAL_THEMES[terminalTheme],
      fontFamily: 'Cascadia Code, Consolas, "Courier New", monospace',
      fontSize: 14, lineHeight: 1.2,
      cols: 80, rows: 150,
      cursorBlink: true, cursorStyle: 'bar',
      scrollback: 50000, allowProposedApi: true, convertEol: false,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(tabDiv);

    // Fit to actual container (rAF ensures layout complete)
    await new Promise(r => requestAnimationFrame(r));
    fitAddon.fit();
    // Reset scrollable-element padding + make viewport scrollbar overlay
    const scrollableEl = tabDiv.querySelector('.xterm-scrollable-element') as HTMLElement | null;
    if (scrollableEl) {
      scrollableEl.style.paddingRight = '0';
    }
    const viewport = tabDiv.querySelector('.xterm-viewport') as HTMLElement | null;
    if (viewport) {
      viewport.style.overflowY = 'overlay';
    }
    // Force xterm-screen (and child canvases) to viewport's content width
    const screen = tabDiv.querySelector('.xterm-screen') as HTMLElement | null;
    if (screen && viewport) {
      const w = viewport.clientWidth;
      screen.style.setProperty('width', w + 'px', 'important');
      screen.style.setProperty('max-width', w + 'px', 'important');
      const canvases = screen.querySelectorAll('canvas');
      canvases.forEach(c => {
        (c as HTMLElement).style.setProperty('width', w + 'px', 'important');
      });
    }
    const dims = { cols: term.cols, rows: 150 };

    // 2. Create backend ConPTY session at the fitted size
    let sessionId: string;
    let shellType: string;
    let initialData = "";
    try {
      const result = await invoke<{ session_id: string; shell_type: string; initial_data?: string }>('terminal_create', {
        shellType: terminalShellType,
        cols: dims.cols,
        rows: dims.rows,
      });
      sessionId = result.session_id;
      shellType = result.shell_type;
      initialData = result.initial_data || '';
    } catch (err) {
      console.error('[Terminal] Failed to create session:', err);
      return;
    }

    // 3. Update tabDiv id with real sessionId and store in refs
    tabDiv.id = `xterm-tab-${sessionId}`;
    terminalRef.current.set(sessionId, { term, fitAddon });

    // Initialize session metadata and byte tracking
    sessionMetaRef.current.set(sessionId, { createdAt: Date.now(), alive: true });
    bytesTrackerRef.current.set(sessionId, { rx: 0, tx: 0 });
    txStateRef.current.set(sessionId, { state: 'idle', lastRxTime: Date.now() });

    // Measure the rendered terminal pixel size and store for layout
    const screenEl = tabDiv.querySelector('.xterm-screen') as HTMLElement | null;
    const renderedH = screenEl?.offsetHeight || tabDiv.offsetHeight;
    terminalOrigSizeRef.current = { w: 0, h: renderedH };
    // Size the scaleWrapper to match terminal height, bottom-aligned in scrollable container
    if (scaleWrapper) {
      scaleWrapper.style.height = renderedH + 'px';
      scaleWrapper.style.marginTop = 'auto';
    }



    // Write banner data that was synchronously read from ConPTY pipe in Rust.
    // This guarantees CMD/PowerShell banner is displayed immediately.
    if (initialData) {
      term.write(initialData);
    }

    // 5. Add tab to state
    const tab: import('../VirtualConsoleManager').TerminalTabData = {
      id: sessionId,
      shellType,
      title: shellType.toUpperCase(),
    };
    setTerminalTabs((prev) => [...prev, tab]);
    setActiveTerminalTabId(sessionId);
    setTerminalDims({ cols: dims.cols, rows: dims.rows });

    // 6. Register event listeners (await to ensure they're ready before read loop)
    await setupTerminalListeners(sessionId, term);

    // 7. Start backend read loop (await to ensure read_loop is running before any resize)
    try {
      await invoke('terminal_attach', { sessionId });
    } catch (err) {
      console.error('[Terminal] Attach failed:', err);
    }

    term.focus();

    // Post-creation: re-enable ResizeObserver after a short delay
    // to allow shell to output banner before any resize can occur.
    setTimeout(() => {
      if (observerRef.current && containerRef.current) {
        observerRef.current.observe(containerRef.current);
      }
    }, 500);
    } finally {
      creatingRef.current = false;
    }
  }, [terminalShellType, terminalTheme]);

  // ── Setup event listeners ──

  const setupTerminalListeners = useCallback((tabId: string, term: Terminal): Promise<void> => {
    const unlistens: UnlistenFn[] = [];

    // Listen for terminal://created to capture pid
    const p_created = listen<{ session_id: string; pid: number }>('terminal://created', (event) => {
      if (event.payload.session_id === tabId) {
        setTerminalTabs(prev => prev.map(t => t.id === tabId ? { ...t, pid: event.payload.pid } : t));
      }
    }).then(fn => unlistens.push(fn));

    const p1 = listen<string>(`terminal://output/${tabId}`, (event) => {
      const payload = event.payload;
      if (payload) {
        const tracker = bytesTrackerRef.current.get(tabId);
        if (tracker) {
          tracker.rx += payload.length;
          setDisplaySecRx(tracker.rx);
          setTick(t => t + 1);
        }
        // Update tx state machine
        const txState = txStateRef.current.get(tabId);
        if (txState) {
          if (txState.state === 'sending' || txState.state === 'waiting') {
            txState.state = 'receiving';
          }
          txState.lastRxTime = Date.now();
        }
      }
      term.write(payload);
    }).then((fn) => {
      unlistens.push(fn);
    });

    const p2 = listen(`terminal://exited`, (event: any) => {
      if (event.payload?.session_id === tabId) {
        const meta = sessionMetaRef.current.get(tabId);
        if (meta) meta.alive = false;
        term.writeln('\r\n\x1b[90m[Process exited]\x1b[0m');
      }
    }).then((fn) => { unlistens.push(fn); });

    term.onData((data) => {
      const tracker = bytesTrackerRef.current.get(tabId);
      if (tracker) {
        tracker.tx += data.length;
        setDisplaySecTx(tracker.tx);
        setTick(t => t + 1);
      }
      // Detect Enter key to start tx state machine (only from idle state)
      if (data === '\r') {
        const txState = txStateRef.current.get(tabId);
        if (txState && txState.state === 'idle') {
          txState.state = 'sending';
          // After 150ms, if still sending, transition to waiting
          setTimeout(() => {
            const s = txStateRef.current.get(tabId);
            if (s && s.state === 'sending') s.state = 'waiting';
          }, 150);
        }
      }
      invoke('terminal_write', {
        sessionId: tabId,
        data,
      }).catch((err) => {
        console.error('[Terminal] Write error:', err);
      });
    });

    unlistenRefs.current.set(tabId, unlistens);
    return Promise.all([p_created, p1, p2]).then(() => {});
  }, []);

  // ── Close terminal tab ──
  //    Uses functional state updates to avoid stale closure issues with activeTerminalTabId.

  const closeTerminal = useCallback(async (tabId: string, event?: React.MouseEvent) => {
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

    try {
      await invoke('terminal_close', { sessionId: tabId });
    } catch (err) {
      console.error('[Terminal] Failed to close session:', err);
    }

    // Clean up tracking refs
    sessionMetaRef.current.delete(tabId);
    bytesTrackerRef.current.delete(tabId);
    txStateRef.current.delete(tabId);

    const tabDiv = document.getElementById(`xterm-tab-${tabId}`);
    if (tabDiv?.parentNode) {
      tabDiv.parentNode.removeChild(tabDiv);
    }

    // Use functional update to read latest state — avoids stale closure on activeTerminalTabId
    setTerminalTabs((prev) => {
      const next = prev.filter((t) => t.id !== tabId);
      // Check against current activeTerminalTabId via functional setter
      setActiveTerminalTabId((currentActive) => {
        if (currentActive === tabId) {
          const newActiveId = next.length > 0 ? next[next.length - 1].id : null;
          return newActiveId;
        }
        return currentActive;
      });
      return next;
    });
  }, []);

  // ── Tab switch: CSS display toggle + focus only (NO fit ever) ──

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
      const tabDims = entry.fitAddon.proposeDimensions();
      if (tabDims?.cols && tabDims?.rows) {
        setTerminalDims({ cols: tabDims.cols, rows: tabDims.rows });
      }
    }
  }, [activeTerminalTabId]);

  // ── ResizeObserver: track container size (NO auto-create) ──
  //    Terminal cols/rows are FIXED at creation time — never changed.
  //    No fit/resize on container change — avoids xterm reflow vs ConPTY VT redraw conflicts.
  //    ✦ 重要：终端模式切换后不会自动创建默认终端 tab，
  //       由用户手动点击「新建终端」或 Ctrl+Shift+T 创建（与空态首页 CTA 一致）。

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const observer = new ResizeObserver(() => {
      // 仅用于当前活动终端尺寸检测；无终端时保持空态（不自动创建）
    });

    observerRef.current = observer;
    observer.observe(container);
    return () => {
      observer.disconnect();
      observerRef.current = null;
    };
  }, [createTerminal]);

  // ── 1s poll: direct state updates from refs (recursive setTimeout, no setInterval) ──
  const [tick, setTick] = useState(0);
  const [displayUptime, setDisplayUptime] = useState(0);
  const [displayAlive, setDisplayAlive] = useState(true);
  const [displayBufferLines, setDisplayBufferLines] = useState(0);
  const [displayTxState, setDisplayTxState] = useState<'idle' | 'sending' | 'waiting' | 'receiving'>('idle');

  // Per-second throughput: poll reads + resets counters, displays last-second values
  const [displaySecRx, setDisplaySecRx] = useState(0);
  const [displaySecTx, setDisplaySecTx] = useState(0);

  useEffect(() => {
    let running = true;
    const poll = () => {
      if (!running) return;
      const activeId = activeTerminalTabId;
      if (activeId) {
        const meta = sessionMetaRef.current.get(activeId);
        const tracker = bytesTrackerRef.current.get(activeId);
        const entry = terminalRef.current.get(activeId);
        const txState = txStateRef.current.get(activeId);
        if (meta) {
          setDisplayAlive(meta.alive);
          setDisplayUptime(Math.floor((Date.now() - meta.createdAt) / 1000));
        }
        if (tracker) {
          setDisplaySecRx(tracker.rx);
          setDisplaySecTx(tracker.tx);
          tracker.rx = 0;
          tracker.tx = 0;
        }
        setTick(t => t + 1);
        if (entry) {
          // baseY (scrollback lines) + cursorY (current line within viewport) + 1 = actual used lines
          setDisplayBufferLines(entry.term.buffer.active.baseY + entry.term.buffer.active.cursorY + 1);
        }
        if (txState) {
          if (txState.state === 'receiving' && Date.now() - txState.lastRxTime > 500) {
            txState.state = 'idle';
          }
          setDisplayTxState(txState.state);
        }
      }
      setTimeout(poll, 1000);
    };
    poll();
    return () => { running = false; };
  }, [activeTerminalTabId]);

  // ── Load console config ──
  useEffect(() => {
    invoke<Record<string, unknown>>('terminal_get_config').then((cfg) => {
      setConsoleConfig({
        consoleType: cfg.console_type as string,
        bufferSize: cfg.buffer_size as number,
        maxLines: cfg.max_lines as number | null,
      });
    }).catch(() => {});
  }, []);

  // ── Keyboard shortcut: Ctrl+Shift+T ──

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key === 'T') {
        e.preventDefault();
        createTerminal();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [createTerminal]);

  // ── Theme update ──

  useEffect(() => {
    const theme = TERMINAL_THEMES[terminalTheme];
    terminalRef.current.forEach(({ term }) => {
      // @ts-expect-error xterm.js internal API
      term.options.theme = theme;
    });
  }, [terminalTheme]);

  // ── No cleanup on unmount: terminals persist because component is always mounted ──

  // ── JSX ──

  const activeTab = terminalTabs.find((t) => t.id === activeTerminalTabId);

  return (
    <div className="h-full flex flex-col" style={{ background: TERMINAL_THEMES[terminalTheme].background }}>
      {/* Tab bar */}
      <div
        className="flex items-center h-9 shrink-0 px-1 gap-1"
        style={{
          borderBottom: '1px solid rgba(255,255,255,0.1)',
          background: 'rgba(0,0,0,0.2)',
        }}
      >
        {/* Shell selector + New button */}
        <div className="flex items-center gap-1 mr-2">
          <div className="relative">
            <button
              onClick={() => setShowShellMenu(!showShellMenu)}
              className="flex items-center gap-1 px-2 py-1 rounded text-xs transition-colors"
              style={{ color: 'rgba(255,255,255,0.7)' }}
              onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
              onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
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
                  className="absolute top-full left-0 mt-1 z-20 rounded-md shadow-lg py-1 min-w-[140px]"
                  style={{
                    background: '#2a2a3a',
                    border: '1px solid rgba(255,255,255,0.1)',
                  }}
                >
                  {SHELL_OPTIONS.map((opt) => (
                    <button
                      key={opt.value}
                      onClick={() => {
                        setTerminalShellType(opt.value);
                        setShowShellMenu(false);
                      }}
                      className="w-full text-left px-3 py-1.5 text-xs transition-colors flex items-center gap-2"
                      style={{ color: 'rgba(255,255,255,0.8)' }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                    >
                      {opt.value === 'powershell' ? (
                        <Command className="w-3.5 h-3.5 shrink-0 opacity-80" />
                      ) : (
                        <TerminalIcon className="w-3.5 h-3.5 shrink-0 opacity-80" />
                      )}
                      {opt.label}
                      {opt.value === terminalShellType && (
                        <span className="text-[10px] ml-auto" style={{ color: 'rgba(255,255,255,0.4)' }}>active</span>
                      )}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
          <button
            onClick={createTerminal}
            className="flex items-center gap-1 px-2 py-1 text-xs rounded transition-colors"
            style={{ color: 'rgba(255,255,255,0.7)' }}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
            title="New Terminal (Ctrl+Shift+T)"
          >
            <Plus className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* Tabs */}
        <div className="flex-1 flex items-center overflow-x-auto">
          {terminalTabs.map((tab) => (
            <div
              key={tab.id}
              onClick={() => setActiveTerminalTabId(tab.id)}
              className={`
                flex items-center gap-1 px-3 py-1 text-xs cursor-pointer rounded transition-colors shrink-0
              `}
              style={{
                background: tab.id === activeTerminalTabId ? 'rgba(255,255,255,0.15)' : 'transparent',
                color: tab.id === activeTerminalTabId ? 'rgba(255,255,255,0.95)' : 'rgba(255,255,255,0.6)',
              }}
              onMouseEnter={(e) => {
                if (tab.id !== activeTerminalTabId) (e.currentTarget.style.background = 'rgba(255,255,255,0.08)');
              }}
              onMouseLeave={(e) => {
                if (tab.id !== activeTerminalTabId) (e.currentTarget.style.background = 'transparent');
              }}
            >
              <Monitor className="w-3 h-3 shrink-0" />
              <span className="truncate max-w-[100px]">{tab.title}</span>
              <button
                onClick={(e) => closeTerminal(tab.id, e)}
                className="ml-1 p-0.5 rounded transition-colors shrink-0"
                style={{ color: 'rgba(255,255,255,0.5)' }}
                onMouseEnter={(e) => (e.currentTarget.style.color = 'rgba(255,255,255,0.9)')}
                onMouseLeave={(e) => (e.currentTarget.style.color = 'rgba(255,255,255,0.5)')}
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          ))}
        </div>

        {/* Theme picker */}
        <div className="relative">
          <button
            onClick={() => setShowThemeMenu(!showThemeMenu)}
            className="flex items-center p-1 rounded transition-colors"
            style={{ color: 'rgba(255,255,255,0.5)' }}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
            title="Theme"
          >
            <Palette className="w-3.5 h-3.5" />
          </button>
          {showThemeMenu && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setShowThemeMenu(false)} />
              <div
                className="absolute top-full right-0 mt-1 z-20 rounded-md shadow-lg py-1 min-w-[160px]"
                style={{
                  background: '#2a2a3a',
                  border: '1px solid rgba(255,255,255,0.1)',
                }}
              >
                {THEME_NAMES.map((name) => (
                  <button
                    key={name}
                    onClick={() => {
                      setTerminalTheme(name);
                      setShowThemeMenu(false);
                    }}
                    className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                    style={{ color: 'rgba(255,255,255,0.8)' }}
                    onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.1)')}
                    onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
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
                      <span className="text-[10px] ml-auto" style={{ color: 'rgba(255,255,255,0.4)' }}>active</span>
                    )}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      </div>

      {/* Terminal content area — scrollable parent for oversized terminal; xterm handles scrollback internally */}
      <div className="flex-1 relative overflow-x-hidden overflow-y-auto terminal-viewport-area" style={{ background: TERMINAL_THEMES[terminalTheme].background }}>
        {/* Empty state — Terminal 欢迎首页：零状态不自动创建终端，由用户手动发起 */}
        {terminalTabs.length === 0 && (
          <div className="absolute inset-0 flex items-center justify-center p-6 overflow-y-auto">
            <div className="w-full max-w-xl flex flex-col items-center gap-7 text-center" style={{ color: 'rgba(255,255,255,0.85)' }}>
              {/* Logo / Title */}
              <div className="flex flex-col items-center gap-3">
                <div
                  className="w-16 h-16 rounded-2xl flex items-center justify-center shadow-lg"
                  style={{
                    background: 'linear-gradient(135deg, #5B7FFF 0%, #8B5CF6 100%)',
                    boxShadow: '0 8px 24px rgba(91,127,255,0.35)',
                  }}
                >
                  <TerminalIcon className="w-8 h-8 text-white" strokeWidth={2} />
                </div>
                <div className="flex flex-col items-center gap-1">
                  <h2 className="text-xl font-semibold tracking-tight">虚拟终端</h2>
                  <p className="text-xs opacity-60" style={{ color: 'rgba(255,255,255,0.55)' }}>
                    请手动创建一个终端标签页以开始使用
                  </p>
                </div>
              </div>

              {/* Primary CTA + Secondary shell picker */}
              <div className="flex flex-col items-center gap-3">
                <button
                  onClick={() => createTerminal()}
                  className="group inline-flex items-center gap-2 px-5 py-2.5 rounded-lg font-medium text-sm text-white transition-all active:scale-[.98]"
                  style={{
                    background: 'linear-gradient(135deg, #5B7FFF 0%, #8B5CF6 100%)',
                    boxShadow: '0 4px 14px rgba(91,127,255,0.4)',
                  }}
                  onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(1.08)')}
                  onMouseLeave={(e) => (e.currentTarget.style.filter = 'brightness(1)')}
                  title="新建终端 (Ctrl + Shift + T)"
                >
                  <Plus className="w-4 h-4" />
                  新建终端
                </button>

                {/* Shell 快捷选择：和顶部 Toolbar 同步 */}
                <div className="flex items-center gap-1.5 p-1 rounded-lg" style={{ background: 'rgba(255,255,255,0.05)', border: '1px solid rgba(255,255,255,0.08)' }}>
                  <span className="text-[10px] opacity-50 px-1.5 pr-2">使用</span>
                  {SHELL_OPTIONS.map((s) => (
                    <button
                      key={s.value}
                      onClick={() => {
                        setTerminalShellType(s.value as 'powershell' | 'cmd');
                        createTerminal();
                      }}
                      className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs transition-all"
                      style={{
                        background: terminalShellType === s.value ? 'rgba(255,255,255,0.12)' : 'transparent',
                        color: terminalShellType === s.value ? '#fff' : 'rgba(255,255,255,0.55)',
                        fontWeight: terminalShellType === s.value ? 600 : 500,
                      }}
                      onMouseEnter={(e) => { if (terminalShellType !== s.value) e.currentTarget.style.background = 'rgba(255,255,255,0.06)'; }}
                      onMouseLeave={(e) => { if (terminalShellType !== s.value) e.currentTarget.style.background = 'transparent'; }}
                      title={`新建 ${s.label} 终端`}
                    >
                      {s.value === 'powershell' ? <Command className="w-3 h-3" /> : <TerminalIcon className="w-3 h-3" />}
                      {s.label}
                    </button>
                  ))}
                </div>
              </div>

              {/* Divider */}
              <div className="w-full flex items-center gap-3">
                <div className="flex-1 h-px" style={{ background: 'rgba(255,255,255,0.08)' }} />
                <span className="text-[10px] opacity-40 uppercase tracking-wider">或使用快捷键</span>
                <div className="flex-1 h-px" style={{ background: 'rgba(255,255,255,0.08)' }} />
              </div>

              {/* Keyboard shortcut hint */}
              <div
                className="inline-flex items-center gap-2 px-3.5 py-2 rounded-lg"
                style={{
                  background: 'rgba(255,255,255,0.04)',
                  border: '1px solid rgba(255,255,255,0.08)',
                  fontFamily: 'Cascadia Code, Consolas, monospace',
                  fontSize: 12,
                }}
              >
                <Keyboard className="w-3.5 h-3.5 opacity-60" />
                <kbd className="px-1.5 py-0.5 rounded border text-[10px]" style={{ background: 'rgba(255,255,255,0.06)', borderColor: 'rgba(255,255,255,0.12)' }}>Ctrl</kbd>
                <span className="opacity-30">+</span>
                <kbd className="px-1.5 py-0.5 rounded border text-[10px]" style={{ background: 'rgba(255,255,255,0.06)', borderColor: 'rgba(255,255,255,0.12)' }}>Shift</kbd>
                <span className="opacity-30">+</span>
                <kbd className="px-1.5 py-0.5 rounded border text-[10px]" style={{ background: 'rgba(255,255,255,0.06)', borderColor: 'rgba(255,255,255,0.12)' }}>T</kbd>
                <span className="opacity-50 ml-1 text-[11px]">新建终端</span>
              </div>

              {/* Info cards: 主题 + 说明 */}
              <div className="grid grid-cols-2 gap-3 w-full mt-1">
                <div
                  className="p-3 rounded-xl text-left"
                  style={{
                    background: 'rgba(255,255,255,0.03)',
                    border: '1px solid rgba(255,255,255,0.06)',
                  }}
                >
                  <div className="flex items-center gap-2 mb-1.5">
                    <Palette className="w-3.5 h-3.5 opacity-60" />
                    <span className="text-xs font-medium">主题</span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    {['Catppuccin Dark', 'One Dark', 'Light', 'Blue'].slice(0, 4).map((name) => {
                      const active = terminalTheme === name;
                      return (
                        <button
                          key={name}
                          onClick={() => setTerminalTheme(name)}
                          className="w-6 h-6 rounded-md border transition-transform hover:scale-110"
                          style={{
                            background: TERMINAL_THEMES[name as keyof typeof TERMINAL_THEMES].background,
                            borderColor: active ? '#5B7FFF' : 'rgba(255,255,255,0.1)',
                            boxShadow: active ? '0 0 0 2px rgba(91,127,255,0.3)' : 'none',
                          }}
                          title={`切换到 ${name}`}
                        />
                      );
                    })}
                  </div>
                </div>
                <div
                  className="p-3 rounded-xl text-left"
                  style={{
                    background: 'rgba(255,255,255,0.03)',
                    border: '1px solid rgba(255,255,255,0.06)',
                  }}
                >
                  <div className="flex items-center gap-2 mb-1.5">
                    <Sparkles className="w-3.5 h-3.5 opacity-60" />
                    <span className="text-xs font-medium">提示</span>
                  </div>
                  <p className="text-[11px] leading-relaxed opacity-55">
                    终端会话创建后尺寸固定，不会随面板拉伸重新排版；
                    可随时在标签栏「+」新建更多会话并行使用。
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
        <div
          className="flex items-center h-6 shrink-0 px-3 gap-3"
          style={{
            borderTop: '1px solid rgba(255,255,255,0.1)',
            background: 'rgba(0,0,0,0.2)',
            color: 'rgba(255,255,255,0.5)',
            fontSize: '11px',
            fontFamily: 'Cascadia Code, Consolas, monospace',
          }}
        >
          {/* Connection status + uptime */}
          <span style={{ color: displayAlive ? '#10B981' : '#EF4444' }}>
            {displayAlive ? 'Live' : 'Exit'}
          </span>
          {displayUptime > 0 && <span className="text-[10px] opacity-70">{formatUptime(displayUptime)}</span>}
          <span style={{ color: 'rgba(255,255,255,0.25)' }}>|</span>
          {/* Shell type */}
          <span>{activeTab.shellType === 'powershell' ? 'PowerShell' : 'CMD'}</span>
          <span style={{ color: 'rgba(255,255,255,0.25)' }}>|</span>
          {/* Dimensions */}
          <span>{terminalDims.cols} x {terminalDims.rows}</span>
          <span style={{ color: 'rgba(255,255,255,0.25)' }}>|</span>
          {/* Buffer lines — actual line count from xterm buffer */}
          <span title="Buffer lines">{displayBufferLines} lines</span>
          <span style={{ color: 'rgba(255,255,255,0.25)' }}>|</span>
          {/* Tab index */}
          <span>Tab {terminalTabs.findIndex(t => t.id === activeTerminalTabId) + 1}/{terminalTabs.length}</span>
          <span style={{ color: 'rgba(255,255,255,0.25)' }}>|</span>
          {/* Transfer info: cumulative bytes + real-time state */}
          <span title="Data transmission">
            {'↑'} {formatBytes(displaySecTx)} {'↓'} {formatBytes(displaySecRx)}
          </span>
          <span
            style={{
              color: displayTxState === 'receiving' ? '#60A5FA' :
                     displayTxState === 'waiting' ? '#FBBF24' :
                     displayTxState === 'sending' ? '#F59E0B' :
                     'rgba(255,255,255,0.5)',
            }}
          >
            {displayTxState === 'idle' && '已就绪'}
            {displayTxState === 'sending' && '发送中...'}
            {displayTxState === 'waiting' && '等待回传...'}
            {displayTxState === 'receiving' && <span>回传中</span>}
          </span>
          <span className="ml-auto">{terminalTheme}</span>
        </div>
      )}
    </div>
  );
};
