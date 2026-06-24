import { useState, useEffect } from 'react';
import { Routes, Route, useNavigate, useLocation } from 'react-router-dom';
import { useSessionStore } from './stores/sessionStore';
import { useSkillStore } from './stores/skillStore';
import { useAgentEvent } from './hooks/useAgentEvent';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';
import { MarketPage } from './components/inspiration/MarketPage';
import { SettingsPage } from './pages/SettingsPage';
import { WorkflowPage } from './pages/WorkflowPage';
import { WorkflowEditorPage } from './pages/WorkflowEditorPage';
import './styles/ui.css';

function MainLayout() {
  const [rightPanelOpen, setRightPanelOpen] = useState(true);
  const navigate = useNavigate();
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  // Agent Event status monitoring (replaces WebSocket)
  useAgentEvent({
    onSkills: (agentType, skills) => {
      useSkillStore.getState().setAgentSkills(agentType, skills);
    },
  });

  return (
    <div className="pilotdesk-window-shell">
      <div className="pilotdesk-window-content flex flex-col h-full">
          <TitleBar
            onOpenSettings={() => navigate('/settings')}
            onOpenWorkflow={() => navigate('/workflow')}
            onToggleRightPanel={() => setRightPanelOpen((v) => !v)}
            rightPanelOpen={rightPanelOpen}
          />
          <div className="flex-1 flex overflow-hidden relative">
            <SessionList />
            <MainPanel />
            <RightPanel isOpen={rightPanelOpen} />
          </div>
          <StatusBar
            onOpenSettings={() => navigate('/settings')}
            onOpenEnvSettings={() => navigate('/settings?tab=environment')}
          />
        </div>
      </div>
  );
}

function App() {
  const location = useLocation();

  // Update window title based on current route
  useEffect(() => {
    const base = 'PilotDesk';
    const path = location.pathname;
    if (path === '/market') {
      document.title = `${base} - 灵感市集`;
    } else if (path === '/workflow/editor') {
      document.title = `${base} - 工作流编辑器`;
    } else if (path === '/workflow') {
      document.title = `${base} - 工作流管理`;
    } else if (path === '/settings') {
      document.title = `${base} - 设置`;
    } else {
      const currentSession = useSessionStore.getState().currentSessionId;
      const session = useSessionStore.getState().sessions.find((s) => s.id === currentSession);
      document.title = session ? `${base} - ${session.title}` : base;
    }
  }, [location]);

  return (
    <Routes>
      <Route path="/" element={<MainLayout />} />
      <Route path="/market" element={<MarketPage onBack={() => window.history.back()} />} />
      <Route path="/workflow" element={<WorkflowPage onBack={() => window.history.back()} />} />
      <Route path="/workflow/editor" element={<WorkflowEditorPage />} />
      <Route path="/settings" element={<SettingsPage onBack={() => window.history.back()} />} />
    </Routes>
  );
}

export default App;
