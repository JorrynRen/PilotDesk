import React, { createContext, useContext, useState } from 'react';

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

// Virtual console toggle button (for TitleBar)
export const VirtualConsoleButton: React.FC = () => {
  const { toggleConsole, viewMode } = useVirtualConsole();

  return (
    <button
      onClick={toggleConsole}
      className={`flex items-center space-x-2 px-3 py-2 text-sm font-medium rounded-md transition-colors ${
        viewMode === 'terminal'
          ? 'bg-primary text-primary-foreground'
          : 'hover:bg-primary/10 text-[var(--text-secondary)]'
      }`}
      title={viewMode === 'terminal' ? '客户端模式' : '终端模式'}
    >
      <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
      </svg>
      <span>{viewMode === 'terminal' ? '客户端模式' : '终端模式'}</span>
    </button>
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
