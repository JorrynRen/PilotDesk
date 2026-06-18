import { useState, useEffect, useRef, useCallback, memo } from 'react';
import { Plus, Search, Archive, Key, ChevronDown, X, Trash2 } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
import { useApiProviderStore, getApiKey } from '../../stores/apiProviderStore';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { AGENT_THEMES } from '../../types';
import { SessionListItem } from './SessionListItem';

type NewSessionType = 'claude' | 'hermes' | 'codex' | 'api' | 'codex';

function SessionListFn() {
  const sessions = useSessionStore((s) => s.sessions);
  const archivedSessions = useSessionStore((s) => s.archivedSessions);
  const currentSessionId = useSessionStore((s) => s.currentSessionId);
  const isLoadingSessions = useSessionStore((s) => s.isLoadingSessions);
  const showArchived = useSessionStore((s) => s.showArchived);
  const fetchSessions = useSessionStore((s) => s.fetchSessions);
  const selectSession = useSessionStore((s) => s.selectSession);
  const createSession = useSessionStore((s) => s.createSession);
  const archiveSession = useSessionStore((s) => s.archiveSession);
  const deleteSession = useSessionStore((s) => s.deleteSession);
  const renameSession = useSessionStore((s) => s.renameSession);
  const toggleArchived = useSessionStore((s) => s.toggleArchived);

  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<typeof sessions>([]);
  const [isSearching, setIsSearching] = useState(false);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showNewDialog, setShowNewDialog] = useState(false);
  const [newSessionType, setNewSessionType] = useState<NewSessionType>('claude');
  const [selectedApiProvider, setSelectedApiProvider] = useState('');
  const [selectedApiModel, setSelectedApiModel] = useState('');
  const [customModel, setCustomModel] = useState('');
  const [useCustomModel, setUseCustomModel] = useState(false);
  const [customTitle, setCustomTitle] = useState('');
  const [customCwd, setCustomCwd] = useState('');
  const [creating, setCreating] = useState(false);
  const [installedAgents, setInstalledAgents] = useState<Set<string>>(new Set());
  const [envLoading, setEnvLoading] = useState(true);

  // WebSocket for Agent session lifecycle
  const { createAgentSession: wsCreateSession, closeAgentSession: wsCloseSession } = useAgentEvent();

  // API providers from SQLite via store
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();

  useEffect(() => {
    fetchSessions().catch((err) => {
      showToast(`加载会话失败: ${err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err)}`, 'error');
    });

    fetchProviders().catch(() => {});


    // Detect installed agents for session creation filtering
    (async () => {
      try {
        const info = await invoke<any>('detect_env');
        const installed = new Set<string>();
        if (info.claudeCodeVersion) installed.add('claude');
        if (info.hermesVersion) installed.add('hermes');
        if (info.codexVersion) installed.add('codex');
        setInstalledAgents(installed);
      } catch { /* ignore */ }
      setEnvLoading(false);
    })();
  }, [fetchSessions, fetchProviders]);

  // Debounced session search
  useEffect(() => {
    if (!searchQuery.trim()) {
      setSearchResults([]);
      setIsSearching(false);
      return;
    }
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

  // Group by time
  const now = Date.now() / 1000;
  const todayStart = Math.floor(now / 86400) * 86400;
  const yesterdayStart = todayStart - 86400;

  const today = filteredList.filter((s) => s.updatedAt >= todayStart);
  const yesterday = filteredList.filter(
    (s) => s.updatedAt >= yesterdayStart && s.updatedAt < todayStart
  );
  const earlier = filteredList.filter((s) => s.updatedAt < yesterdayStart);



  const selectAll = useCallback(() => {
    const ids = new Set(filteredList.map((s) => s.id));
    setSelectedIds(ids);
  }, [filteredList]);

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

  const batchDelete = useCallback(async () => {
    if (selectedIds.size === 0) return;
    for (const id of selectedIds) {
      try { await invoke('delete_session', { sessionId: id }); } catch { /* ignore */ }
    }
    setSelectedIds(new Set());
    setBatchMode(false);
    fetchSessions();
    showToast(`已删除 ${selectedIds.size} 个会话`, 'success');
  }, [selectedIds, fetchSessions]);

  // Reload providers when dialog opens
  useEffect(() => {
    if (showNewDialog) {
      fetchProviders().then(() => {
        // After fetch, auto-select first provider via the updated store state
      }).catch(() => {});

      // Reset state
      setUseCustomModel(false);
      setCustomModel('');
      setCustomTitle('');
      setCustomCwd('');
    }
  }, [showNewDialog]);

  // Update model when provider changes; auto-select first if none
  useEffect(() => {
    if (apiProviders.length > 0 && !selectedApiProvider) {
      setSelectedApiProvider(apiProviders[0].id);
    }
    const provider = apiProviders.find((p) => p.id === selectedApiProvider);
    if (provider) {
      setSelectedApiModel(provider.models[0] || '');
      setUseCustomModel(false);
      setCustomModel('');
    }
  }, [selectedApiProvider, apiProviders]);

  const handleCreate = async () => {
    setCreating(true);
    try {
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
        const provider = apiProviders.find((p) => p.id === selectedApiProvider)!;
        const session = await createSession(
          'api' as const,
          undefined,
          customTitle.trim() || `API: ${provider.name} - ${model}`,
          selectedApiProvider,
          model,
        );
        setShowNewDialog(false);
        selectSession(session.id);
      } else {
        const session = await createSession(
          newSessionType,
          customCwd.trim() || undefined,
          customTitle.trim() || undefined,
        );
        // Notify Sidecar to create the Agent session
        wsCreateSession(session.id, newSessionType, customCwd.trim() || undefined);
        setShowNewDialog(false);
        selectSession(session.id);
      }
    } catch (err) {
      showToast(`创建会话失败: ${err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err)}`, 'error');
    } finally {
      setCreating(false);
    }
  };

  const handleArchive = async (id: string) => {
    try {
      // Notify Sidecar to close the Agent session
      const session = sessions.find(s => s.id === id);
      if (session && session.agentType !== 'api') {
        wsCloseSession(id, session.agentType);
      }
      await archiveSession(id);
    } catch (err) {
      showToast(`归档失败: ${err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err)}`, 'error');
    }
  };

  const handleRename = async (id: string, newTitle: string) => {
    try {
      await renameSession(id, newTitle);
    } catch (err) {
      showToast(`重命名失败: ${err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err)}`, 'error');
    }
  };

  const handleDelete = async (id: string) => {
    try {
      // Notify Sidecar to close the Agent session
      const session = sessions.find(s => s.id === id) || archivedSessions.find(s => s.id === id);
      if (session && session.agentType !== 'api') {
        wsCloseSession(id, session.agentType);
      }
      await deleteSession(id);
    } catch (err) {
      showToast(`删除失败: ${err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err)}`, 'error');
    }
  };

  const renderGroup = (label: string, items: typeof sessions) => {
    if (items.length === 0) return null;
    return (
      <div key={label}>
        <div className="px-3 py-1 text-[10px] " style={{ color: 'var(--text-secondary)' }}>
          {label}
        </div>
        {items.map((session) => (
          <SessionListItem
            key={session.id}
            session={session}
            isActive={session.id === currentSessionId}
            onSelect={() => selectSession(session.id)}
            onArchive={showArchived ? undefined : () => handleArchive(session.id)}
            onRename={handleRename}
            onDelete={() => handleDelete(session.id)}
            batchMode={batchMode}
            selected={selectedIds.has(session.id)}
            onToggleSelect={toggleBatchSelect}
          />
        ))}
      </div>
    );
  };

  const currentApiProvider = apiProviders.find((p) => p.id === selectedApiProvider);

  return (
    <aside
      className="w-[260px] shrink-0 flex flex-col overflow-hidden"
      style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
    >
      {/* Header */}
      <div className="flex items-center px-3 h-9" style={{ borderBottom: '1px solid var(--border)' }}>
        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>会话</span>
        <div className="flex-1" />
        <div className="flex items-center gap-1">
          <button
            onClick={() => setBatchMode(!batchMode)}
            className="pd-btn pd-btn-sm"
            style={{
              backgroundColor: batchMode ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: batchMode ? '#fff' : 'var(--text-secondary)',
            }}
            title="批量操作"
          >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></svg>
            批量
          </button>
          <button
            onClick={toggleArchived}
            className="pd-btn pd-btn-sm"
            style={{ color: showArchived ? 'var(--accent)' : 'var(--text-secondary)', backgroundColor: showArchived ? 'var(--accent-light)' : 'var(--bg-tertiary)' }}
            title="归档"
          >
            <Archive size={11} />
            归档
          </button>
          <button
            onClick={() => setShowNewDialog(true)}
            className="pd-btn pd-btn-sm pd-btn-primary"
            title="新建会话"
          >
            <Plus size={11} />
            新建
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
            title="全选"
          >
            全
          </button>
          <button
            onClick={deselectAll}
            className="pd-btn pd-btn-sm"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="取消全选"
          >
            重
          </button>
          <div className="w-px h-4 mx-0.5" style={{ backgroundColor: 'var(--border)' }} />
          <button
            onClick={batchArchive}
            className="pd-btn pd-btn-sm pd-btn-info"
            disabled={selectedIds.size === 0}
            title="批量归档"
          >
            档({selectedIds.size})
          </button>
          <button
            onClick={batchDelete}
            className="pd-btn pd-btn-sm pd-btn-danger"
            disabled={selectedIds.size === 0}
            title="批量删除"
          >
            删({selectedIds.size})
          </button>
          <div className="flex-1" />
          <button
            onClick={() => { setBatchMode(false); setSelectedIds(new Set()); }}
            className="pd-btn pd-btn-sm"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="退出批量模式"
          >
            退
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
            placeholder="搜索会话..."
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
        <div className="flex-1 overflow-y-auto">
          {filteredList.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full gap-2 px-4">
              <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                {showArchived ? '暂无归档会话' : '暂无会话'}
              </p>
              {!showArchived && (
                <button
                  onClick={() => setShowNewDialog(true)}
                  className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors"
                  style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                >
                  <Plus size={12} />
                  新建会话
                </button>
              )}
            </div>
          ) : (
            <>
              {renderGroup('今天', today)}
              {renderGroup('昨天', yesterday)}
              {renderGroup('更早', earlier)}
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
            className="w-[420px] rounded-xl p-4"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
          >
            <h3 className="text-sm font-medium mb-3" style={{ color: 'var(--text-primary)' }}>新建会话</h3>

            {/* Session type selector */}
            <div className="flex gap-2 mb-4">
              {([
                { type: 'claude' as const, ...AGENT_THEMES.claude },
                { type: 'hermes' as const, ...AGENT_THEMES.hermes },
                { type: 'codex' as const, ...AGENT_THEMES.codex },
                { type: 'api' as const, ...AGENT_THEMES.api },
              ]).filter(({ type }) => {
                // API mode always visible; agent types only if installed or still loading
                if (type === 'api') return true;
                if (envLoading) return true;
                return installedAgents.has(type);
              }).map(({ type, color, label }) => (
                <button
                  key={type}
                  onClick={() => setNewSessionType(type)}
                  className="flex-1 py-2 rounded-lg text-xs  transition-colors flex items-center justify-center gap-1"
                  style={{
                    backgroundColor: newSessionType === type ? `${color}15` : 'var(--bg-secondary)',
                    color: newSessionType === type ? color : 'var(--text-secondary)',
                    border: `1px solid ${newSessionType === type ? color : 'var(--border)'}`,
                  }}
                >
                  {type === 'api' && <Key size={12} />}
                  {label}
                </button>
              ))}
            </div>

            {/* Agent sessions: optional title & cwd */}
            {newSessionType !== 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    会话标题（可选）
                  </label>
                  <input
                    type="text"
                    value={customTitle}
                    onChange={(e) => setCustomTitle(e.target.value)}
                    placeholder={newSessionType === 'claude' ? 'Claude Code 新会话' : newSessionType === 'hermes' ? 'Hermes Agent 新会话' : 'codeX 新会话'}
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    工作目录（可选）
                  </label>
                  <input
                    type="text"
                    value={customCwd}
                    onChange={(e) => setCustomCwd(e.target.value)}
                    placeholder="留空使用默认目录"
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
              </div>
            )}

            {/* API direct: provider & model selection */}
            {newSessionType === 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    API 提供商
                  </label>
                  {apiProviders.length === 0 ? (
                    <div
                      className="px-3 py-2 rounded-lg text-xs"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
                    >
                      暂无配置的 API 提供商，请先在「设置 - API 配置」中添加
                    </div>
                  ) : (
                    <select
                      value={selectedApiProvider}
                      onChange={(e) => setSelectedApiProvider(e.target.value)}
                      className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                      style={{
                        backgroundColor: 'var(--bg-tertiary)',
                        color: 'var(--text-primary)',
                        border: '1px solid var(--border)',
                      }}
                    >
                      {apiProviders.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name}{p.apiKeySet ? ' (已配置)' : ' (未配置Key)'}
                        </option>
                      ))}
                    </select>
                  )}
                </div>

                {currentApiProvider && (
                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-xs " style={{ color: 'var(--text-secondary)' }}>
                        模型
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
                        自定义
                      </button>
                    </div>
                    {useCustomModel ? (
                      <input
                        type="text"
                        value={customModel}
                        onChange={(e) => setCustomModel(e.target.value)}
                        placeholder="输入模型名称，如: gpt-4o-2024-08-06"
                        className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                        style={{
                          backgroundColor: 'var(--bg-tertiary)',
                          color: 'var(--text-primary)',
                          border: '1px solid var(--border)',
                        }}
                        autoFocus
                      />
                    ) : currentApiProvider.models.length > 0 ? (
                      <select
                        value={selectedApiModel}
                        onChange={(e) => setSelectedApiModel(e.target.value)}
                        className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                        style={{
                          backgroundColor: 'var(--bg-tertiary)',
                          color: 'var(--text-primary)',
                          border: '1px solid var(--border)',
                        }}
                      >
                        {currentApiProvider.models.map((m) => (
                          <option key={m} value={m}>{m}</option>
                        ))}
                      </select>
                    ) : (
                      <div
                        className="px-3 py-2 rounded-lg text-xs"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
                      >
                        该提供商暂无预定义模型，请使用"自定义"输入模型名称
                      </div>
                    )}
                  </div>
                )}

                <div>
                  <label className="block text-xs  mb-1" style={{ color: 'var(--text-secondary)' }}>
                    会话标题（可选）
                  </label>
                  <input
                    type="text"
                    value={customTitle}
                    onChange={(e) => setCustomTitle(e.target.value)}
                    placeholder={`API: ${currentApiProvider?.name || ''} - ${useCustomModel ? customModel || '自定义模型' : selectedApiModel || '模型'}`}
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
                <div
                  className="px-3 py-2 rounded-lg text-xs"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                >
                  通过 API 直接与模型对话，无需 Agent 中转。请确保已在设置中配置对应 API Key。
                </div>
              </div>
            )}

            <div className="flex justify-end gap-2">
              <button
                onClick={() => setShowNewDialog(false)}
                className="px-3 py-1.5 rounded-lg text-xs"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
              >
                取消
              </button>
              <button
                onClick={handleCreate}
                disabled={creating || (newSessionType === 'api' && apiProviders.length === 0)}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs  disabled:opacity-50"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                {creating ? '创建中...' : '创建'}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}

export const SessionList = memo(SessionListFn);

