import { useState, useEffect } from 'react';
import { useSessionStore } from './stores/sessionStore';
import { useSkillStore } from './stores/skillStore';
import { useConfigStore } from './stores/configStore';
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
  const { isConnected: wsConnected, requestAllSkills } = useWebSocket(19830, {
    onSkills: (agentType, skills) => {
      // Convert string[] to SkillInfo[]
      const skillInfos = (skills as string[]).map((name: string) => ({ name, description: '' }));
      useSkillStore.getState().setAgentSkills(agentType, skillInfos);
    },
  });

  // Load config on app startup
  const { fetchConfig } = useConfigStore();
  useEffect(() => {
    fetchConfig();
  }, [fetchConfig]);

  // Fetch all agent skills when WebSocket connects
  const { setLoading: setSkillsLoading } = useSkillStore();
  useEffect(() => {
    if (wsConnected) {
      setSkillsLoading(true);
      requestAllSkills();
    }
  }, [wsConnected, requestAllSkills, setSkillsLoading]);

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

  return (
    <div className="pilotdesk-glow">
      <div className="pilotdesk-window-shell">
        <div className="pilotdesk-window-content flex flex-col h-full">
          <TitleBar
            onOpenSettings={() => setCurrentPage(p => p === "settings" ? "main" : "settings")}
            onToggleRightPanel={() => setRightPanelOpen((v) => !v)}
            rightPanelOpen={rightPanelOpen}
          />
          {currentPage === 'market' ? (
            <MarketPage onBack={() => setCurrentPage('main')} />
          ) : currentPage === 'settings' ? (
            <SettingsPage onBack={() => setCurrentPage('main')} />
          ) : (
            <div className="flex-1 flex overflow-hidden relative">
              <SessionList />
              <MainPanel />
              <RightPanel isOpen={rightPanelOpen} />
            </div>
          )}
          <StatusBar onOpenSettings={() => setCurrentPage(p => p === "settings" ? "main" : "settings")} wsConnected={wsConnected} />
        </div>
      </div>

    </div>
  );
}

export default App;
