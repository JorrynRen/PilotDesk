import React, { createContext, useContext } from 'react';

export type ViewMode = 'workflow' | 'session' | 'groupchat' | 'terminal' | 'custom';

export interface TerminalTabData {
  id: string;
  shellType: string;
  title: string;
  pid?: number;
  cwd?: string;
}

export interface TerminalContextType {
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

export const TerminalContext = createContext<TerminalContextType | undefined>(undefined);

export const useTerminal = () => {
  const context = useContext(TerminalContext);
  if (!context) {
    throw new Error('useTerminal must be used within a TerminalProvider');
  }
  return context;
};

// ── 活跃终端注入桥 ──
// TerminalPanel 将“向当前活跃终端输入点注入文本”的能力注册于此；
// 灵感面板等外部组件在终端模式下借此把内容直接送达终端（term.paste 语义）。
// 不用 context 而是模块级单例：避免每 tick 改动触发 context 重渲染，且调用方无需 Provider。
type TerminalInjector = (text: string) => boolean;
let activeInjector: TerminalInjector | null = null;

export function registerTerminalInjector(injector: TerminalInjector | null): void {
  activeInjector = injector;
}

/** 向当前活跃终端输入点注入文本；无活跃终端或注入失败时返回 false。 */
export function injectToActiveTerminal(text: string): boolean {
  if (!activeInjector) return false;
  try {
    return activeInjector(text);
  } catch {
    return false;
  }
}
