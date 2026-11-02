import React, { useState } from 'react';
import { TerminalContext, type TerminalTabData, type ViewMode } from './TerminalManager';

interface TerminalProviderProps {
  children: React.ReactNode;
}

/**
 * 视图模式 / 终端标签的 Provider。
 *
 * 单独成文件：`react-refresh/only-export-components` 要求一个文件要么只导出组件、要么只导出非组件，
 * 而 TerminalManager.tsx 里还有 `useTerminal` 等 hook 与工具函数（被大量调用方按原路径引用），
 * 故把组件拆出来，TerminalManager 只保留非组件导出。
 */
export const TerminalProvider: React.FC<TerminalProviderProps> = ({ children }) => {
  const [viewMode, setViewMode] = useState<ViewMode>('session');
  /** 终端面板是否已经挂载过：为真后不再回退（卸载 xterm 会丢会话内容） */
  const [terminalEverOpened, setTerminalEverOpened] = useState(false);
  const [terminalTabs, setTerminalTabs] = useState<TerminalTabData[]>([]);
  const [activeTerminalTabId, setActiveTerminalTabId] = useState<string | null>(null);
  const [terminalShellType, setTerminalShellType] = useState('powershell');
  const [terminalTheme, setTerminalTheme] = useState<string>('Catppuccin Dark');

  // 切换动作本身标记「用过终端」：不放在 effect 里派生，避免多一轮渲染
  const setMode = (mode: ViewMode) => {
    setViewMode(mode);
    if (mode === 'terminal') setTerminalEverOpened(true);
  };
  const openConsole = () => setMode('terminal');
  const closeConsole = () => setMode('session');
  const toggleConsole = () => setMode(viewMode === 'session' ? 'terminal' : 'session');

  return (
    <TerminalContext.Provider value={{
      viewMode, setMode, openConsole, closeConsole, toggleConsole, terminalEverOpened,
      terminalTabs, setTerminalTabs,
      activeTerminalTabId, setActiveTerminalTabId,
      terminalShellType, setTerminalShellType,
      terminalTheme, setTerminalTheme,
    }}>
      {children}
    </TerminalContext.Provider>
  );
}
