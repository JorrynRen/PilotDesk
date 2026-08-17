import React, { createContext, useContext, useState } from 'react';

export type ViewMode = 'workflow' | 'session' | 'groupchat' | 'terminal' | 'custom';

export interface TerminalTabData {
  id: string;
  shellType: string;
  title: string;
  pid?: number;
  cwd?: string;
}

interface TerminalContextType {
  /** Current main panel view mode */
  viewMode: ViewMode;
  /** Switch to a specific mode (workflow/session/groupchat/terminal) */
  setMode: (mode: ViewMode) => void;
  /** Switch to terminal view */
  openConsole: () => void;
  /** Switch to session (chat) view */
  closeConsole: () => void;
  /** Toggle between session/terminal view */
  toggleConsole: () => void;
  /** Persisted terminal tabs (survives unmount) */
  terminalTabs: TerminalTabData[];
  /** Set terminal tabs */
  setTerminalTabs: React.Dispatch<React.SetStateAction<TerminalTabData[]>>;
  /** Currently active terminal tab id */
  activeTerminalTabId: string | null;
  /** Set active terminal tab */
  setActiveTerminalTabId: React.Dispatch<React.SetStateAction<string | null>>;
  /** Current shell type for new terminals */
  terminalShellType: string;
  /** Set shell type */
  setTerminalShellType: (shell: string) => void;
  /** Current theme */
  terminalTheme: string;
  /** Set theme */
  setTerminalTheme: (theme: string) => void;
}

const TerminalContext = createContext<TerminalContextType | undefined>(undefined);

export const useTerminal = () => {
  const context = useContext(TerminalContext);
  if (!context) {
    throw new Error('useTerminal must be used within a TerminalProvider');
  }
  return context;
};

interface TerminalProviderProps {
  children: React.ReactNode;
}

export const TerminalProvider: React.FC<TerminalProviderProps> = ({ children }) => {
  const [viewMode, setViewMode] = useState<ViewMode>('session');
  const [terminalTabs, setTerminalTabs] = useState<TerminalTabData[]>([]);
  const [activeTerminalTabId, setActiveTerminalTabId] = useState<string | null>(null);
  const [terminalShellType, setTerminalShellType] = useState('powershell');
  const [terminalTheme, setTerminalTheme] = useState<string>('Catppuccin Dark');

  const setMode = (mode: ViewMode) => setViewMode(mode);
  const openConsole = () => setViewMode('terminal');
  const closeConsole = () => setViewMode('session');
  const toggleConsole = () => setViewMode((prev) => (prev === 'session' ? 'terminal' : 'session'));

  return (
    <TerminalContext.Provider value={{
      viewMode, setMode, openConsole, closeConsole, toggleConsole,
      terminalTabs, setTerminalTabs,
      activeTerminalTabId, setActiveTerminalTabId,
      terminalShellType, setTerminalShellType,
      terminalTheme, setTerminalTheme,
    }}>
      {children}
    </TerminalContext.Provider>
  );
};
