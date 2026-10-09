import { useState, useEffect } from 'react';
import { Routes, Route, useNavigate, useLocation } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from './stores/sessionStore';
import { useAccountStore } from './stores/accountStore';
import { useSkillStore } from './stores/skillStore';
import { useAgentEvent } from './hooks/useAgentEvent';
import { commandDispatcher } from './plugin/CommandDispatcher';
import { usePluginStore, applyCommandParamDefaults } from './stores/pluginStore';
import { errorMessage } from './utils/errorMessage';
import { pluginRegistry } from './plugin/PluginRegistry';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar, NotificationCenter, CommandCenter } from './components/layout';
import { TerminalPanel } from './components/TerminalPanel';
import { CustomTabHost } from './components/custom/CustomTabHost';
import { useCustomTabsStore } from './stores/customTabsStore';
import { InspirationLibraryPage } from './components/inspiration/InspirationLibraryPage';
import { ResourceMarketPage } from './components/market/ResourceMarketPage';
import { SettingsPage } from './pages/SettingsPage';
import { WorkflowPage } from './pages/WorkflowPage';
import { GroupChatPage } from './pages/GroupChatPage';
import { KnowledgePage } from './pages/KnowledgePage';
import { subscribeGroupChat, useGroupChatStore } from './stores/groupChatStore';
import { subscribeNotifications } from './stores/notificationEvents';
import { WorkflowEditorPage } from './pages/WorkflowEditorPage';
import { TerminalProvider } from './TerminalProvider';
import { useTerminal } from './TerminalManager';
import { ImagePreview } from './components/message/ImagePreview';
import { ConfirmDialog } from './components/common/ConfirmDialog';
import './styles/ui.css';

function MainLayout() {
  // 各模式各自的右侧面板开合状态（会话/终端用 RightPanel；群聊用其页内专属右侧面板；
  // 工作流/自定义无右栏，不显示折叠按钮）
  const [sidePanelOpen, setSidePanelOpen] = useState<{ session: boolean; terminal: boolean; groupchat: boolean }>({
    // 会话右栏默认收起（把横向空间还给对话本身），需要时用顶栏按钮展开
    session: false,
    terminal: true,
    groupchat: true,
  });
  const { viewMode, setMode } = useTerminal();
  const rightPanelMode =
    viewMode === 'session' || viewMode === 'terminal' || viewMode === 'groupchat' ? viewMode : null;
  const rightPanelOpen = rightPanelMode ? sidePanelOpen[rightPanelMode] : false;
  const toggleRightPanel = () => {
    if (!rightPanelMode) return;
    setSidePanelOpen((prev) => ({ ...prev, [rightPanelMode]: !prev[rightPanelMode] }));
  };
  const navigate = useNavigate();

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

  // 启动时加载门户标签配置
  useEffect(() => {
    useCustomTabsStore.getState().load();
  }, []);

  /**
   * 离开「会话 / 群聊」模式即清空各自列表的选中项：两者每次进入都是「未选中任何条目」的干净状态，
   * 由用户自行点选。
   *
   * 之所以在"离开时"清、而不是"进入时"清：指挥中心 / 通知中心 / 会话转群聊都是「先选中再 setMode」，
   * 进入时清空会把这些显式跳转的落点一起抹掉；离开时清则只清残留（否则上次选中的条目会在下次进入时
   * 仍高亮，看起来像"默认选中"）。仅清本地选中，不发后端请求、不动任何数据。
   */
  useEffect(() => {
    if (viewMode !== 'session') {
      const { currentSessionId, startNewSession } = useSessionStore.getState();
      if (currentSessionId) startNewSession();
    }
    if (viewMode !== 'groupchat') {
      const { currentRoomId, selectRoom } = useGroupChatStore.getState();
      if (currentRoomId) void selectRoom('');
    }
  }, [viewMode]);

  return (
    <div className="pilotdesk-window-shell">
      <div className="pilotdesk-window-content flex flex-col h-full">
        <TitleBar
          mode={viewMode}
          onModeChange={setMode}
          onOpenSettings={() => navigate('/settings')}
          onOpenKnowledge={() => navigate('/knowledge')}
          onOpenMarket={() => navigate('/market')}
          onToggleRightPanel={rightPanelMode ? toggleRightPanel : undefined}
          rightPanelOpen={rightPanelMode ? rightPanelOpen : undefined}
        />
        {/* 工作区：这一层的底色就是壳层的"底"（--bg-canvas，见 globals.css）。
            只留左右各 8px 与面板之间 8px —— **上下不加内边距**：顶栏/状态栏自身高度已经足够，
            再留 8px 只会把三栏压矮，且上边那条缝会被读成"又一条横带"。 */}
        <div className="flex-1 flex overflow-hidden relative px-2 gap-2">
          {/* 群聊模式：本页自带左/中/右三栏，所以这一层**不当面板**——留透明露出壳层的"底"，
              由页面内部的三栏各自圆角成面板（与其它模式观感一致）。 */}
          {isGroupChat && (
            <div className="flex-1 flex flex-col overflow-hidden">
              <GroupChatPage rightPanelOpen={sidePanelOpen.groupchat} />
            </div>
          )}
          {/* 工作流模式：嵌入主布局（复用工作流管理页，去除自身 TitleBar/StatusBar），全宽 */}
          {isWorkflow && (
            <div className="flex-1 flex flex-col overflow-hidden rounded-lg" style={{ backgroundColor: 'var(--bg-content)' }}>
              <WorkflowPage embedded />
            </div>
          )}
          {/* 门户标签模式：固定壳。CustomTabHost 常挂载（CSS 隐藏切换），
              避免每次进出卸载导致已打开标签页的 iframe 状态丢失 */}
          <div className={`flex-1 flex flex-col overflow-hidden rounded-lg ${isCustom ? '' : 'hidden'}`} style={{ backgroundColor: 'var(--bg-content)' }}>
            <CustomTabHost />
          </div>
          {/* 终端模式：中间终端 + 右侧面板（保留原始布局：会话列表隐藏）。
              TerminalPanel 必须常挂载——xterm 会话 DOM/内容由组件实例持有，
              卸载即丢失；非终端模式仅用 display:none 隐藏，切回时内容原样保留。 */}
          <div className={`flex-1 flex flex-col overflow-hidden rounded-lg ${isTerminal ? '' : 'hidden'}`} style={{ backgroundColor: 'var(--bg-content)' }}>
            <TerminalPanel />
          </div>
          {isTerminal && <RightPanel isOpen={rightPanelOpen} mode="terminal" />}
          {/* 会话模式（默认）：三栏布局 */}
          {isSession && (
            <>
              <SessionList style={undefined} />
              <MainPanel style={undefined} />
              <RightPanel isOpen={rightPanelOpen} mode="session" />
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

  // 启动时拉一次会员登录状态与已解锁能力（受限入口按它显隐；平台不可达则按未登录处理）
  useEffect(() => {
    void useAccountStore.getState().refresh();
  }, []);

  // 回到本窗口时刷新权益：升级页（独立窗口）里 0 元下单后，切回来就能看到解锁
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        unlisten = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
          if (focused) void useAccountStore.getState().refresh();
        });
      } catch {
        /* 非 Tauri 环境（纯浏览器调试）忽略 */
      }
    })();
    return () => unlisten?.();
  }, []);

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

  // 注册通知中心的事件来源（工作流待人工输入、后台执行失败；全局单例）
  useEffect(() => {
    subscribeNotifications();
  }, []);

  // 说明：原「空状态自动弹出指挥中心」的逻辑已移除。无选中会话时，
  // 会话默认页（MainPanel）现在内嵌了指挥中心内容（variant="inline"），
  // 不需要再额外弹一个模态盖在它上面。

  // Update window title based on current route
  useEffect(() => {
    const base = 'PilotDesk';
    const path = location.pathname;
    // 灵感库：本地保存的灵感/提示词（原「灵感市集」改名并让出 /market）
    if (path === '/inspirations') {
      document.title = `${base} - 灵感库`;
    } else if (path === '/market') {
      document.title = `${base} - 资源市集`;
    } else if (path === '/workflow/editor') {
      document.title = `${base} - 工作流编辑器`;
    } else if (path === '/workflow') {
      document.title = `${base} - 工作流管理`;
    } else if (path === '/settings') {
      document.title = `${base} - 设置`;
    } else if (path === '/knowledge') {
      document.title = `${base} - 知识库`;
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
      params: Record<string, unknown>;
      timeout_seconds: number;
    }>('workflow:plugin-execute', async (event) => {
      const { execution_id, node_id, plugin_id, command_id, params, timeout_seconds } = event.payload;
      try {
        // 用命令声明的 default 兜底：插件表单可能只把默认值显示出来而从没写进节点参数，
        // 直接执行会让 handler 收到空值/别的命令的取值（表现为"选默认项报错、换一个选项就正常"）
        const commandParams = applyCommandParamDefaults(plugin_id, command_id, params);
        const cmdResult = await commandDispatcher.execute(plugin_id, command_id, commandParams, {
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
            error: errorMessage(err),
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
      {/* 全局图片放大预览（会话/群聊/工作流等所有页面共用） */}
      <ImagePreview />
      {/* 全局确认弹窗（替代 window.confirm：Tauri WebView 下后者常不触发） */}
      <ConfirmDialog />
      {/* 全局通知中心（顶栏铃铛触发；App 级挂载，所有模式/页面都能打开） */}
      <NotificationCenter />
      {/* 全局指挥中心模态（工作流页「待处理」触发；会话默认页另有一份内嵌内容） */}
      <CommandCenter />
      <Routes>
        <Route path="/" element={<MainLayout />} />
        {/* 灵感库：本地灵感/提示词（/market 已让给「资源市集」） */}
        <Route path="/inspirations" element={<InspirationLibraryPage onBack={() => window.history.back()} />} />
        {/* 资源市集：插件 / 工作流模板 / CLI Agent 配置 / 灵感 的统一获取入口 */}
        <Route path="/market" element={<ResourceMarketPage />} />
        {/* 工作流/群聊：不再独立全屏路由，挂载后切到对应模式并回到主布局（用户感知为开关滑动） */}
        <Route path="/workflow" element={<ModeRedirect mode="workflow" />} />
        <Route path="/groupchat" element={<ModeRedirect mode="groupchat" />} />
        <Route path="/workflow/editor" element={<WorkflowEditorPage />} />
        <Route path="/settings" element={<SettingsPage onBack={() => window.history.back()} />} />
        {/* 知识库：独立路由（顶部组合菜单的「知识库」段进入） */}
        <Route path="/knowledge" element={<KnowledgePage />} />
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
