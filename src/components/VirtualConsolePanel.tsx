import React, { useRef, useEffect, useState, useCallback } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Plus, X, Monitor, ChevronDown, Palette } from 'lucide-react';
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

  // terminal instance map: tabId -> { term, fitAddon }
  const terminalRef = useRef<Map<string, { term: Terminal; fitAddon: FitAddon }>>(new Map());
  const containerRef = useRef<HTMLDivElement>(null);
  // Tauri event listener unlisten functions: tabId -> UnlistenFn[]
  const unlistenRefs = useRef<Map<string, UnlistenFn[]>>(new Map());
  // Track ConPTY's known dimensions to avoid redundant resizes
  const conptyDimsRef = useRef<Map<string, { cols: number; rows: number }>>(new Map());
  // Guard against ResizeObserver triggering createTerminal while one is in-flight
  const creatingRef = useRef(false);
  const observerRef = useRef<ResizeObserver | null>(null);
  // Debounce timer for resize operations — prevents rapid ConPTY resize during window drag
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Track which sessions have received first data (banner) — skip all fit/resize until then
  const firstDataReceivedRef = useRef<Set<string>>(new Set());

  // Mirror tabs count in a ref for ResizeObserver callback (avoids stale closure)
  const tabsCountRef = useRef(terminalTabs.length);
  tabsCountRef.current = terminalTabs.length;

  // ── Create terminal ──

  const createTerminal = useCallback(async () => {
    if (creatingRef.current) return; // Already creating
    const container = containerRef.current;
    if (!container || container.offsetWidth === 0 || container.offsetHeight === 0) return;
    creatingRef.current = true;
    try {
    // (body continues below)

    // 1. Pre-calculate dimensions with a temporary Terminal so ConPTY starts at the right size.
    //    This eliminates the need for post-creation resize, preserving the shell banner.
    const tempDiv = document.createElement('div');
    tempDiv.style.cssText = 'position:absolute;top:0;left:0;width:100%;height:100%;padding:4px 4px 4px 8px;visibility:hidden';
    container.appendChild(tempDiv);
    const dummy = new Terminal({
      fontFamily: 'Cascadia Code, Consolas, "Courier New", monospace',
      fontSize: 14, lineHeight: 1.2, cols: 80, rows: 24,
    });
    const dummyFit = new FitAddon();
    dummy.loadAddon(dummyFit);
    dummy.open(tempDiv);
    dummyFit.fit();
    const dims = dummyFit.proposeDimensions() || { cols: 80, rows: 24 };
    dummy.dispose();
    container.removeChild(tempDiv);

    // 2. Create backend ConPTY session at the calculated size
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
      if (initialData) {
      }
    } catch (err) {
      console.error('[Terminal] Failed to create session:', err);
      return;
    }

    // Record ConPTY initial dimensions (so ResizeObserver won't trigger a redundant resize)
    conptyDimsRef.current.set(sessionId, { cols: dims.cols, rows: dims.rows });

    // 3. Create persistent div for this tab's xterm instance
    const tabDiv = document.createElement('div');
    tabDiv.id = `xterm-tab-${sessionId}`;
    tabDiv.style.cssText = 'position:absolute;top:0;left:0;width:100%;height:100%;padding:4px 4px 4px 8px';
    container.querySelectorAll('[id^="xterm-tab-"]').forEach((el) => {
      (el as HTMLElement).style.display = 'none';
    });
    tabDiv.style.display = 'block';
    container.appendChild(tabDiv);

    // 4. Create xterm instance — pre-set cols/rows to match ConPTY exactly
    const term = new Terminal({
      theme: TERMINAL_THEMES[terminalTheme],
      fontFamily: 'Cascadia Code, Consolas, "Courier New", monospace',
      fontSize: 14, lineHeight: 1.2,
      cols: dims.cols, rows: dims.rows,
      cursorBlink: true, cursorStyle: 'bar',
      scrollback: 10000, allowProposedApi: true, convertEol: false,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(tabDiv);
    terminalRef.current.set(sessionId, { term, fitAddon });

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

    // 6. Register event listeners (await to ensure they're ready before read loop)
    await setupTerminalListeners(sessionId, term);

    // 7. Start backend read loop (await to ensure read_loop is running before any resize)
    try {
      await invoke('terminal_attach', { sessionId });
    } catch (err) {
      console.error('[Terminal] Attach failed:', err);
    }

    // Mark that this tab should NOT be resized until first data arrives.
    // This protects the banner from being overwritten by a premature fit/resize.
    // firstDataReceivedRef will be set to true in the output listener callback.
    firstDataReceivedRef.current.delete(sessionId);

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

    const p1 = listen<string>(`terminal://output/${tabId}`, (event) => {
      // Mark banner received so debounce resize can proceed
      if (!firstDataReceivedRef.current.has(tabId)) {
        firstDataReceivedRef.current.add(tabId);
      }
      term.write(event.payload);
    }).then((fn) => {
      unlistens.push(fn);
    });

    const p2 = listen(`terminal://exited`, (event: any) => {
      if (event.payload?.session_id === tabId) {
        term.writeln('\r\n\x1b[90m[Process exited]\x1b[0m');
      }
    }).then((fn) => { unlistens.push(fn); });

    term.onData((data) => {
      invoke('terminal_write', {
        sessionId: tabId,
        data,
      }).catch((err) => {
        console.error('[Terminal] Write error:', err);
      });
    });

    unlistenRefs.current.set(tabId, unlistens);
    return Promise.all([p1, p2]).then(() => {});
  }, []);

  // ── Close terminal tab ──

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

    conptyDimsRef.current.delete(tabId);
    firstDataReceivedRef.current.delete(tabId);

    try {
      await invoke('terminal_close', { sessionId: tabId });
    } catch (err) {
      console.error('[Terminal] Failed to close session:', err);
    }

    const tabDiv = document.getElementById(`xterm-tab-${tabId}`);
    if (tabDiv?.parentNode) {
      tabDiv.parentNode.removeChild(tabDiv);
    }

    setTerminalTabs((prev) => {
      const next = prev.filter((t) => t.id !== tabId);
      if (activeTerminalTabId === tabId) {
        const newActiveId = next.length > 0 ? next[next.length - 1].id : null;
        if (newActiveId) {
          const div = document.getElementById(`xterm-tab-${newActiveId}`);
          if (div) div.style.display = 'block';
        }
        setActiveTerminalTabId(newActiveId);
      }
      return next;
    });
  }, [activeTerminalTabId]);

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
    }
  }, [activeTerminalTabId]);

  // ── ResizeObserver: auto-create + fit + smart resize ──
  //    This is the ONLY effect that responds to container size changes.
  //    No visibility tracking, no mode-switch-specific logic.

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const observer = new ResizeObserver(() => {
      // Skip all processing during terminal creation
      if (creatingRef.current) return;

      const w = container.offsetWidth;
      const h = container.offsetHeight;

      // Auto-create first terminal when panel becomes visible with no tabs
      if (tabsCountRef.current === 0 && w > 0 && h > 0) {
        observer.disconnect(); // Pause observer during async create
        createTerminal();
        return;
      }

      // Ignore zero-size observations (display:none or transition)
      if (w === 0 || h === 0) return;

      if (!activeTerminalTabId) return;
      const entry = terminalRef.current.get(activeTerminalTabId);
      if (!entry) return;

      // Don't resize until shell has output its first data (banner).
      // This prevents premature fit/resize from overwriting the banner.
      if (!firstDataReceivedRef.current.has(activeTerminalTabId)) return;

      // DEBOUNCE: Cancel any pending resize and schedule a new one after 150ms.
      // This prevents multiple ConPTY resizes during window drag, which causes
      // shell screen buffer redraws → content loss and duplication.
      if (resizeTimerRef.current !== null) {
        clearTimeout(resizeTimerRef.current);
      }
      resizeTimerRef.current = setTimeout(() => {
        resizeTimerRef.current = null;
        try {
          // Send VT clear-screen to xterm BEFORE fit to prevent content duplication.
          // fit() changes cols → text rewraps; then ConPTY resize sends VT redraw.
          // Without clearing, both rewrapped text AND ConPTY redraw are visible.
          // \x1b[2J clears entire screen — ConPTY redraw becomes the sole source.
          // Clear xterm visible area before fit to prevent content duplication.
          // ConPTY resize will redraw the screen buffer via VT sequences,
          // so clearing here ensures the final content comes only from ConPTY.
          const term = entry.term;
          const buffer = term.buffer.active;
          const totalLines = buffer.length;
          const viewRows = term.rows;
          const base = buffer.baseY + viewRows;
          const scrollback = totalLines - base;
          if (scrollback > 0) {
            // Preserve scrollback, clear only visible viewport
            for (let r = 0; r < viewRows; r++) {
              const line = buffer.getLine(base + r - viewRows);
              if (line) line.clear();
            }
          }
          
          entry.fitAddon.fit();
          const newDims = entry.fitAddon.proposeDimensions();
          if (!newDims?.cols || !newDims?.rows) return;

          const conptyKnown = conptyDimsRef.current.get(activeTerminalTabId);
          if (conptyKnown && conptyKnown.cols === newDims.cols && conptyKnown.rows === newDims.rows) return;

          conptyDimsRef.current.set(activeTerminalTabId, { cols: newDims.cols, rows: newDims.rows });
          invoke('terminal_resize', {
            sessionId: activeTerminalTabId,
            cols: newDims.cols,
            rows: newDims.rows,
          }).catch(() => {});
        } catch { /* ignore */ }
      }, 150);
    });

    observerRef.current = observer;
    observer.observe(container);
    return () => {
      observer.disconnect();
      observerRef.current = null;
      if (resizeTimerRef.current !== null) {
        clearTimeout(resizeTimerRef.current);
        resizeTimerRef.current = null;
      }
    };
  }, [activeTerminalTabId, createTerminal]);

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
              <Monitor className="w-3.5 h-3.5" />
              <span>{terminalShellType === 'powershell' ? 'PS' : 'CMD'}</span>
              <ChevronDown className="w-3 h-3" />
            </button>
            {showShellMenu && (
              <>
                <div className="fixed inset-0 z-10" onClick={() => setShowShellMenu(false)} />
                <div
                  className="absolute top-full left-0 mt-1 z-20 rounded-md shadow-lg py-1 min-w-[120px]"
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

      {/* Terminal content area */}
      <div className="flex-1 relative overflow-hidden">
        <div ref={containerRef} className="absolute inset-0">
          {terminalTabs.length === 0 && (
            <div className="h-full flex flex-col items-center justify-center gap-4" style={{ color: 'rgba(255,255,255,0.3)' }}>
              <Monitor className="w-12 h-12" />
              <div className="text-center">
                <p className="text-sm font-medium">No active terminals</p>
                <p className="text-xs mt-1">
                  Press <kbd className="px-1.5 py-0.5 rounded text-[10px] font-mono" style={{ background: 'rgba(255,255,255,0.1)' }}>Ctrl+Shift+T</kbd> to open a terminal
                </p>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
