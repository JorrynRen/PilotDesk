import { useState, useEffect } from 'react';
import { Plus, Search, Archive, Key } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
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
    toggleArchived,
  } = useSessionStore();

  const [searchQuery, setSearchQuery] = useState('');
  const [showNewDialog, setShowNewDialog] = useState(false);
  const [newSessionType, setNewSessionType] = useState<NewSessionType>('claude');
  const [selectedApiProvider, setSelectedApiProvider] = useState(API_PROVIDERS[0].id);
  const [selectedApiModel, setSelectedApiModel] = useState(API_PROVIDERS[0].models[0]);
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    fetchSessions().catch((err) => {
      showToast(`加载会话失败: ${err}`, 'error');
    });
  }, [fetchSessions]);

  // Update model when provider changes
  useEffect(() => {
    const provider = API_PROVIDERS.find((p) => p.id === selectedApiProvider);
    if (provider) {
      setSelectedApiModel(provider.models[0]);
    }
  }, [selectedApiProvider]);

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
        const maskedKey = localStorage.getItem(`pd-api-${selectedApiProvider}-masked`);
        if (!maskedKey) {
          showToast('请先在设置 > API 配置中添加 API Key', 'error');
          setCreating(false);
          return;
        }
        // Create API direct session
        const provider = API_PROVIDERS.find((p) => p.id === selectedApiProvider)!;
        const session = await createSession(
          'api' as 'claude',
          undefined,
          `API: ${provider.name} - ${selectedApiModel}`,
          selectedApiProvider,
          selectedApiModel,
        );
        setShowNewDialog(false);
        selectSession(session.id);
      } else {
        const session = await createSession(newSessionType);
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
      await archiveSession(id);
    } catch (err) {
      showToast(`归档失败: ${err}`, 'error');
    }
  };

  const handleDelete = async (id: string) => {
    try {
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
            onDelete={() => handleDelete(session.id)}
          />
        ))}
      </div>
    );
  };

  // Current API provider's models
  const currentApiProvider = API_PROVIDERS.find((p) => p.id === selectedApiProvider)!;

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
            className="w-[400px] rounded-xl p-4"
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

            {/* API direct: provider & model selection */}
            {newSessionType === 'api' && (
              <div className="space-y-3 mb-4">
                <div>
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
                    API 提供商
                  </label>
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
                    {API_PROVIDERS.map((p) => (
                      <option key={p.id} value={p.id}>{p.name}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
                    模型
                  </label>
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
                disabled={creating}
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
