import React, { createContext, useContext, useState } from 'react';
import { MessageSquare, Terminal } from 'lucide-react';

type ViewMode = 'session' | 'terminal';

interface TerminalTabData {
  id: string;
  shellType: string;
  title: string;
  pid?: number;
  cwd?: string;
}

interface VirtualConsoleContextType {
  /** Current main panel view mode */
  viewMode: ViewMode;
  /** Switch to terminal view */
  openConsole: () => void;
  /** Switch to session (chat) view */
  closeConsole: () => void;
  /** Toggle between session/terminal view */
  toggleConsole: () => void;
  /** Persisted terminal tabs (survives unmount) */
  terminalTabs: TerminalTabData[];
  /** Set terminal tabs */
  setTerminalTabs: (tabs: TerminalTabData[]) => void;
  /** Currently active terminal tab id */
  activeTerminalTabId: string | null;
  /** Set active terminal tab */
  setActiveTerminalTabId: (id: string | null) => void;
  /** Current shell type for new terminals */
  terminalShellType: string;
  /** Set shell type */
  setTerminalShellType: (shell: string) => void;
  /** Current theme */
  terminalTheme: string;
  /** Set theme */
  setTerminalTheme: (theme: string) => void;
}

const VirtualConsoleContext = createContext<VirtualConsoleContextType | undefined>(undefined);

export const useVirtualConsole = () => {
  const context = useContext(VirtualConsoleContext);
  if (!context) {
    throw new Error('useVirtualConsole must be used within a VirtualConsoleProvider');
  }
  return context;
};

interface VirtualConsoleProviderProps {
  children: React.ReactNode;
}

export const VirtualConsoleProvider: React.FC<VirtualConsoleProviderProps> = ({ children }) => {
  const [viewMode, setViewMode] = useState<ViewMode>('session');
  const [terminalTabs, setTerminalTabs] = useState<TerminalTabData[]>([]);
  const [activeTerminalTabId, setActiveTerminalTabId] = useState<string | null>(null);
  const [terminalShellType, setTerminalShellType] = useState('powershell');
  const [terminalTheme, setTerminalTheme] = useState<string>('Catppuccin Dark');

  const openConsole = () => setViewMode('terminal');
  const closeConsole = () => setViewMode('session');
  const toggleConsole = () => setViewMode((prev) => (prev === 'session' ? 'terminal' : 'session'));

  return (
    <VirtualConsoleContext.Provider value={{
      viewMode, openConsole, closeConsole, toggleConsole,
      terminalTabs, setTerminalTabs,
      activeTerminalTabId, setActiveTerminalTabId,
      terminalShellType, setTerminalShellType,
      terminalTheme, setTerminalTheme,
    }}>
      {children}
    </VirtualConsoleContext.Provider>
  );
};

// Virtual console toggle button (外观与 TitleBar 的 segmented 滑动开关完全一致：尺寸略放大版)
export const VirtualConsoleButton: React.FC = () => {
  const { toggleConsole, viewMode } = useVirtualConsole();
  const isTerminal = viewMode === 'terminal';

  return (
    <div
      className="relative flex items-center select-none"
      role="group"
      aria-label="视图模式切换"
      style={{
        backgroundColor: 'var(--bg-tertiary)',
        border: '1px solid var(--border-strong, rgba(0,0,0,0.12))',
        padding: 2,
        borderRadius: 8,
        height: 30,
        width: 164,
        boxShadow: 'inset 0 1px 1px rgba(0,0,0,0.04)',
      }}
    >
      <button
        onClick={() => { if (isTerminal) toggleConsole(); }}
        className="relative z-10 flex items-center justify-center gap-1.5 flex-1 h-full rounded-[6px] text-[12px] transition-colors"
        style={{
          color: !isTerminal ? '#fff' : 'var(--text-secondary)',
          fontWeight: !isTerminal ? 600 : 500,
        }}
        title="切换到客户端模式"
      >
        <MessageSquare size={13} />
        客户端
      </button>
      <button
        onClick={() => { if (!isTerminal) toggleConsole(); }}
        className="relative z-10 flex items-center justify-center gap-1.5 flex-1 h-full rounded-[6px] text-[12px] transition-colors"
        style={{
          color: isTerminal ? '#fff' : 'var(--text-secondary)',
          fontWeight: isTerminal ? 600 : 500,
        }}
        title="切换到终端模式"
      >
        <Terminal size={13} />
        终端
      </button>
      <div
        aria-hidden
        className="absolute top-[2px] rounded-[6px]"
        style={{
          height: 'calc(100% - 4px)',
          width: 'calc(50% - 2px)',
          backgroundColor: 'var(--accent)',
          left: isTerminal ? 'calc(50% + 1px)' : 2,
          transition: 'left 180ms cubic-bezier(.22,.61,.36,1)',
          boxShadow:
            '0 1px 3px rgba(0,0,0,0.15), 0 0 0 1px rgba(0,0,0,0.06), inset 0 1px 0 rgba(255,255,255,0.18)',
          zIndex: 0,
        }}
      />
    </div>
  );
};

// Re-export for backward compatibility
export const VirtualConsoleTrigger: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { openConsole } = useVirtualConsole();
  return <div onClick={openConsole}>{children}</div>;
};

// Direct context hook (without React 19 Compiler wrapper issues)
export const useVirtualConsoleState = () => {
  const { viewMode, openConsole, closeConsole, toggleConsole } = useVirtualConsole();
  return { viewMode, openConsole, closeConsole, toggleConsole };
};
