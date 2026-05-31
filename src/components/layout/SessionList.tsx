import { useState, useEffect } from 'react';
import { Plus, Search, Archive } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
import { SessionListItem } from './SessionListItem';
import { showToast } from '../../utils/toast';

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
  const [newAgentType, setNewAgentType] = useState<'claude' | 'hermes'>('claude');
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    fetchSessions().catch((err) => {
      showToast(`加载会话失败: ${err}`, 'error');
    });
  }, [fetchSessions]);

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
      const session = await createSession(newAgentType);
      setShowNewDialog(false);
      selectSession(session.id);
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
            className="w-[320px] rounded-xl p-4"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>新建会话</h3>
            <div className="flex gap-2 mb-4">
              {(['claude', 'hermes'] as const).map((type) => (
                <button
                  key={type}
                  onClick={() => setNewAgentType(type)}
                  className="flex-1 py-2 rounded-lg text-xs font-medium transition-colors"
                  style={{
                    backgroundColor: newAgentType === type
                      ? (type === 'claude' ? 'rgba(59,130,246,0.15)' : 'rgba(139,92,246,0.15)')
                      : 'var(--bg-secondary)',
                    color: newAgentType === type
                      ? (type === 'claude' ? '#3B82F6' : '#8B5CF6')
                      : 'var(--text-secondary)',
                    border: `1px solid ${newAgentType === type ? (type === 'claude' ? '#3B82F6' : '#8B5CF6') : 'var(--border)'}`,
                  }}
                >
                  {type === 'claude' ? 'Claude Code' : 'Hermes Agent'}
                </button>
              ))}
            </div>
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
