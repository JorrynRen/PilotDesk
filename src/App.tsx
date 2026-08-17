import { useState, useEffect } from 'react';
import { Routes, Route, useNavigate, useLocation } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from './stores/sessionStore';
import { useSkillStore } from './stores/skillStore';
import { useAgentEvent } from './hooks/useAgentEvent';
import { commandDispatcher } from './plugin/CommandDispatcher';
import { usePluginStore } from './stores/pluginStore';
import { pluginRegistry } from './plugin/PluginRegistry';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';
import { TerminalPanel } from './components/TerminalPanel';
import { CustomTabHost } from './components/custom/CustomTabHost';
import { useCustomTabsStore } from './stores/customTabsStore';
import { MarketPage } from './components/inspiration/MarketPage';
import { SettingsPage } from './pages/SettingsPage';
import { WorkflowPage } from './pages/WorkflowPage';
import { GroupChatPage } from './pages/GroupChatPage';
import { subscribeGroupChat } from './stores/groupChatStore';
import { WorkflowEditorPage } from './pages/WorkflowEditorPage';
import { TerminalProvider, useTerminal } from './TerminalManager';
import './styles/ui.css';

function MainLayout() {
  const [rightPanelOpen, setRightPanelOpen] = useState(true);
  const { viewMode, setMode } = useTerminal();
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

  const isTerminal = viewMode === 'terminal';
  const isSession = viewMode === 'session';
  const isWorkflow = viewMode === 'workflow';
  const isGroupChat = viewMode === 'groupchat';
  const isCustom = viewMode === 'custom';

  // 启动时加载自定义标签配置
  useEffect(() => {
    useCustomTabsStore.getState().load();
  }, []);

  return (
    <div className="pilotdesk-window-shell">
      <div className="pilotdesk-window-content flex flex-col h-full">
        <TitleBar
          mode={viewMode}
          onModeChange={setMode}
          onOpenSettings={() => navigate('/settings')}
          onToggleRightPanel={() => setRightPanelOpen((v) => !v)}
          rightPanelOpen={rightPanelOpen}
        />
        <div className="flex-1 flex overflow-hidden relative">
          {/* 群聊模式：原型页面（独立路由 /groupchat 复用同一页面），全宽无右侧面板 */}
          {isGroupChat && (
            <div className="flex-1 flex flex-col overflow-hidden">
              <GroupChatPage />
            </div>
          )}
          {/* 工作流模式：嵌入主布局（复用工作流管理页，去除自身 TitleBar/StatusBar），全宽 */}
          {isWorkflow && (
            <div className="flex-1 flex flex-col overflow-hidden">
              <WorkflowPage embedded />
            </div>
          )}
          {/* 自定义标签模式：固定壳（TitleBar/StatusBar），内容区渲染标签 iframe */}
          {isCustom && (
            <div className="flex-1 flex flex-col overflow-hidden">
              <CustomTabHost />
            </div>
          )}
          {/* 终端模式：中间终端 + 右侧面板（保留原始布局：会话列表隐藏） */}
          {isTerminal && (
            <>
              <div className="flex-1 flex flex-col overflow-hidden">
                <TerminalPanel />
              </div>
              <RightPanel isOpen={rightPanelOpen} />
            </>
          )}
          {/* 会话模式（默认）：三栏布局 */}
          {isSession && (
            <>
              <SessionList style={undefined} />
              <MainPanel style={undefined} />
              <RightPanel isOpen={rightPanelOpen} />
            </>
          )}
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

  // 应用启动时自动发现并加载插件（确保工作流节点类型可用）
  useEffect(() => {
    (async () => {
      try {
        await usePluginStore.getState().discover();
        await pluginRegistry.loadAllPlugins(usePluginStore.getState().plugins);
      } catch (err) {
        console.warn('[App] 插件发现失败:', err);
      }
    })();
  }, []);

  // 注册群聊事件单通道监听（全局单例，app 生命周期内常驻，不卸载）
  useEffect(() => {
    subscribeGroupChat();
  }, []);

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
    <TerminalProvider>
      <Routes>
        <Route path="/" element={<MainLayout />} />
        <Route path="/market" element={<MarketPage onBack={() => window.history.back()} />} />
        {/* 工作流/群聊：不再独立全屏路由，挂载后切到对应模式并回到主布局（用户感知为开关滑动） */}
        <Route path="/workflow" element={<ModeRedirect mode="workflow" />} />
        <Route path="/groupchat" element={<ModeRedirect mode="groupchat" />} />
        <Route path="/workflow/editor" element={<WorkflowEditorPage />} />
        <Route path="/settings" element={<SettingsPage onBack={() => window.history.back()} />} />
      </Routes>
    </TerminalProvider>
  );
}

/** 兼容旧路由：挂载时切换到指定模式并跳回主布局 */
function ModeRedirect({ mode }: { mode: 'workflow' | 'groupchat' }) {
  const { setMode } = useTerminal();
  const navigate = useNavigate();
  useEffect(() => {
    setMode(mode);
    navigate('/', { replace: true });
  }, [mode, setMode, navigate]);
  return null;
}

export default App;
