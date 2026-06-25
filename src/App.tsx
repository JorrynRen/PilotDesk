import { useState, useEffect } from 'react';
import { Routes, Route, useNavigate, useLocation } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from './stores/sessionStore';
import { useSkillStore } from './stores/skillStore';
import { useAgentEvent } from './hooks/useAgentEvent';
import { commandDispatcher } from './plugin/CommandDispatcher';
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

  // 监听后端插件节点执行请求：后端 emit workflow:plugin-execute，
  // 前端通过 commandDispatcher 调用插件注册的命令 handler，再回传结果。
  // 插件命令 handler 注册在前端 JS 运行时，后端无法直接调用，故采用事件回传机制。
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    listen<{
      execution_id: string;
      node_id: string;
      plugin_id: string;
      command_id: string;
      params: any;
      timeout_seconds: number;
    }>('workflow:plugin-execute', async (event) => {
      const { execution_id, node_id, plugin_id, command_id, params, timeout_seconds } = event.payload;
      try {
        const cmdResult = await commandDispatcher.execute(plugin_id, command_id, params, {
          timeout: (timeout_seconds ?? 30) * 1000,
        });
        await invoke('respond_plugin_execute', {
          executionId: execution_id,
          nodeId: node_id,
          result: {
            success: cmdResult.success,
            data: cmdResult.data ?? null,
            error: cmdResult.error ?? null,
          },
        });
      } catch (err) {
        await invoke('respond_plugin_execute', {
          executionId: execution_id,
          nodeId: node_id,
          result: {
            success: false,
            data: null,
            error: String(err),
          },
        });
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

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
