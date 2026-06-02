import { useState, useEffect } from 'react';
import { Plus, Search, Archive, Key, ChevronDown, X } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
import { useApiProviderStore, getApiKey } from '../../stores/apiProviderStore';
import { useWebSocket } from '../../hooks/useWebSocket';
import { SessionListItem } from './SessionListItem';
import { showToast } from '../../utils/toast';
import { API_PROVIDERS } from '../../types';

type NewSessionType = 'claude' | 'hermes' | 'api';

export function SessionList() {
  const {
    sessions,
    archivedSessions,
    currentSessionId,
    isLoadingSessions,
    showArchived,
    fetchSessions,
    selectSession,
    createSession,
    archiveSession,
    deleteSession,
    renameSession,
    toggleArchived,
  } = useSessionStore();

  const [searchQuery, setSearchQuery] = useState('');
  const [showNewDialog, setShowNewDialog] = useState(false);
  const [newSessionType, setNewSessionType] = useState<NewSessionType>('claude');
  const [selectedApiProvider, setSelectedApiProvider] = useState('');
  const [selectedApiModel, setSelectedApiModel] = useState('');
  const [customModel, setCustomModel] = useState('');
  const [useCustomModel, setUseCustomModel] = useState(false);
  const [customTitle, setCustomTitle] = useState('');
  const [customCwd, setCustomCwd] = useState('');
  const [creating, setCreating] = useState(false);

  // WebSocket for Agent session lifecycle
  const { createAgentSession: wsCreateSession, closeAgentSession: wsCloseSession } = useWebSocket(19830);

  // API providers from SQLite via store
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();

  useEffect(() => {
    fetchSessions().catch((err) => {
      showToast(`加载会话失败: ${err}`, 'error');
    });
  }, [fetchSessions]);

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

  const displayList = showArchived ? archivedSessions : sessions;
  const filteredList = displayList.filter((s) =>
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
      showToast(`创建会话失败: ${err}`, 'error');
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
      showToast(`归档失败: ${err}`, 'error');
    }
  };

  const handleRename = async (id: string, newTitle: string) => {
    try {
      await renameSession(id, newTitle);
    } catch (err) {
      showToast(`重命名失败: ${err}`, 'error');
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
      showToast(`删除失败: ${err}`, 'error');
    }
  };

  const renderGroup = (label: string, items: typeof sessions) => {
    if (items.length === 0) return null;
    return (
      <div key={label}>
        <div className="px-3 py-1 text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>
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
          />
        ))}
      </div>
    );
  };

  const currentApiProvider = apiProviders.find((p) => p.id === selectedApiProvider);

  return (
    <aside
      className="w-[260px] shrink-0 flex flex-col overflow-hidden"
      style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-panel)' }}
    >
      {/* Header */}
      <div className="px-3 py-2 flex items-center justify-between" style={{ borderBottom: '1px solid var(--border)' }}>
        <span className="text-xs font-semibold" style={{ color: 'var(--text-primary)' }}>会话</span>
        <div className="flex items-center gap-1">
          <button
            onClick={toggleArchived}
            className="p-1 rounded transition-colors"
            style={{ color: showArchived ? 'var(--accent)' : 'var(--text-secondary)' }}
            title="归档"
          >
            <Archive size={14} />
          </button>
          <button
            onClick={() => setShowNewDialog(true)}
            className="p-1 rounded transition-colors"
            style={{ color: 'var(--accent)' }}
            title="新建会话"
          >
            <Plus size={14} />
          </button>
        </div>
      </div>

      {/* Search */}
      <div className="px-3 py-1.5">
        <div className="flex items-center gap-1.5 px-2 py-1 rounded-md text-xs" style={{ backgroundColor: 'var(--bg-secondary)' }}>
          <Search size={12} style={{ color: 'var(--text-secondary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索会话..."
            className="flex-1 bg-transparent outline-none text-xs"
            style={{ color: 'var(--text-primary)' }}
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
            <h3 className="text-sm font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>新建会话</h3>

            {/* Session type selector */}
            <div className="flex gap-2 mb-4">
              {([
                { type: 'claude' as const, label: 'Claude Code', color: '#3B82F6' },
                { type: 'hermes' as const, label: 'Hermes Agent', color: '#8B5CF6' },
                { type: 'api' as const, label: 'API 直连', color: '#10B981' },
              ]).map(({ type, label, color }) => (
                <button
                  key={type}
                  onClick={() => setNewSessionType(type)}
                  className="flex-1 py-2 rounded-lg text-xs font-medium transition-colors flex items-center justify-center gap-1"
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

            {/* Claude / Hermes: optional title & cwd */}
            {newSessionType !== 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
                    会话标题（可选）
                  </label>
                  <input
                    type="text"
                    value={customTitle}
                    onChange={(e) => setCustomTitle(e.target.value)}
                    placeholder={newSessionType === 'claude' ? 'Claude Code 新会话' : 'Hermes Agent 新会话'}
                    className="w-full px-3 py-2 rounded-lg text-sm outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
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
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
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
                      <label className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
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
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
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
                className="px-3 py-1.5 rounded-lg text-xs font-medium disabled:opacity-50"
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
