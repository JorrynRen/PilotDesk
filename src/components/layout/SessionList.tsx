import { useState, useEffect, useRef, useCallback, memo, type ReactNode } from 'react';
import { Plus, Search, Archive, Key, X, FolderOpen, ChevronDown, ChevronRight, Terminal } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from '../../stores/sessionStore';
import { useApiProviderStore, getApiKey } from '../../stores/apiProviderStore';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { isWorkflowSession } from '../../utils/sessionType';
import { errorMessage } from '../../utils/errorMessage';
import { useI18n } from '../../hooks/useI18n';
import { SessionListItem } from './SessionListItem';
import { Select } from '../common/Select';

type NewSessionType = string;

/** 「工作流会话」折叠分组的展开状态（纯 UI 状态，存 localStorage 即可，不入库）。 */
const WORKFLOW_GROUP_STORAGE_KEY = 'pilotdesk.session-list.workflow-expanded';

function SessionListFn({ style }: { style?: React.CSSProperties } = {}) {
  const { t } = useI18n();
  const sessions = useSessionStore((s) => s.sessions);
  const archivedSessions = useSessionStore((s) => s.archivedSessions);
  const currentSessionId = useSessionStore((s) => s.currentSessionId);
  const isLoadingSessions = useSessionStore((s) => s.isLoadingSessions);
  const showArchived = useSessionStore((s) => s.showArchived);
  const fetchSessions = useSessionStore((s) => s.fetchSessions);
  const refreshSessions = useSessionStore((s) => s.refreshSessions);
  const selectSession = useSessionStore((s) => s.selectSession);
  const createSession = useSessionStore((s) => s.createSession);
  const archiveSession = useSessionStore((s) => s.archiveSession);
  const unarchiveSession = useSessionStore((s) => s.unarchiveSession);
  const deleteSession = useSessionStore((s) => s.deleteSession);
  const renameSession = useSessionStore((s) => s.renameSession);
  const toggleArchived = useSessionStore((s) => s.toggleArchived);

  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<typeof sessions>([]);
  const [, setIsSearching] = useState(false);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [installedAgents, setInstalledAgents] = useState<Set<string>>(new Set());
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  // 「工作流会话」折叠分组：默认折叠，展开状态跨重启记忆
  const [workflowExpanded, setWorkflowExpanded] = useState(() => {
    try { return localStorage.getItem(WORKFLOW_GROUP_STORAGE_KEY) === '1'; } catch { return false; }
  });
  const toggleWorkflowGroup = useCallback(() => {
    setWorkflowExpanded((prev) => {
      const next = !prev;
      try { localStorage.setItem(WORKFLOW_GROUP_STORAGE_KEY, next ? '1' : '0'); } catch { /* ignore */ }
      return next;
    });
  }, []);
  const [showNewDialog, setShowNewDialog] = useState(false);
    const [newSessionType, setNewSessionType] = useState<NewSessionType>('');
  const [selectedApiProvider, setSelectedApiProvider] = useState('');
  const [selectedApiModel, setSelectedApiModel] = useState('');
  const [customModel, setCustomModel] = useState('');
  const [useCustomModel, setUseCustomModel] = useState(false);
  const [customTitle, setCustomTitle] = useState('');
  const [customCwd, setCustomCwd] = useState('');
  const [sessionTemperature, setSessionTemperature] = useState(0.7);
  const [sessionMaxTokens, setSessionMaxTokens] = useState<number | undefined>(undefined);
  const [creating, setCreating] = useState(false);

  const { getDisplayName, getEnabledAgentTypes } = useAgentRegistry();

  // WebSocket for Agent session lifecycle
  const { createAgentSession: wsCreateSession, closeAgentSession: wsCloseSession } = useAgentEvent();

  // API providers from SQLite via store
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();

  // 初始加载：只拉取会话列表与提供商。
  // **不自动选中任何会话**：会话模式默认停在初始页（"输入消息开始新会话…"），
  // 选中哪个会话由用户点选决定；同理也不再恢复"上次打开的会话"。
  useEffect(() => {
    const init = async () => {
      await fetchSessions();
      fetchProviders().catch(() => {});
    };
    init().catch((err) => {
      showToast(`加载会话失败: ${errorMessage(err)}`, 'error');
    });
  }, [fetchSessions, fetchProviders]);

  /**
   * 工作流 Agent 会话不会自己出现在列表里：它们由后端在节点执行时自动写库
   * （`create_session_inner`，origin = `workflow:*`），前端内存里没有，只有重挂组件才会重新拉 ——
   * 表现为"来自工作流的会话记录要刷新才看得到"。
   *
   * 这类会话**只有工作流这一个来源**，所以借引擎已在发的 `workflow:execution-progress` 把列表拉新即可，
   * 不必新增后端接口（订阅的是既有事件，事件名与 notificationEvents.ts / 指挥中心保持一致）。
   *
   * 两个细节：
   * - 用 `refreshSessions()`（静默刷新）而不是 `fetchSessions()`：前者不动 isLoadingSessions，
   *   不会让列表闪一下加载态、也不打断当前选中。
   * - 进度事件按节点成簇下发，做 2s 合并 —— 会话列表查询随会话数增长，不该按事件频率打。
   */
  useEffect(() => {
    let disposed = false;
    let timer: number | null = null;
    let off: UnlistenFn | null = null;
    void listen('workflow:execution-progress', () => {
      if (timer !== null) return; // 已在合并窗口内，等这一轮跑完
      timer = window.setTimeout(() => {
        timer = null;
        void refreshSessions();
      }, 2000);
    }).then((fn) => {
      // 卸载与 listen() 的 Promise 存在竞态：cleanup 先跑时 off 还是 null，用 disposed 兜底
      if (disposed) fn();
      else off = fn;
    });
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
      if (off) off();
    };
  }, [refreshSessions]);

  // Sync installed agents from envInfo (shared singleton, no extra detect_env call)。
  // 用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮级联渲染
  // （`react-hooks/set-state-in-effect`），两者行为一致。
  const { envInfo } = useEnvInfo();
  /**
   * ⚠️ 前值哨兵的初值必须是 `null`，**不能**写成 `useState(envInfo)`。
   * `envInfo` 是跨页共享的单例：本组件挂载时它很可能已经有值。若初值就等于当帧值，
   * 第一帧"前值 === 当前值"、判据不成立，这段同步**永远不会执行** ——
   * 表现为 `installedAgents` 恒为空 → CLI Agent 类型被全部过滤掉 → 新建会话只剩 API 可选。
   * （原 `useEffect(..., [envInfo])` 在挂载时也会跑一次，这是"变化即同步"的改写必须补回的那一次。）
   */
  const [syncedEnvInfo, setSyncedEnvInfo] = useState<typeof envInfo | null>(null);
  if (syncedEnvInfo !== envInfo) {
    setSyncedEnvInfo(envInfo);
    if (envInfo?.agentVersions) {
      const installed = new Set<string>();
      for (const [agentType, version] of Object.entries(envInfo.agentVersions)) {
        if (version) installed.add(agentType);
      }
      setInstalledAgents(installed);
    }
  }

  /**
   * 打开弹窗时把"会话方式"归一到确实可用的选项（默认 API 优先、其次第一个已装 CLI）：
   * 否则默认值可能指向未安装的 CLI —— 按钮不亮、下拉空白，用户点创建才发现不可用。
   * 用「渲染期修正」而不是 effect：不可用时立刻改写状态，收敛后即不再触发，与 effect 等价。
   */
  if (showNewDialog) {
    const cliTypes = getEnabledAgentTypes().filter((t) => t !== 'api' && installedAgents.has(t));
    const usable = newSessionType === 'api' ? apiProviders.length > 0 : cliTypes.includes(newSessionType);
    if (!usable) {
      if (apiProviders.length > 0) setNewSessionType('api');
      else if (cliTypes.length > 0) setNewSessionType(cliTypes[0]);
    }
  }

  // Load workspace setting from SQLite when dialog opens
  useEffect(() => {
    if (showNewDialog) {
      invoke<string | null>('get_app_setting', { key: 'pilotdesk-workspace' })
        .then((val) => {
          if (val) {
            setCustomCwd(val);
          }
        })
        .catch(() => {});
    }
  }, [showNewDialog]);

  // 清空搜索词时立刻清掉结果。用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），两者行为一致。
  const [prevSearchQuery, setPrevSearchQuery] = useState(searchQuery);
  if (prevSearchQuery !== searchQuery) {
    setPrevSearchQuery(searchQuery);
    if (!searchQuery.trim()) {
      setSearchResults([]);
      setIsSearching(false);
    }
  }

  // Debounced session search
  useEffect(() => {
    if (!searchQuery.trim()) return;
    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    searchDebounceRef.current = setTimeout(async () => {
      setIsSearching(true);
      try {
        const results = await invoke<typeof sessions>('search_sessions', { query: searchQuery.trim() });

        setSearchResults(results);
      } catch { /* ignore */ }
      setIsSearching(false);
    }, 300);
    return () => {
      if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    };
  }, [searchQuery]);

  // Batch operations
  const toggleBatchSelect = useCallback((id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  }, []);

  const displayList = showArchived ? archivedSessions : sessions;
  const isSearchActive = searchQuery.trim().length > 0;
  const filteredList = isSearchActive
    ? searchResults
    : displayList.filter((s) =>
        s.title.toLowerCase().includes(searchQuery.toLowerCase())
      );

  // 工作流会话（origin 前缀 `workflow`）是节点每次执行自动创建的内部会话：数量多、同名，
  // 混在时间组里会淹没用户自己的会话，因此收进列表底部一个折叠分组。
  // 只在活动列表、非搜索时折叠——搜索要"搜得到就能看见"，归档页也照常平铺。
  const foldsWorkflow = !showArchived && !isSearchActive;
  const visibleList = foldsWorkflow ? filteredList.filter((s) => !isWorkflowSession(s.origin)) : filteredList;
  const workflowList = foldsWorkflow ? filteredList.filter((s) => isWorkflowSession(s.origin)) : [];

  // Group by time。当前时间只在挂载时取一次（Date.now() 是渲染期不纯调用，react-hooks/purity
  // 不允许在渲染体里直接调用）；分组粒度是天，跨天重开即可，渲染语义不受影响。
  const [now] = useState(() => Date.now() / 1000);
  const todayStart = Math.floor(now / 86400) * 86400;
  const yesterdayStart = todayStart - 86400;

  const today = visibleList.filter((s) => s.updatedAt >= todayStart);
  const yesterday = visibleList.filter(
    (s) => s.updatedAt >= yesterdayStart && s.updatedAt < todayStart
  );
  const earlier = visibleList.filter((s) => s.updatedAt < yesterdayStart);



  const selectAll = useCallback(() => {
    // 全选 = 当前可见项：折叠着的工作流会话不计入（与"看不见就不选"一致）
    const ids = new Set(visibleList.map((s) => s.id));
    setSelectedIds(ids);
  }, [visibleList]);

  const deselectAll = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  const batchArchive = useCallback(async () => {
    if (selectedIds.size === 0) return;
    for (const id of selectedIds) {
      try { await invoke('archive_session', { sessionId: id }); } catch { /* ignore */ }
    }
    setSelectedIds(new Set());
    setBatchMode(false);
    fetchSessions();
    showToast(`已归档 ${selectedIds.size} 个会话`, 'success');
  }, [selectedIds, fetchSessions]);

  const batchUnarchive = useCallback(async () => {
    if (selectedIds.size === 0) return;
    for (const id of selectedIds) {
      try { await invoke('unarchive_session', { sessionId: id }); } catch { /* ignore */ }
    }
    setSelectedIds(new Set());
    setBatchMode(false);
    fetchSessions();
    showToast(`已取消归档 ${selectedIds.size} 个会话`, 'success');
  }, [selectedIds, fetchSessions]);

  const batchDelete = useCallback(async () => {
    if (selectedIds.size === 0) return;
    // 二次确认：删除会话会连带清理其用量记录（含工作流节点自动创建的内部会话），不可恢复。
    const confirmed = await confirmDialog({
      title: '确认删除',
      message: `确定删除选中的 ${selectedIds.size} 个会话？此操作不可撤销，其消息与用量记录会一并清理。`,
      confirmText: '删除',
    });
    if (!confirmed) return;
    for (const id of selectedIds) {
      try { await invoke('delete_session', { sessionId: id }); } catch { /* ignore */ }
    }
    setSelectedIds(new Set());
    setBatchMode(false);
    fetchSessions();
    showToast(`已删除 ${selectedIds.size} 个会话`, 'success');
  }, [selectedIds, fetchSessions]);

  // 打开弹窗时重置表单状态。用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），两者行为一致。
  const [prevShowNewDialog, setPrevShowNewDialog] = useState(showNewDialog);
  if (prevShowNewDialog !== showNewDialog) {
    setPrevShowNewDialog(showNewDialog);
    if (showNewDialog) {
      // Reset state
      setUseCustomModel(false);
      setCustomModel('');
      setCustomTitle('');
      setCustomCwd('');
      setSessionTemperature(0.7);
      setSessionMaxTokens(undefined);

      // Auto-select first enabled agent
      const enabled = getEnabledAgentTypes().filter(t => t !== 'api');
      if (enabled.length > 0 && !newSessionType) {
        setNewSessionType(enabled[0]);
      }
    }
  }

  // Reload providers when dialog opens
  useEffect(() => {
    if (showNewDialog) {
      fetchProviders().then(() => {
        // After fetch, auto-select first provider via the updated store state
      }).catch(() => {});
    }
  }, [showNewDialog, fetchProviders]);

  // 提供商变化时同步模型（无选中时选第一个）。用「渲染期修正」而不是 effect：
  // 在 effect 里同步 setState 会多一轮级联渲染（`react-hooks/set-state-in-effect`），两者行为一致。
  const [prevProviderDeps, setPrevProviderDeps] = useState<[string, typeof apiProviders]>([selectedApiProvider, apiProviders]);
  if (prevProviderDeps[0] !== selectedApiProvider || prevProviderDeps[1] !== apiProviders) {
    setPrevProviderDeps([selectedApiProvider, apiProviders]);
    if (apiProviders.length > 0 && !selectedApiProvider) {
      setSelectedApiProvider(apiProviders[0].id);
    }
    const provider = apiProviders.find((p) => p.id === selectedApiProvider);
    if (provider) {
      setSelectedApiModel(provider.models[0] || '');
      setUseCustomModel(false);
      setCustomModel('');
    }
  }

  const validateAndEnsureDir = async (dir: string): Promise<string | null> => {
    // 空值：回落到全局工作区设置；没有则交给后端默认目录
    if (!dir.trim()) {
      try {
        const globalDir = await invoke<string | null>('get_app_setting', { key: 'pilotdesk-workspace' });
        if (globalDir) {
          return await invoke<string>('ensure_dir', { path: globalDir });
        }
        return null; // No global setting either, use default
      } catch {
        return null;
      }
    }

    /**
     * 非空：**校验全部交给后端的 `ensure_dir`**，前端不再重复实现一遍。
     *
     * 这里原先有一份重复的校验（非法字符 / 纯盘符），而后端 `ensure_dir` 早就把
     * 空值、盘符、非法字符、是否存在、是否目录、以及按需创建全都做完了，且文案更准确。
     * 那份重复实现唯一的"贡献"是一个 bug：提示里写的是「如 C:\Users\work」，
     * 但 `\U`、`\w` 在 JS 里是无效转义，实际渲染成「C:Userswork」。
     * 删掉重复实现后，用户看到的是后端那条转义正确的提示（前缀带「目录校验失败: 」以说明上下文）。
     */
    try {
      return await invoke<string>('ensure_dir', { path: dir.trim() });
    } catch (err) {
      showToast(`目录校验失败: ${errorMessage(err)}`, 'error');
      return null;
    }
  };

  const handleCreate = async () => {
    setCreating(true);
    try {
      // Validate and resolve working directory
      const resolvedCwd = await validateAndEnsureDir(customCwd);
      if (resolvedCwd === null && customCwd.trim()) {
        // User entered a path but validation failed
        setCreating(false);
        return;
      }

      if (newSessionType === 'api') {
        // Check if API key is configured
        const apiKey = await getApiKey(selectedApiProvider);
        if (!apiKey) {
          showToast('请先在「设置 - API 配置」中添加 API Key', 'error');
          setCreating(false);
          return;
        }
        const model = useCustomModel ? customModel.trim() : selectedApiModel;
        if (!model) {
          showToast('请选择或输入模型名称', 'error');
          setCreating(false);
          return;
        }
        // Create API direct session
        const session = await createSession(
          'api' as const,
          resolvedCwd || undefined,
          customTitle.trim() || null,
          selectedApiProvider,
          model,
          sessionTemperature,
          sessionMaxTokens,
        );
        setShowNewDialog(false);
        selectSession(session.id);
      } else {
        const session = await createSession(
          newSessionType,
          resolvedCwd || undefined,
          customTitle.trim() || null,
        );
        // Notify Sidecar to create the Agent session
        wsCreateSession(session.id, newSessionType, resolvedCwd || undefined);
        setShowNewDialog(false);
        selectSession(session.id);
      }
    } catch (err) {
      showToast(`创建会话失败: ${errorMessage(err)}`, 'error');
    } finally {
      setCreating(false);
    }
  };

  const handleArchive = useCallback(async (id: string) => {
    try {
      // Notify Sidecar to close the Agent session
      const session = sessions.find(s => s.id === id);
      if (session && session.agentType !== 'api') {
        wsCloseSession(id, session.agentType);
      }
      await archiveSession(id);
    } catch (err) {
      showToast(`归档失败: ${errorMessage(err)}`, 'error');
    }
  }, [sessions, wsCloseSession, archiveSession]);

  // 与 handleArchive 对称：归档时会关掉 Sidecar 会话，取消归档不需要反向操作（会话本就已停）。
  const handleUnarchive = useCallback(async (id: string) => {
    try {
      await unarchiveSession(id);
    } catch (err) {
      showToast(`取消归档失败: ${errorMessage(err)}`, 'error');
    }
  }, [unarchiveSession]);

  const handleRename = async (id: string, newTitle: string) => {
    try {
      await renameSession(id, newTitle);
    } catch (err) {
      showToast(`重命名失败: ${errorMessage(err)}`, 'error');
    }
  };

  const handleDelete = useCallback(async (id: string) => {
    const session = sessions.find(s => s.id === id) || archivedSessions.find(s => s.id === id);
    // 删除会话会连带清理其用量记录（工作流节点会话亦在列表内），故二次确认
    const confirmed = await confirmDialog({
      title: '确认删除',
      message: `确定删除会话「${session?.title || id}」？此操作不可撤销，其消息与用量记录会一并清理。`,
      confirmText: '删除',
    });
    if (!confirmed) return;
    try {
      // Notify Sidecar to close the Agent session
      if (session && session.agentType !== 'api') {
        wsCloseSession(id, session.agentType);
      }
      await deleteSession(id);
      showToast(`已删除会话「${session?.title || id}」`, 'success');
    } catch (err) {
      showToast(`删除失败: ${errorMessage(err)}`, 'error');
    }
  }, [sessions, archivedSessions, wsCloseSession, deleteSession]);

  const handleListClick = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    const row = target.closest<HTMLElement>('[data-session-id]');
    if (!row) return;
    const sessionId = row.dataset.sessionId!;
    const action = target.closest<HTMLElement>('[data-action]')?.dataset.action;

    switch (action) {
      case 'archive':
        handleArchive(sessionId);
        break;
      case 'unarchive':
        handleUnarchive(sessionId);
        break;
      case 'delete':
        handleDelete(sessionId);
        break;
      default:
        // 点击行本身 = 选中会话
        if (!batchMode) selectSession(sessionId);
        else toggleBatchSelect(sessionId);
    }
  }, [batchMode, selectSession, toggleBatchSelect, handleArchive, handleUnarchive, handleDelete]);

  const renderItem = (session: (typeof sessions)[number]) => (
    <SessionListItem
      key={session.id}
      session={session}
      isActive={session.id === currentSessionId}
      onRename={handleRename}
      batchMode={batchMode}
      selected={selectedIds.has(session.id)}
      onToggleSelect={toggleBatchSelect}
      archived={showArchived}
    />
  );

  const renderGroup = (label: string, items: typeof sessions) => {
    if (items.length === 0) return null;
    return (
      <div key={label}>
        <div className="px-3 py-1 text-[10px] " style={{ color: 'var(--text-secondary)' }}>
          {label}
        </div>
        {items.map(renderItem)}
      </div>
    );
  };

  const currentApiProvider = apiProviders.find((p) => p.id === selectedApiProvider);

  return (
    <aside
      className="w-[260px] shrink-0 flex flex-col overflow-hidden rounded-lg"
      // 壳层风格：面板靠"底色 + 圆角 + 与相邻面板之间的 8px 缝"区分，不再画 border-right（见 App.tsx 工作区）
      style={{ backgroundColor: 'var(--bg-side)', ...style }}
    >
      {/* Header */}
      <div className="flex items-center px-3 h-9" style={{ borderBottom: '1px solid var(--border)' }}>
        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>{t('sessionList.title', '会话')}</span>
        <div className="flex-1" />
        <div className="flex items-center gap-1">
          <button
            onClick={() => setBatchMode(!batchMode)}
            className="pd-btn pd-btn-sm"
            style={{
              backgroundColor: batchMode ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: batchMode ? '#fff' : 'var(--text-secondary)',
            }}
            title={t('sessionList.batch.title', '批量操作')}
          >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></svg>
            {t('sessionList.batch', '批量')}
          </button>
          <button
            onClick={toggleArchived}
            className="pd-btn pd-btn-sm"
            style={{ color: showArchived ? 'var(--accent)' : 'var(--text-secondary)', backgroundColor: showArchived ? 'var(--accent-light)' : 'var(--bg-tertiary)' }}
            title={t('sessionList.archive.title', '归档')}
          >
            <Archive size={11} />
            {t('sessionList.archive', '归档')}
          </button>
          <button
            onClick={() => setShowNewDialog(true)}
            className="pd-btn pd-btn-sm pd-btn-primary"
            title={t('sessionList.new.title', '新建会话')}
          >
            <Plus size={11} />
            {t('sessionList.new', '新建')}
          </button>
        </div>
      </div>

      {/* Batch operations row - above search */}
      {batchMode && (
        <div className="flex items-center gap-1 px-3 py-1.5" style={{ borderBottom: '1px solid var(--border)' }}>
          <button
            onClick={selectAll}
            className="pd-btn pd-btn-sm"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title={t('sessionList.selectAll.title', '全选')}
          >
            {t('sessionList.selectAll', '全')}
          </button>
          <button
            onClick={deselectAll}
            className="pd-btn pd-btn-sm"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title={t('sessionList.clearAll.title', '取消全选')}
          >
            {t('sessionList.clearAll', '重')}
          </button>
          <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
          <button
            onClick={showArchived ? batchUnarchive : batchArchive}
            className="pd-btn pd-btn-sm pd-btn-info"
            disabled={selectedIds.size === 0}
            title={showArchived ? t('sessionList.batchUnarchive.title', '批量取消归档') : t('sessionList.batchArchive.title', '批量归档')}
          >
            {showArchived
              ? t('sessionList.batchUnarchive', '消档({count})', { count: selectedIds.size })
              : t('sessionList.batchArchive', '档({count})', { count: selectedIds.size })}
          </button>
          <button
            onClick={batchDelete}
            className="pd-btn pd-btn-sm pd-btn-danger"
            disabled={selectedIds.size === 0}
            title={t('sessionList.batchDelete.title', '批量删除')}
          >
            {t('sessionList.batchDelete', '删({count})', { count: selectedIds.size })}
          </button>
          <div className="flex-1" />
          <button
            onClick={() => { setBatchMode(false); setSelectedIds(new Set()); }}
            className="pd-btn pd-btn-sm"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title={t('sessionList.exitBatch.title', '退出批量模式')}
          >
            {t('sessionList.exitBatch', '退')}
          </button>
        </div>
      )}
      {/* Search */}
      <div className="px-3 py-1.5">
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder={t('sessionList.search.placeholder', '搜索会话...')}
            className="search-input"
          />
        </div>
      </div>

      {/* Loading */}
      {isLoadingSessions ? (
        <div className="flex-1 flex items-center justify-center">
          <div className="pilotdesk-spinner" />
        </div>
      ) : (
        /* Session list */
        <div className="flex-1 overflow-y-auto pd-scroll-stable" onClick={handleListClick}>
          {filteredList.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full gap-2 px-4">
              <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                {showArchived ? t('sessionList.empty.archived', '暂无归档会话') : t('sessionList.empty', '暂无会话')}
              </p>
              {!showArchived && (
                <button
                  onClick={() => setShowNewDialog(true)}
                  className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors"
                  style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                >
                  <Plus size={12} />
                  {t('sessionList.new.title', '新建会话')}
                </button>
              )}
            </div>
          ) : (
            <>
              {/* 「工作流会话」放在最前（原先在列表末尾）：它是节点执行自动创建的内部会话，
                  数量随跑量增长，排在时间组之后时会被「更早」那一段一路冲到底 —— 入口要滚很久才够得到。
                  默认仍折叠，只占一行；下方补一条细分隔线，否则紧接着的「今天」看起来像它的子项。
                  分隔线只在下面确实还有内容时画，避免整列都是工作流会话时留一条悬空的线。 */}
              {workflowList.length > 0 && (
                <div key="workflow-sessions">
                  {/* 折叠头：点击展开/收起；与时间组标题同字号、加计数，避免看起来像分组标签 */}
                  <button
                    onClick={toggleWorkflowGroup}
                    className="w-full flex items-center gap-1 px-3 py-1 text-[10px] transition-colors"
                    style={{ color: 'var(--text-secondary)' }}
                    title={
                      workflowExpanded
                        ? t('sessionList.workflowGroup.collapse', '收起工作流会话')
                        : t('sessionList.workflowGroup.expand', `展开 ${workflowList.length} 个工作流会话（节点执行自动创建）`, { count: workflowList.length })
                    }
                  >
                    {workflowExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                    <span>{t('sessionList.workflowGroup', '工作流会话')}</span>
                    <span style={{ color: 'var(--text-tertiary)' }}>{workflowList.length}</span>
                  </button>
                  {workflowExpanded && workflowList.map(renderItem)}
                  {visibleList.length > 0 && (
                    <div className="mx-3 my-1" style={{ height: 1, backgroundColor: 'var(--border)' }} />
                  )}
                </div>
              )}
              {renderGroup(t('sessionList.group.today', '今天'), today)}
              {renderGroup(t('sessionList.group.yesterday', '昨天'), yesterday)}
              {renderGroup(t('sessionList.group.earlier', '更早'), earlier)}
            </>
          )}
        </div>
      )}

      {/* New session dialog */}
      {showNewDialog && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={(e) => { if (e.target === e.currentTarget) setShowNewDialog(false); }}
        >
          <div
            className="w-auto min-w-[420px] max-w-[560px] rounded-xl p-4"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
          >
            <h3 className="text-sm font-medium mb-3" style={{ color: 'var(--text-primary)' }}>{t('sessionList.dialog.title', '新建会话')}</h3>

            {/* 会话方式：一级「CLI Agent / API Agent」两个按钮 + 二级下拉（与 InputBar 快捷开始同构） */}
            <div className="mb-4">
              {(() => {
                // CLI：只列"已启用且本机已安装"的；API：需要有提供商
                const cliTypes = getEnabledAgentTypes().filter((t) => t !== 'api' && installedAgents.has(t));
                const hasApi = apiProviders.length > 0;
                if (cliTypes.length === 0 && !hasApi) {
                  return (
                    <div className="w-full px-3 py-3 rounded-lg text-xs text-center"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}>
                      {t('sessionList.dialog.noAgent', '暂无可用会话方式。请先在「设置」中配置 API 提供商或安装 CLI Agent。')}
                    </div>
                  );
                }
                const kind: 'cli' | 'api' = newSessionType === 'api' ? 'api' : 'cli';
                const kindBtn = (target: 'cli' | 'api', label: string, icon: ReactNode) => (
                  <button
                    key={target}
                    onClick={() => {
                      if (target === 'api') setNewSessionType('api');
                      else if (cliTypes.length > 0) setNewSessionType(cliTypes.includes(newSessionType) ? newSessionType : cliTypes[0]);
                    }}
                    className="flex-1 py-2 rounded-lg text-xs transition-colors flex items-center justify-center gap-1"
                    style={{
                      backgroundColor: kind === target ? 'var(--accent-light)' : 'var(--bg-secondary)',
                      color: kind === target ? 'var(--accent)' : 'var(--text-secondary)',
                      border: `1px solid ${kind === target ? 'var(--accent)' : 'var(--border)'}`,
                    }}
                  >
                    {icon}
                    {label}
                  </button>
                );
                return (
                  <>
                    <div className="flex gap-2">
                      {cliTypes.length > 0 && kindBtn('cli', 'CLI Agent', <Terminal size={12} />)}
                      {hasApi && kindBtn('api', 'API Agent', <Key size={12} />)}
                    </div>
                    {kind === 'cli' && cliTypes.length > 0 && (
                      <div className="mt-3">
                        <label className="block text-xs mb-1" style={{ color: 'var(--text-secondary)' }}>
                          {t('sessionList.dialog.cliLabel', '选择 CLI Agent')}
                        </label>
                        <Select
                          value={newSessionType}
                          onChange={setNewSessionType}
                          options={cliTypes.map((t) => ({ value: t, label: getDisplayName(t) }))}
                          className="w-full"
                        />
                      </div>
                    )}
                  </>
                );
              })()}
            </div>

            {/* Agent sessions: optional title & cwd */}
            {newSessionType !== 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    {t('sessionList.dialog.titleLabel', '会话标题（可选）')}
                  </label>
                  <input
                    type="text"
                    value={customTitle}
                    onChange={(e) => setCustomTitle(e.target.value)}
                    placeholder={t('sessionList.dialog.titlePlaceholder', `${getDisplayName(newSessionType)} 新会话`, { name: getDisplayName(newSessionType) })}
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    {t('sessionList.dialog.cwdLabel', '工作目录（可选）')}
                  </label>
                  <div className="flex gap-2">
                    <div
                      className="flex-1 px-3 py-2 rounded-lg text-sm truncate"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                      title={customCwd || t('sessionList.dialog.cwdPlaceholder', '留空使用默认目录')}
                    >
                      {customCwd || t('sessionList.dialog.cwdPlaceholder', '留空使用默认目录')}
                    </div>
                    <button
                      onClick={async () => {
                        try {
                          
                          const selected = await openDialog({ directory: true, multiple: false });
                          if (selected && typeof selected === 'string') {
                            setCustomCwd(selected);
                          }
                        } catch { /* ignore */ }
                      }}
                      className="px-2.5 py-2 rounded-lg shrink-0 transition-colors hover:opacity-80"
                      style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                      title={t('sessionList.dialog.chooseDir', '选择目录')}
                    >
                      <FolderOpen size={14} />
                    </button>
                  </div>
                </div>
              </div>
            )}

            {/* API direct: provider & model selection */}
            {newSessionType === 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    {t('sessionList.dialog.providerLabel', 'API 提供商')}
                  </label>
                  {apiProviders.length === 0 ? (
                    <div
                      className="px-3 py-2 rounded-lg text-xs"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
                    >
                      {t('sessionList.dialog.noProvider', '暂无配置的 API 提供商，请先在「设置 - API集成配置」中添加')}
                    </div>
                  ) : (
                    <Select
                      className="w-full"
                      value={selectedApiProvider}
                      onChange={setSelectedApiProvider}
                      options={apiProviders.map((p) => ({
                        value: p.id,
                        label: `${p.name}${p.apiKeySet ? ` ${t('sessionList.dialog.configured', '(已配置)')}` : ` ${t('sessionList.dialog.notConfigured', '(未配置Key)')}`}`,
                      }))}
                    />
                  )}
                </div>

                {currentApiProvider && (
                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-xs " style={{ color: 'var(--text-secondary)' }}>
                        {t('sessionList.dialog.modelLabel', '模型')}
                      </label>
                      <button
                        onClick={() => setUseCustomModel((v) => !v)}
                        className="flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded transition-colors"
                        style={{
                          color: useCustomModel ? 'var(--accent)' : 'var(--text-tertiary)',
                          backgroundColor: useCustomModel ? 'var(--bg-tertiary)' : 'transparent',
                        }}
                      >
                        {useCustomModel ? <X size={10} /> : null}
                        {t('sessionList.dialog.custom', '自定义')}
                      </button>
                    </div>
                    {useCustomModel ? (
                      <input
                        type="text"
                        value={customModel}
                        onChange={(e) => setCustomModel(e.target.value)}
                        placeholder={t('sessionList.dialog.modelPlaceholder', '输入模型名称，如: gpt-4o-2024-08-06')}
                        className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                        style={{
                          backgroundColor: 'var(--bg-tertiary)',
                          color: 'var(--text-primary)',
                          border: '1px solid var(--border)',
                        }}
                        autoFocus
                      />
                    ) : currentApiProvider.models.length > 0 ? (
                      <Select
                        className="w-full"
                        value={selectedApiModel}
                        onChange={setSelectedApiModel}
                        options={currentApiProvider.models.map((m) => ({ value: m, label: m }))}
                      />
                    ) : (
                      <div
                        className="px-3 py-2 rounded-lg text-xs"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
                      >
                        {t('sessionList.dialog.noModel', '该提供商暂无预定义模型，请使用"自定义"输入模型名称')}
                      </div>
                    )}
                  </div>
                )}

                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label className="block text-xs mb-1" style={{ color: 'var(--text-secondary)' }}>
                      {t('sessionList.dialog.temperature', '温度 (Temperature)')}
                    </label>
                    <input
                      type="number"
                      min={0}
                      max={2}
                      step={0.1}
                      value={sessionTemperature}
                      onChange={(e) => setSessionTemperature(parseFloat(e.target.value) || 0.7)}
                      className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    />
                  </div>
                  <div>
                    <label className="block text-xs mb-1" style={{ color: 'var(--text-secondary)' }}>
                      {t('sessionList.dialog.maxTokens', '最大 Token (可选)')}
                    </label>
                    <input
                      type="number"
                      min={1}
                      step={1}
                      value={sessionMaxTokens ?? ''}
                      onChange={(e) => setSessionMaxTokens(e.target.value ? parseInt(e.target.value) : undefined)}
                      placeholder={t('sessionList.dialog.maxTokens.placeholder', '不限制')}
                      className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    />
                  </div>
                </div>

                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    {t('sessionList.dialog.titleLabel', '会话标题（可选）')}
                  </label>
                  <input
                    type="text"
                    value={customTitle}
                    onChange={(e) => setCustomTitle(e.target.value)}
                    placeholder={`API: ${currentApiProvider?.name || ''} - ${useCustomModel ? customModel || t('sessionList.dialog.custom', '自定义') : selectedApiModel || t('sessionList.dialog.modelFallback', '模型')}`}
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    {t('sessionList.dialog.cwdLabel', '工作目录（可选）')}
                  </label>
                  <div className="flex gap-2">
                    <input
                      type="text"
                      value={customCwd}
                      onChange={(e) => setCustomCwd(e.target.value)}
                      placeholder={t('sessionList.dialog.cwdPlaceholder', '留空使用默认目录')}
                      className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    />
                    <button
                      onClick={async () => {
                        try {
                          
                          const selected = await openDialog({ directory: true, multiple: false });
                          if (selected && typeof selected === 'string') {
                            setCustomCwd(selected);
                          }
                        } catch { /* ignore */ }
                      }}
                      className="px-2.5 py-2 rounded-lg shrink-0 transition-colors hover:opacity-80"
                      style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                      title={t('sessionList.dialog.chooseDir', '选择目录')}
                    >
                      <FolderOpen size={14} />
                    </button>
                  </div>
                </div>
                <div
                  className="px-3 py-2 rounded-lg text-xs"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                >
                  {t('sessionList.dialog.apiHint', '通过 API 直接与模型对话，无需 Agent 中转。请确保已在设置中配置对应 API Key。')}
                </div>
              </div>
            )}

            <div className="flex justify-end gap-2">
              <button
                onClick={() => setShowNewDialog(false)}
                className="px-3 py-1.5 rounded-lg text-xs"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
              >
                {t('sessionList.dialog.cancel', '取消')}
              </button>
              <button
                onClick={handleCreate}
                disabled={creating || (newSessionType === 'api' && apiProviders.length === 0)}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs  disabled:opacity-50"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                {creating ? t('sessionList.dialog.creating', '创建中...') : t('sessionList.dialog.create', '创建')}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}

export const SessionList = memo(SessionListFn);

