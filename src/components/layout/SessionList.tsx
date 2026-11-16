import { useState, useEffect, useMemo, useRef, useCallback, memo } from 'react';
import { Plus, Search, Archive, X, ChevronDown, ChevronRight, RefreshCw, Trash2, Download, Check, ListChecks, Loader2, LogOut, MoreHorizontal, SquareCheckBig, Workflow } from 'lucide-react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from '../../stores/sessionStore';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo, isAgentAvailable } from '../../hooks/useEnvInfo';
import { isWorkflowSession } from '../../utils/sessionType';
import { errorMessage } from '../../utils/errorMessage';
import { useI18n } from '../../hooks/useI18n';
import { SessionListItem } from './SessionListItem';
import { Select } from '../common/Select';
import { AgentBadge } from '../common/AgentBadge';
import { useHostPreviewStore } from '../../stores/hostPreviewStore';
import {
  subscribeProjectCreation,
  useProjectCreationStore,
} from '../../stores/projectCreationStore';
import { useProjectStore } from '../../stores/projectStore';
import {
  PROJECT_FILTER_ALL,
  PROJECT_FILTER_UNASSIGNED,
  ProjectFilter,
  ProjectMembershipDialog,
} from '../project/ProjectPicker';
import { ProjectWorkspaceContext } from '../project/ProjectWorkspaceContext';
import { HEADER_ICON_BTN_STYLE } from './sessionListStyles';
import { NewSessionDialog } from '../session/NewSessionDialog';
import type { ProjectCreateContext } from '../../types/project';

/** ACP `session/list` 返回的宿主会话（字段与后端 `HostSession` 对齐） */
interface HostSessionRow {
  sessionId: string;
  cwd?: string | null;
  title?: string | null;
  updatedAt?: string | null;
}

type HostSessionListOutcome =
  | { status: 'ok'; sessions: HostSessionRow[]; nextCursor?: string | null }
  | { status: 'unsupported'; capability: string; hint: string };

/**
 * 与本地会话列表一致的相对时间（今天 / 昨天 / 月日）。
 *
 * 宿主返回的是 ISO 字符串，直接 `toLocaleString()` 又长又挤，放在行尾会把标题压没。
 */
function formatHostTime(value?: string | null): string {
  if (!value) return '';
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return value;
  const now = new Date();
  const timeStr = d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  if (d.toDateString() === now.toDateString()) return `今天 ${timeStr}`;
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (d.toDateString() === yesterday.toDateString()) return `昨天 ${timeStr}`;
  return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

/** 「工作流会话」折叠分组的展开状态（纯 UI 状态，存 localStorage 即可，不入库）。 */
const WORKFLOW_GROUP_STORAGE_KEY = 'pilotdesk.session-list.workflow-expanded';

function SessionBatchMenu({
  clearDisabled,
  exitDisabled,
  onClear,
  onExit,
}: {
  clearDisabled: boolean;
  exitDisabled?: boolean;
  onClear: () => void;
  onExit: () => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const closeOnOutside = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', closeOnOutside);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('mousedown', closeOnOutside);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        type="button"
        className="pd-btn pd-batch-icon-btn"
        onClick={() => setOpen((value) => !value)}
        title="更多批量操作"
        aria-label="更多批量操作"
        aria-expanded={open}
      >
        <MoreHorizontal size={13} />
      </button>
      {open && (
        <div role="menu" className="pd-menu pd-batch-menu">
          <button
            type="button"
            role="menuitem"
            className="pd-menu-item disabled:opacity-40"
            disabled={clearDisabled}
            onClick={() => {
              setOpen(false);
              onClear();
            }}
          >
            <X size={13} />
            <span className="flex-1">清空选择</span>
          </button>
          <button
            type="button"
            role="menuitem"
            className="pd-menu-item disabled:opacity-40"
            disabled={exitDisabled}
            onClick={() => {
              setOpen(false);
              onExit();
            }}
          >
            <LogOut size={13} />
            <span className="flex-1">退出批量</span>
          </button>
        </div>
      )}
    </div>
  );
}

function SessionListFn({
  style,
  onSessionSelected,
}: {
  style?: React.CSSProperties;
  onSessionSelected?: () => void;
} = {}) {
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
  const [newSessionSeed, setNewSessionSeed] = useState<ProjectCreateContext | null>(null);
  const [projectFilter, setProjectFilter] = useState(PROJECT_FILTER_ALL);
  const [membershipSession, setMembershipSession] = useState<(typeof sessions)[number] | null>(null);
  // 「宿主会话」视图：视图/选中宿主/右侧预览态放在独立 store（预览不是库里的会话）
  const {
    view,
    agentType: hostAgent,
    preview: hostPreview,
    replay: hostReplay,
    setView,
    setAgentType,
    openPreview,
    closePreview,
  } = useHostPreviewStore();
  const [viewMenuOpen, setViewMenuOpen] = useState(false);
  /** 宿主会话列表（按当前选中的宿主加载） */
  const [hostSessions, setHostSessions] = useState<HostSessionRow[]>([]);
  const [hostNextCursor, setHostNextCursor] = useState<string | null>(null);
  const [hostListLoading, setHostListLoading] = useState(false);
  const [hostLoadingMore, setHostLoadingMore] = useState(false);
  const [hostListError, setHostListError] = useState<string | null>(null);
  /** agentType → 不支持说明：记住结论，切回来不再重复探测 */
  const [hostUnsupported, setHostUnsupported] = useState<Record<string, string>>({});
  const [importingHostId, setImportingHostId] = useState<string | null>(null);
  /** 正在回放（点击预览）的宿主会话 */
  const [previewingHostId, setPreviewingHostId] = useState<string | null>(null);
  /** 宿主会话批量选择（只支持批量删除；批量导入见下方说明） */
  const [hostBatchMode, setHostBatchMode] = useState(false);
  const [selectedHostIds, setSelectedHostIds] = useState<Set<string>>(new Set());
  const [hostCwd, setHostCwd] = useState('');
  const [hostProjectId, setHostProjectId] = useState<string | null>(null);
  /** 正在删除的宿主会话（逐行反馈，避免"点了没反应"被重复点） */
  const [deletingHostIds, setDeletingHostIds] = useState<Set<string>>(new Set());
  /** 批量删除进行中 */
  const [hostBatchDeleting, setHostBatchDeleting] = useState(false);
  /** 当前列表数据所属的宿主（换宿主必须重拉，否则会显示上一个宿主的会话） */
  const loadedAgentRef = useRef<string | null>(null);
  /** 当前列表数据对应的宿主目录；换项目/目录时 Agent 不变也必须重拉 */
  const loadedHostCwdRef = useRef<string | null>(null);

  const openNewSessionDialog = useCallback(() => {
    useProjectCreationStore.getState().clear();
    setNewSessionSeed(null);
    setShowNewDialog(true);
  }, []);

  const closeNewSessionDialog = useCallback(() => {
    useProjectCreationStore.getState().clear();
    setShowNewDialog(false);
    setNewSessionSeed(null);
  }, []);

  useEffect(() => subscribeProjectCreation('session', (context) => {
    setNewSessionSeed(context);
    setShowNewDialog(true);
  }), []);

  const { getDisplayName, getEnabledAgentTypes, getTransport } = useAgentRegistry();

  // WebSocket for Agent session lifecycle
  const { closeAgentSession: wsCloseSession } = useAgentEvent();

  // 初始加载：只拉取会话列表。
  // **不自动选中任何会话**：会话模式默认停在初始页（"输入消息开始新会话…"），
  // 选中哪个会话由用户点选决定；同理也不再恢复"上次打开的会话"。
  useEffect(() => {
    const init = async () => {
      await fetchSessions();
    };
    init().catch((err) => {
      showToast(`加载会话失败: ${errorMessage(err)}`, 'error');
    });
  }, [fetchSessions]);

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

  const { envInfo } = useEnvInfo();

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
  const filteredList = (isSearchActive ? searchResults : displayList).filter((session) => {
    if (projectFilter === PROJECT_FILTER_ALL) return true;
    if (projectFilter === PROJECT_FILTER_UNASSIGNED) return !session.projectId;
    return session.projectId === projectFilter;
  });

  // 工作流会话（origin 前缀 `workflow`）是节点每次执行自动创建的内部会话：数量多、同名，
  // 混在时间组里会淹没用户自己的会话，因此收进列表底部一个折叠分组。
  // 只在活动列表、非搜索时折叠——搜索要"搜得到就能看见"，归档页也照常平铺。
  const foldsWorkflow = !showArchived && !isSearchActive;
  const visibleList = foldsWorkflow ? filteredList.filter((s) => !isWorkflowSession(s.origin)) : filteredList;
  const workflowList = foldsWorkflow ? filteredList.filter((s) => isWorkflowSession(s.origin)) : [];
  const allWorkflowSelected =
    workflowList.length > 0 && workflowList.every((s) => selectedIds.has(s.id));

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



  const selectAll = () => {
    // 全选 = 当前可见项：折叠着的工作流会话不计入（与"看不见就不选"一致）
    const ids = new Set(visibleList.map((s) => s.id));
    setSelectedIds(ids);
  };

  const selectAllWorkflow = () => {
    const ids = workflowList.map((s) => s.id);
    setSelectedIds((prev) => new Set([...prev, ...ids]));
  };

  const deselectAll = () => {
    setSelectedIds(new Set());
  };

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

  // ── 宿主会话（左侧「宿主会话」视图）──
  /** 可用的 ACP 宿主：只有环境检测判定可用的才出现（起不来就列不出会话来） */
  const hostAgents = useMemo(
    () =>
      getEnabledAgentTypes().filter(
        (t) => t !== 'api' && getTransport(t) === 'acp' && isAgentAvailable(envInfo, t),
      ),
    [envInfo, getEnabledAgentTypes, getTransport],
  );

  const loadHostSessions = useCallback(
    async (agentType: string, cursor?: string, force = false, cwdOverride?: string) => {
      if (!agentType) return;
      const requestedCwd = (cwdOverride ?? hostCwd).trim();
      loadedAgentRef.current = agentType;
      loadedHostCwdRef.current = requestedCwd;
      const more = !!cursor;
      if (more) setHostLoadingMore(true);
      else {
        setHostListLoading(true);
        setHostSessions([]);
        setHostNextCursor(null);
        // 手动刷新要重新探测：上次的"不支持"结论可能因为适配器升级而失效
        if (force) {
          setHostUnsupported((prev) => {
            const next = { ...prev };
            delete next[agentType];
            return next;
          });
        }
      }
      setHostListError(null);
      try {
        const outcome = await invoke<HostSessionListOutcome>('acp_list_sessions', {
          agentType,
          cwd: requestedCwd || null,
          cursor: cursor ?? null,
        });
        if (outcome.status === 'unsupported') {
          setHostUnsupported((prev) => ({ ...prev, [agentType]: outcome.hint }));
          setHostSessions([]);
          setHostNextCursor(null);
        } else {
          setHostSessions((prev) => {
            if (!more) return outcome.sessions;
            // 分页去重：对端可能把同一会话重复返回
            const seen = new Set(prev.map((s) => s.sessionId));
            return [...prev, ...outcome.sessions.filter((s) => !seen.has(s.sessionId))];
          });
          setHostNextCursor(outcome.nextCursor ?? null);
        }
      } catch (err) {
        const msg = errorMessage(err);
        setHostListError(msg);
        showToast(`加载宿主会话失败: ${msg}`, 'error');
      } finally {
        setHostListLoading(false);
        setHostLoadingMore(false);
      }
    },
    [hostCwd],
  );

  /** 切换宿主（下拉）：换人必须重拉，否则会显示上一个宿主的会话 */
  const switchHostAgent = useCallback((agentType: string | null, cwdOverride?: string) => {
    setAgentType(agentType);
    // 选中项属于上一个宿主，换了宿主必须清掉
    setSelectedHostIds(new Set());
    if (!agentType) return;
    // 右侧若还停着上一个宿主的预览，先收掉（否则标题与会话对不上）
    if (hostPreview && hostPreview.agentType !== agentType) closePreview();
    const requestedCwd = (cwdOverride ?? hostCwd).trim();
    const sameLoadedQuery =
      loadedAgentRef.current === agentType
      && loadedHostCwdRef.current === requestedCwd;
    if (sameLoadedQuery || hostUnsupported[agentType]) return;
    loadedAgentRef.current = agentType;
    loadedHostCwdRef.current = requestedCwd;
    void loadHostSessions(agentType, undefined, false, requestedCwd);
  }, [hostCwd, hostPreview, hostUnsupported, closePreview, setAgentType, loadHostSessions]);

  /**
   * 进入宿主视图（下拉切换 / 弹窗入口共用）。
   *
   * `preferred` 是调用方指定的宿主（如新建会话弹窗里用户选中的 ACP Agent）：
   * 传了就优先用它，否则沿用上次选中的，最后才回退到第一个可用宿主。
   */
  const enterHostView = useCallback(
    (preferred?: string | null, cwd?: string, projectId?: string | null) => {
      const nextCwd = cwd !== undefined ? cwd : hostCwd;
      const nextProjectId = projectId !== undefined ? projectId : hostProjectId;
      const projectChanged = nextProjectId !== hostProjectId;
      const cwdChanged = cwd !== undefined && nextCwd.trim() !== hostCwd.trim();
      if (cwdChanged) setHostCwd(cwd);
      if (projectChanged) setHostProjectId(nextProjectId);
      // 宿主会话的导入归属与恢复目录都属于当前预览上下文；切换后不能沿用上一份预览。
      if (projectChanged || cwdChanged) closePreview();
      setView('host');
      const target =
        (preferred && hostAgents.includes(preferred) ? preferred : null) ??
        (hostAgent && hostAgents.includes(hostAgent) ? hostAgent : hostAgents[0]);
      if (!target) return;
      // 换宿主走同一套逻辑（清选择、收掉对不上的预览、重拉列表）
      if (target !== hostAgent || loadedHostCwdRef.current !== nextCwd.trim()) {
        switchHostAgent(target, nextCwd);
        return;
      }
      if (loadedAgentRef.current === target || hostUnsupported[target]) return;
      loadedAgentRef.current = target;
      loadedHostCwdRef.current = nextCwd.trim();
      void loadHostSessions(target, undefined, false, nextCwd);
    },
    [
      hostAgent,
      hostAgents,
      hostCwd,
      hostProjectId,
      hostUnsupported,
      setView,
      switchHostAgent,
      loadHostSessions,
      closePreview,
    ],
  );

  // 环境检测结果晚于进入宿主视图时（启动早期常见）补一次自动加载。
  // 用定时器异步触发：effect 内同步 setState 会触发级联渲染（react-hooks/set-state-in-effect）
  useEffect(() => {
    if (view !== 'host') return;
    if (hostAgents.length === 0) return;
    if (loadedAgentRef.current) return;
    const timer = setTimeout(() => enterHostView(), 0);
    return () => clearTimeout(timer);
  }, [view, hostAgents, enterHostView]);

  /** 点击行：只读回放整段历史（不落库） */
  const previewHostSession = useCallback(async (row: HostSessionRow) => {
    if (!hostAgent || previewingHostId) return;
    setPreviewingHostId(row.sessionId);
    try {
      const messages = await invoke<Array<{ role: 'user' | 'assistant'; content: string }>>(
        'acp_load_host_session',
        { agentType: hostAgent, hostSessionId: row.sessionId, cwd: row.cwd ?? null },
      );
      openPreview({
        agentType: hostAgent,
        hostSessionId: row.sessionId,
        title: (row.title || '').trim() || '导入的会话',
        projectId: hostProjectId,
        cwd: row.cwd ?? null,
        messages: messages.map((m, i) => ({
          id: `${row.sessionId}-${i}`,
          role: m.role,
          content: m.content,
        })),
      });
    } catch (err) {
      showToast(`预览失败: ${errorMessage(err)}`, 'error');
    } finally {
      setPreviewingHostId(null);
    }
  }, [hostAgent, hostProjectId, previewingHostId, openPreview]);

  /** 导入为本地会话并切过去（宿主侧会话已导入过则直接打开） */
  const importHostSession = useCallback(async (row: HostSessionRow) => {
    if (!hostAgent || importingHostId) return;
    const existing = sessions.find(
      (s) => s.agentType === hostAgent && s.agentSessionId === row.sessionId,
    );
    if (existing) {
      closePreview();
      void selectSession(existing.id);
      return;
    }

    setImportingHostId(row.sessionId);
    const cwd = row.cwd ?? null;
    const title = (row.title || '').trim() || '导入的会话';
    let createdId: string | null = null;
    try {
      const local = await createSession(
        hostAgent,
        title,
        undefined,
        undefined,
        undefined,
        undefined,
        hostProjectId ?? undefined,
      );
      createdId = local.id;
      const count = await invoke<number>('acp_import_host_session', {
        agentType: hostAgent,
        hostSessionId: row.sessionId,
        cwd,
        localSessionId: local.id,
      });
      closePreview();
      // 绑定（agentSessionId）与生效 cwd 都只写进了库，内存里还是 `createSession`
      // 那一刻的副本。不拉新的话首次发送会按"无 agentSessionId"走 `session/new`，
      // 宿主上下文直接丢失，回写的新 id 还会覆盖原绑定。
      await refreshSessions();
      showToast(`已导入 ${count} 条消息`, 'success');
      // 导入的消息已落库，选中后从库里加载回来
      void selectSession(local.id);
    } catch (err) {
      // 导入失败不留空会话
      if (createdId) {
        try { await deleteSession(createdId); } catch { /* ignore */ }
      }
      showToast(`导入失败: ${errorMessage(err)}`, 'error');
    } finally {
      setImportingHostId(null);
    }
  }, [hostAgent, hostProjectId, importingHostId, sessions, createSession, deleteSession, selectSession, closePreview, refreshSessions]);

  /** 删除宿主侧会话（只删宿主，本地已导入副本保留） */
  const deleteHostSession = useCallback(async (row: HostSessionRow) => {
    if (!hostAgent) return;
    if (deletingHostIds.has(row.sessionId)) return;
    const ok = await confirmDialog({
      title: '删除宿主会话',
      message: `确定要删除宿主侧的会话「${(row.title || '').trim() || '（无标题）'}」吗？\n\n只删除宿主里的记录，已导入到本应用的会话不受影响。`,
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;
    // 标记为"删除中"：行内显示进度并禁用按钮，避免用户以为没执行而重复点击
    setDeletingHostIds((prev) => new Set(prev).add(row.sessionId));
    try {
      const outcome = await invoke<{ status: 'ok' | 'unsupported'; hint?: string }>(
        'acp_delete_host_session',
        { agentType: hostAgent, hostSessionId: row.sessionId, cwd: row.cwd ?? null },
      );
      if (outcome.status === 'unsupported') {
        showToast(outcome.hint || '该适配器不支持删除宿主会话', 'warning');
        return;
      }
      showToast('已删除宿主会话', 'success');
      setHostSessions((prev) => prev.filter((s) => s.sessionId !== row.sessionId));
      if (hostPreview?.hostSessionId === row.sessionId) closePreview();
    } catch (err) {
      showToast(`删除失败: ${errorMessage(err)}`, 'error');
    } finally {
      setDeletingHostIds((prev) => {
        const next = new Set(prev);
        next.delete(row.sessionId);
        return next;
      });
    }
  }, [hostAgent, hostPreview, closePreview, deletingHostIds]);

  const toggleHostSelect = useCallback((sessionId: string) => {
    setSelectedHostIds((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      return next;
    });
  }, []);

  /**
   * 批量删除宿主会话。
   *
   * 逐条调用（ACP 每次都要起一个一次性进程）；结果合并成一条提示，
   * 避免 N 条会话刷 N 个 toast。不支持删除的适配器（如 hermes）单独说明。
   */
  const batchDeleteHostSessions = useCallback(async () => {
    if (!hostAgent || selectedHostIds.size === 0 || hostBatchDeleting) return;
    const ids = Array.from(selectedHostIds);
    const ok = await confirmDialog({
      title: '批量删除宿主会话',
      message: `确定要删除宿主侧的 ${ids.length} 条会话吗？\n\n只删除宿主里的记录，已导入到本应用的会话不受影响。`,
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;

    // 逐条删除耗时较长：置忙并让按钮显示进度，避免重复点击
    setHostBatchDeleting(true);
    const cwdOf = (id: string) => hostSessions.find((s) => s.sessionId === id)?.cwd ?? null;
    const deleted: string[] = [];
    let unsupportedHint: string | null = null;
    let firstError: string | null = null;
    try {
      for (const id of ids) {
        // 逐条显示"删除中"，用户能看到进度在推进
        setDeletingHostIds((prev) => new Set(prev).add(id));
        try {
          const outcome = await invoke<{ status: 'ok' | 'unsupported'; hint?: string }>(
            'acp_delete_host_session',
            { agentType: hostAgent, hostSessionId: id, cwd: cwdOf(id) },
          );
          if (outcome.status === 'ok') {
            deleted.push(id);
            // **逐条移除**：连同删除中的样式一起消失，用户能看到"删一条少一条"，
            // 而不是等整个批次跑完才一起消失（那样中途像卡住了）
            setHostSessions((prev) => prev.filter((s) => s.sessionId !== id));
            if (hostPreview?.hostSessionId === id) closePreview();
          } else {
            unsupportedHint = outcome.hint || unsupportedHint;
          }
        } catch (err) {
          firstError = firstError ?? errorMessage(err);
        } finally {
          setDeletingHostIds((prev) => {
            const next = new Set(prev);
            next.delete(id);
            return next;
          });
        }
      }
    } finally {
      setHostBatchDeleting(false);
    }

    // 列表移除与预览关闭已在每条成功时即时完成，这里只做汇总提示与状态收尾
    if (deleted.length > 0) {
      showToast(`已删除 ${deleted.length} 条宿主会话`, 'success');
    }
    if (unsupportedHint) showToast(unsupportedHint, 'warning');
    else if (firstError) showToast(`删除失败: ${firstError}`, 'error');

    setSelectedHostIds(new Set());
    if (deleted.length > 0) setHostBatchMode(false);
  }, [hostAgent, selectedHostIds, hostSessions, hostPreview, closePreview, hostBatchDeleting]);

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

  const handleProjectChanged = useCallback(async () => {
    await refreshSessions();
    await useProjectStore.getState().loadProjects();
  }, [refreshSessions]);

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
        if (!batchMode) {
          selectSession(sessionId);
          onSessionSelected?.();
        }
        else toggleBatchSelect(sessionId);
    }
  }, [batchMode, selectSession, toggleBatchSelect, handleArchive, handleUnarchive, handleDelete, onSessionSelected]);

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
      onArchive={showArchived ? handleUnarchive : handleArchive}
      onDelete={handleDelete}
      onSetProject={isWorkflowSession(session.origin) ? undefined : setMembershipSession}
    />
  );

  const renderGroup = (label: string, items: typeof sessions) => {
    if (items.length === 0) return null;
    return (
      <div key={label}>
        <div className="pd-session-group-label text-[10px]" style={{ color: 'var(--text-secondary)' }}>
          {label}
        </div>
        {items.map(renderItem)}
      </div>
    );
  };

  /** 当前宿主的"不支持会话列表"说明（没有则为空） */
  const hostUnsupportedHint = hostAgent ? hostUnsupported[hostAgent] ?? null : null;
  /** 宿主会话的客户端过滤（标题 / 目录） */
  const filteredHostSessions = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return hostSessions;
    return hostSessions.filter(
      (r) =>
        (r.title || '').toLowerCase().includes(q) || (r.cwd || '').toLowerCase().includes(q),
    );
  }, [hostSessions, searchQuery]);

  useEffect(() => {
    if (!viewMenuOpen) return;
    // 点菜单外 / 按 Esc 关闭：用文档级监听而不是遮罩层——遮罩会被侧栏的
    // 层叠上下文/溢出边界限制，点在主区域时未必能命中
    const onDown = (e: MouseEvent) => {
      const el = e.target as HTMLElement | null;
      if (el?.closest('[data-session-view-menu]')) return;
      setViewMenuOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setViewMenuOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [viewMenuOpen]);

  return (
    <aside
      className="pd-session-list shrink-0 flex flex-col overflow-hidden rounded-lg"
      // 壳层风格：面板靠"底色 + 圆角 + 与相邻面板之间的 8px 缝"区分，不再画 border-right（见 App.tsx 工作区）
      style={{ backgroundColor: 'var(--bg-side)', ...style }}
    >
      {/* Header：标题即视图切换器（本地会话 / 宿主会话）；右侧只放图标按钮，
          260px 宽度下才放得下（原先三个按钮都带文字，已无余量） */}
      <div className="pd-session-list-header flex items-center h-10" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="relative" data-session-view-menu>
          <button
            onClick={() => setViewMenuOpen((v) => !v)}
            className="flex items-center gap-1 text-xs font-medium"
            style={{ color: 'var(--text-primary)' }}
            title={t('sessionList.view.title', '切换会话视图')}
          >
            <span className="truncate">
              {view === 'host'
                ? t('sessionList.host.listTitle', '宿主会话')
                : t('sessionList.title', '会话')}
            </span>
            <ChevronDown size={12} style={{ color: 'var(--text-tertiary)' }} />
          </button>
          {viewMenuOpen && (
            <div
              role="menu"
              className="pd-menu absolute left-0 top-full mt-1 z-20"
              style={{
                minWidth: 132,
              }}
            >
              {([
                { key: 'local' as const, label: t('sessionList.view.local', '本地会话') },
                { key: 'host' as const, label: t('sessionList.view.host', '宿主会话') },
              ]).map((item) => (
                <button
                  key={item.key}
                  type="button"
                  role="menuitem"
                  data-active={view === item.key ? 'true' : undefined}
                  onClick={() => {
                    setViewMenuOpen(false);
                    if (item.key === 'host') {
                      enterHostView(null, '', null);
                    } else {
                      setView('local');
                      setHostBatchMode(false);
                      setSelectedHostIds(new Set());
                    }
                  }}
                  className="pd-menu-item"
                >
                  {item.label}
                  {view === item.key && <Check size={11} className="ml-auto" />}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex-1" />
        {view === 'host' ? (
          <div className="flex items-center gap-1">
            <button
              onClick={() => {
                setHostBatchMode((v) => !v);
                setSelectedHostIds(new Set());
              }}
              className="pd-btn pd-btn-sm"
              style={{
                ...HEADER_ICON_BTN_STYLE,
                backgroundColor: hostBatchMode ? 'var(--accent)' : 'var(--bg-tertiary)',
                color: hostBatchMode ? '#fff' : 'var(--text-secondary)',
              }}
              title={t('sessionList.batch.title', '批量操作')}
            >
              <ListChecks size={13} />
            </button>
            <button
              onClick={() => {
                if (!hostAgent) return;
                loadedAgentRef.current = hostAgent;
                void loadHostSessions(hostAgent, undefined, true, hostCwd);
              }}
              disabled={!hostAgent || hostListLoading}
              className="pd-btn pd-btn-sm"
              style={{ ...HEADER_ICON_BTN_STYLE, color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
              title={t('sessionList.host.refresh', '刷新宿主会话')}
            >
              <RefreshCw size={13} className={hostListLoading ? 'animate-spin' : ''} />
            </button>
          </div>
        ) : (
          <div className="flex items-center gap-1">
            <button
              onClick={() => setBatchMode(!batchMode)}
              className="pd-btn pd-btn-sm"
              style={{
                ...HEADER_ICON_BTN_STYLE,
                backgroundColor: batchMode ? 'var(--accent)' : 'var(--bg-tertiary)',
                color: batchMode ? '#fff' : 'var(--text-secondary)',
              }}
              title={t('sessionList.batch.title', '批量操作')}
            >
              {/* 语义=多选/批量；不要用宫格图标（与顶部门户标签的 LayoutGrid 撞脸） */}
              <ListChecks size={13} />
            </button>
            <button
              onClick={toggleArchived}
              className="pd-btn pd-btn-sm"
              style={{ ...HEADER_ICON_BTN_STYLE, color: showArchived ? 'var(--accent)' : 'var(--text-secondary)', backgroundColor: showArchived ? 'var(--accent-light)' : 'var(--bg-tertiary)' }}
              title={t('sessionList.archive.title', '归档')}
            >
              <Archive size={13} />
            </button>
            <button
              onClick={openNewSessionDialog}
              className="pd-btn pd-btn-sm pd-btn-primary"
              style={HEADER_ICON_BTN_STYLE}
              title={t('sessionList.new.title', '新建会话')}
            >
              <Plus size={13} />
            </button>
          </div>
        )}
      </div>

      {/* Batch operations row - above search（仅本地会话视图有意义） */}
      {batchMode && view === 'local' && (
        <div className="pd-batch-bar">
          <span className="pd-batch-count">
            {selectedIds.size > 0 ? `已选 ${selectedIds.size}` : '未选择'}
          </span>
          <button
            onClick={selectAll}
            className="pd-btn pd-batch-icon-btn"
            title={t('sessionList.selectAllRegular.title', '全选普通会话')}
            aria-label={t('sessionList.selectAllRegular.title', '全选普通会话')}
          >
            <SquareCheckBig size={13} />
          </button>
          {workflowList.length > 0 && (
            <button
              onClick={selectAllWorkflow}
              className="pd-btn pd-batch-icon-btn"
              title={t('sessionList.selectAllWorkflow.title', `全选工作流会话（${workflowList.length}）`, { count: workflowList.length })}
              aria-label={t('sessionList.selectAllWorkflow.title', `全选工作流会话（${workflowList.length}）`, { count: workflowList.length })}
              aria-pressed={allWorkflowSelected}
              style={{
                color: allWorkflowSelected ? 'var(--accent)' : undefined,
                backgroundColor: allWorkflowSelected ? 'var(--accent-light)' : undefined,
              }}
            >
              <Workflow size={13} />
            </button>
          )}
          <button
            onClick={showArchived ? batchUnarchive : batchArchive}
            className="pd-btn pd-batch-icon-btn"
            disabled={selectedIds.size === 0}
            title={showArchived ? t('sessionList.batchUnarchive.title', '批量取消归档') : t('sessionList.batchArchive.title', '批量归档')}
            aria-label={showArchived ? t('sessionList.batchUnarchive.title', '批量取消归档') : t('sessionList.batchArchive.title', '批量归档')}
          >
            <Archive size={13} />
          </button>
          <button
            onClick={batchDelete}
            className="pd-btn pd-batch-icon-btn is-danger"
            disabled={selectedIds.size === 0}
            title={t('sessionList.batchDelete.title', '批量删除')}
            aria-label={t('sessionList.batchDelete.title', '批量删除')}
          >
            <Trash2 size={13} />
          </button>
          <SessionBatchMenu
            clearDisabled={selectedIds.size === 0}
            onClear={deselectAll}
            onExit={() => {
              setBatchMode(false);
              setSelectedIds(new Set());
            }}
          />
        </div>
      )}
      {/* 宿主会话批量动作条：只提供批量删除。
          批量导入暂不做——每条导入都要单独拉起一次 ACP 进程并重放整段历史，
          N 条会话会串行等待 N 次，且会一次性在本地列表里堆出 N 条会话，风险大于收益。 */}
      {view === 'host' && hostBatchMode && (
        <div className="pd-batch-bar">
          <span className="pd-batch-count">
            {selectedHostIds.size > 0 ? `已选 ${selectedHostIds.size}` : '未选择'}
          </span>
          <button
            onClick={() => setSelectedHostIds(new Set(filteredHostSessions.map((r) => r.sessionId)))}
            className="pd-btn pd-batch-icon-btn"
            disabled={hostBatchDeleting}
            title="全选"
            aria-label="全选"
          >
            <SquareCheckBig size={13} />
          </button>
          <button
            onClick={() => void batchDeleteHostSessions()}
            disabled={selectedHostIds.size === 0 || hostBatchDeleting}
            className="pd-btn pd-batch-icon-btn is-danger"
            title="批量删除宿主会话（只删宿主，本地已导入的副本保留）"
            aria-label="批量删除宿主会话"
          >
            {hostBatchDeleting ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Trash2 size={13} />
            )}
          </button>
          <SessionBatchMenu
            clearDisabled={selectedHostIds.size === 0 || hostBatchDeleting}
            exitDisabled={hostBatchDeleting}
            onClear={() => setSelectedHostIds(new Set())}
            onExit={() => {
              setHostBatchMode(false);
              setSelectedHostIds(new Set());
            }}
          />
        </div>
      )}

      {/* Search / 宿主选择：宿主视图复用同一个输入框做客户端过滤 */}
      {view === 'host' ? (
        <div className="pd-session-search py-2 space-y-1.5">
          {hostAgents.length > 1 ? (
            <Select
              value={hostAgent ?? ''}
              onChange={(v) => switchHostAgent(v || null)}
              options={hostAgents.map((a) => ({ value: a, label: getDisplayName(a) }))}
              className="w-full"
            />
          ) : hostAgents.length === 1 ? (
            <div className="text-[10px] px-1 truncate" style={{ color: 'var(--text-tertiary)' }}>
              {getDisplayName(hostAgents[0])}
            </div>
          ) : null}
          <ProjectWorkspaceContext
            projectId={hostProjectId}
            workRoot={hostCwd || null}
            rootSourceLabel={hostCwd ? '宿主会话目录' : undefined}
            className="w-full"
          />
          <div className="relative">
            <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t('sessionList.host.search', '搜索宿主会话...')}
              className="search-input"
            />
          </div>
        </div>
      ) : (
        <div className="pd-session-search py-2 space-y-1.5">
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
          <ProjectFilter
            value={projectFilter}
            onChange={setProjectFilter}
            className="w-full"
            width="100%"
          />
        </div>
      )}

      {/* 宿主会话列表：与本应用会话互斥展示 */}
      {view === 'host' ? (
        <div
          className="flex-1 overflow-y-auto pd-scroll-stable"
          onScroll={(e) => {
            const el = e.currentTarget;
            if (!hostNextCursor || hostLoadingMore || !hostAgent) return;
            if (el.scrollHeight - el.scrollTop - el.clientHeight < 24) {
              void loadHostSessions(hostAgent, hostNextCursor, false, hostCwd);
            }
          }}
        >
          {hostAgents.length === 0 ? (
            <div className="px-4 py-6 text-center text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              没有可用的 ACP 宿主。请先在「设置 › 运行与策略 › 环境检测」确认适配器可用。
            </div>
          ) : hostListLoading ? (
            <div className="flex items-center justify-center py-8">
              <div className="pilotdesk-spinner" />
            </div>
          ) : hostUnsupportedHint ? (
            <div
              className="m-2 px-3 py-2 rounded-lg text-xs leading-relaxed"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            >
              {hostUnsupportedHint}
            </div>
          ) : hostListError ? (
            <div className="p-2 space-y-2">
              <div
                className="px-3 py-2 rounded-lg text-xs leading-relaxed"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
              >
                {hostListError}
              </div>
              <button
                onClick={() => hostAgent && void loadHostSessions(hostAgent, undefined, true, hostCwd)}
                className="pd-btn pd-btn-sm"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                {t('sessionList.import.retry', '重试')}
              </button>
            </div>
          ) : filteredHostSessions.length === 0 ? (
            <div className="px-4 py-6 text-center text-xs" style={{ color: 'var(--text-tertiary)' }}>
              {searchQuery.trim()
                ? t('sessionList.host.noMatch', '没有匹配的宿主会话')
                : t('sessionList.import.empty', '宿主暂无会话记录')}
            </div>
          ) : (
            <div className="py-0">
              {filteredHostSessions.map((row) => {
                const busyPreview = previewingHostId === row.sessionId;
                const busyImport = importingHostId === row.sessionId;
                const deleting = deletingHostIds.has(row.sessionId);
                const isPreviewing = hostPreview?.hostSessionId === row.sessionId;
                // 逐条露出阶段：列表也要转圈，否则右侧在动、左侧像没反应
                const replayingRow = isPreviewing && hostReplay !== null;
                const selected = selectedHostIds.has(row.sessionId);
                const updatedLabel = formatHostTime(row.updatedAt);
                return (
                  <div
                    key={row.sessionId}
                    // 与本地会话行保持同一套视觉：内边距/分隔线/选中竖条/悬停才出现的行内动作
                    className="group px-3 py-2 cursor-pointer transition-colors"
                    style={{
                      backgroundColor: selected
                        ? 'var(--accent-light)'
                        : isPreviewing
                          ? 'var(--border)'
                          : 'transparent',
                      borderBottom: '1px solid var(--border)',
                      backgroundImage:
                        selected || isPreviewing
                          ? 'linear-gradient(to right, var(--accent) 3px, transparent 3px)'
                          : undefined,
                      // 删除中整行降透明度：明确的"已受理、正在处理"反馈
                      opacity: deleting ? 0.55 : 1,
                    }}
                    onClick={() => {
                      if (deleting || hostBatchDeleting) return;
                      if (hostBatchMode) {
                        toggleHostSelect(row.sessionId);
                        return;
                      }
                      if (busyPreview || busyImport) return;
                      void previewHostSession(row);
                    }}
                    title={row.cwd || undefined}
                  >
                    <div className="flex items-start gap-2">
                      {hostBatchMode && (
                        <div className="shrink-0 pt-1">
                          <div
                            className="w-4 h-4 rounded border flex items-center justify-center transition-colors"
                            style={{
                              borderColor: selected ? 'var(--accent)' : 'var(--border)',
                              backgroundColor: selected ? 'var(--accent)' : 'transparent',
                            }}
                          >
                            {selected && <Check size={10} color="#fff" />}
                          </div>
                        </div>
                      )}
                      <div className="shrink-0">
                        <AgentBadge agentType={hostAgent ?? ''} />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center justify-between gap-1">
                          <span className="text-xs truncate">
                            {deleting ? (
                              '删除中…'
                            ) : busyPreview || replayingRow ? (
                              <span className="inline-flex items-center gap-1">
                                <Loader2 size={10} className="animate-spin shrink-0" />
                                {t('sessionList.host.loading', '回放中…')}
                              </span>
                            ) : (
                              row.title || '（无标题）'
                            )}
                          </span>
                          {updatedLabel && !deleting && (
                            <span
                              className="text-[10px] shrink-0"
                              style={{ color: 'var(--text-secondary)' }}
                            >
                              {updatedLabel}
                            </span>
                          )}
                        </div>
                        <p
                          className="text-[10px] mt-0.5 truncate"
                          style={{ color: 'var(--text-secondary)' }}
                        >
                          {deleting ? '正在删除宿主侧记录…' : row.cwd || '-'}
                        </p>
                      </div>
                      {!hostBatchMode && (
                        <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              void importHostSession(row);
                            }}
                            disabled={!!importingHostId || deleting || hostBatchDeleting}
                            className="pd-btn p-0.5 rounded transition-all hover:bg-black/10 dark:hover:bg-white/10 active:scale-90 disabled:opacity-40"
                            style={{ color: 'var(--accent)' }}
                            title={t('sessionList.host.import', '导入并继续')}
                          >
                            <Download size={11} />
                          </button>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              void deleteHostSession(row);
                            }}
                            disabled={deleting || hostBatchDeleting}
                            className="p-0.5 rounded transition-all hover:bg-red-500/20 active:scale-90 disabled:opacity-40"
                            style={{ color: '#EF4444' }}
                            title={deleting ? '正在删除…' : t('sessionList.host.delete', '删除宿主会话')}
                          >
                            {deleting ? <Loader2 size={11} className="animate-spin" /> : <Trash2 size={11} />}
                          </button>
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
              {hostLoadingMore && (
                <div className="py-2 text-center text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {t('sessionList.import.loadingMore', '加载中…')}
                </div>
              )}
            </div>
          )}
        </div>
      ) : isLoadingSessions ? (
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
                  onClick={openNewSessionDialog}
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

      {/* New session dialog：遮罩不响应点击——误触外部会丢掉刚填的内容，只能走「取消 / 关闭」 */}
      {showNewDialog && (
        <NewSessionDialog
          projectSeed={newSessionSeed}
          onClose={closeNewSessionDialog}
          onCreated={async (sessionId) => {
            closeNewSessionDialog();
            await selectSession(sessionId);
          }}
          onOpenHostSessions={(agentType, cwd, projectId) => {
            closeNewSessionDialog();
            enterHostView(agentType, cwd, projectId ?? null);
          }}
        />
      )}
      {membershipSession && (
        <ProjectMembershipDialog
          workType="session"
          workId={membershipSession.id}
          workTitle={membershipSession.title}
          currentProjectId={membershipSession.projectId}
          onClose={() => setMembershipSession(null)}
          onChanged={handleProjectChanged}
        />
      )}
    </aside>
  );
}

export const SessionList = memo(SessionListFn);

