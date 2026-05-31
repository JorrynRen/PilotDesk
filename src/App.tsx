import { useState, useCallback, useEffect } from 'react';
import { useSessionStore } from './stores/sessionStore';
import { useWebSocket } from './hooks/useWebSocket';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';
import { MarketPage } from './components/inspiration/MarketPage';
import { SettingsPage } from './pages/SettingsPage';
import './styles/ui.css';

type PageView = 'main' | 'market' | 'settings';

function App() {
  const [currentPage, setCurrentPage] = useState<PageView>('main');
  const [rightPanelOpen, setRightPanelOpen] = useState(true);
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  // WebSocket status monitoring (shared with StatusBar)
  const { isConnected: wsConnected } = useWebSocket(19830);

  // Update window title based on current session
  useEffect(() => {
    const base = 'PilotDesk';
    if (currentPage === 'market') {
      document.title = `${base} - 灵感市集`;
    } else if (currentPage === 'settings') {
      document.title = `${base} - 设置`;
    } else if (currentSession) {
      document.title = `${base} - ${currentSession.title}`;
    } else {
      document.title = base;
    }
  }, [currentPage, currentSession]);

  const handleSendToSession = useCallback((content: string) => {
    setCurrentPage('main');
    sessionStorage.setItem('pilotdesk_pending_input', content);
  }, []);

  return (
    <div
      className="h-screen flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)' }}
    >
      <TitleBar
        onOpenSettings={() => setCurrentPage("settings")}
        onToggleRightPanel={() => setRightPanelOpen((v) => !v)}
        rightPanelOpen={rightPanelOpen}
      />
      {currentPage === 'market' ? (
        <MarketPage onBack={() => setCurrentPage('main')} onSendToSession={handleSendToSession} />
      ) : currentPage === 'settings' ? (
        <SettingsPage onBack={() => setCurrentPage('main')} />
      ) : (
        <div className="flex-1 flex overflow-hidden relative">
          <SessionList />
          <MainPanel />
          <RightPanel isOpen={rightPanelOpen} />
        </div>
      )}
      <StatusBar onOpenSettings={() => setCurrentPage('settings')} wsConnected={wsConnected} />
    </div>
  );
}

export default App;
