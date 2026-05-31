import { useState, useEffect, useCallback, useRef } from 'react';
import { Search, Plus, Star, ArrowLeft } from 'lucide-react';
import { useInspirationStore, type InspirationItem } from '../../stores/inspirationStore';
import { useSessionStore } from '../../stores/sessionStore';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { InspirationCard } from './InspirationCard';
import { InspirationForm } from './InspirationForm';
import { TagFilter } from './TagFilter';

interface MarketPageProps {
  onBack: () => void;
}

export function MarketPage({ onBack }: MarketPageProps) {
  const {
    inspirations,
    loading,
    error,
    searchQuery,
    favoriteOnly,
    fetchInspirations,
    fetchTags,
    searchInspirations,
    createInspiration,
    updateInspiration,
    deleteInspiration,
    toggleFavorite,
    setSearchQuery,
    setFavoriteOnly,
  } = useInspirationStore();

  const [showForm, setShowForm] = useState(false);
  const [editingInspiration, setEditingInspiration] = useState<InspirationItem | null>(null);
  const [prefillContent, setPrefillContent] = useState('');
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  useEffect(() => {
    fetchInspirations();
    fetchTags();
  }, [fetchInspirations, fetchTags]);

  const handleSearch = useCallback(
    (query: string) => {
      setSearchQuery(query);
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        if (query.trim()) {
          searchInspirations(query);
        } else {
          fetchInspirations();
        }
      }, 300);
    },
    [setSearchQuery, searchInspirations, fetchInspirations]
  );

  const handleSendToSession = useCallback(
    (content: string) => {
      usePendingInputStore.getState().set(content);
    },
    []
  );

  const handleEdit = (insp: InspirationItem) => {
    setEditingInspiration(insp);
    setShowForm(false);
  };

  const handleSaveNew = async (data: Parameters<typeof createInspiration>[0]) => {
    await createInspiration(data);
    setShowForm(false);
  };

  const handleUpdate = async (data: Parameters<typeof updateInspiration>[0]) => {
    await updateInspiration(data);
    setEditingInspiration(null);
  };

  const handleDelete = async (id: string) => {
    if (confirm('确定要删除这条灵感吗？')) {
      await deleteInspiration(id);
    }
  };

  const openNewForm = () => {
    setEditingInspiration(null);
    setPrefillContent('');
    setShowForm(true);
  };

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* Header */}
      <div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-3">
          <button onClick={onBack} className="p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
            <ArrowLeft size={16} />
          </button>
          <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>灵感市集</h2>
          <span className="text-xs px-2 py-0.5 rounded-full" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
            {inspirations.length}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setFavoriteOnly(!favoriteOnly)}
            className="p-1.5 rounded-lg transition-colors"
            style={{
              backgroundColor: favoriteOnly ? '#FBBF2422' : 'var(--bg-tertiary)',
              color: favoriteOnly ? '#FBBF24' : 'var(--text-secondary)',
            }}
          >
            <Star size={14} fill={favoriteOnly ? '#FBBF24' : 'none'} />
          </button>
          <button
            onClick={openNewForm}
            className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={12} />
            新建
          </button>
        </div>
      </div>

      {/* Search + Tags */}
      <div className="shrink-0 px-4 py-2 space-y-2" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="relative">
          <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => handleSearch(e.target.value)}
            placeholder="搜索灵感..."
            className="w-full pl-9 pr-3 py-1.5 rounded-lg text-xs outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
        </div>
        <TagFilter />
      </div>

      {/* Grid */}
      <div className="flex-1 overflow-y-auto p-4">
        {error && (
          <div className="mb-3 px-3 py-2 rounded-lg text-xs" style={{ backgroundColor: 'var(--danger-bg)', color: 'var(--danger)' }}>
            {error}
          </div>
        )}

        {loading && !inspirations.length ? (
          <div className="flex items-center justify-center h-32">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : inspirations.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-32 gap-2">
            <span className="text-3xl">💡</span>
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配的灵感' : '还没有灵感，点击新建开始收集'}
            </p>
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {inspirations.map((insp) => (
              <InspirationCard
                key={insp.id}
                inspiration={insp}
                onToggleFavorite={toggleFavorite}
                onSendToSession={handleSendToSession}
                onDelete={handleDelete}
                onEdit={handleEdit}
              />
            ))}
          </div>
        )}
      </div>

      {/* Form Modal */}
      {(showForm || editingInspiration) && (
        <InspirationForm
          initialData={editingInspiration}
          prefill={prefillContent}
          sourceAgent={currentSession?.agentType}
          onSave={handleSaveNew}
          onUpdate={handleUpdate}
          onCancel={() => {
            setShowForm(false);
            setEditingInspiration(null);
            setPrefillContent('');
          }}
        />
      )}
    </div>
  );
}
