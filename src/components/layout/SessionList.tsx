import { useState } from 'react';
import { Plus, Search, Archive } from 'lucide-react';

export function SessionList() {
  const [searchQuery, setSearchQuery] = useState('');
  const [showArchived, setShowArchived] = useState(false);

  return (
    <aside
      className="w-[260px] flex flex-col shrink-0"
      style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
    >
      {/* Header */}
      <div className="p-3 flex items-center justify-between" style={{ borderBottom: '1px solid var(--border)' }}>
        <span className="text-xs font-semibold uppercase tracking-wider" style={{ color: 'var(--text-secondary)' }}>
          会话列表
        </span>
        <button
          className="p-1 rounded transition-colors hover:opacity-80"
          style={{ color: 'var(--accent)' }}
          title="新建会话"
        >
          <Plus size={16} />
        </button>
      </div>

      {/* Search */}
      <div className="px-3 py-2">
        <div
          className="flex items-center gap-2 px-2 py-1.5 rounded-md text-xs"
          style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        >
          <Search size={12} style={{ color: 'var(--text-secondary)' }} />
          <input
            type="text"
            placeholder="搜索会话..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="bg-transparent outline-none flex-1 text-xs"
            style={{ color: 'var(--text-primary)' }}
          />
        </div>
      </div>

      {/* Filter */}
      <div className="px-3 pb-2 flex gap-1">
        <button
          onClick={() => setShowArchived(false)}
          className="px-2 py-0.5 rounded text-xs transition-colors"
          style={{
            backgroundColor: !showArchived ? 'var(--accent)' : 'transparent',
            color: !showArchived ? '#fff' : 'var(--text-secondary)',
          }}
        >
          活跃
        </button>
        <button
          onClick={() => setShowArchived(true)}
          className="px-2 py-0.5 rounded text-xs transition-colors flex items-center gap-1"
          style={{
            backgroundColor: showArchived ? 'var(--accent)' : 'transparent',
            color: showArchived ? '#fff' : 'var(--text-secondary)',
          }}
        >
          <Archive size={10} />
          归档
        </button>
      </div>

      {/* Session Items Placeholder */}
      <div className="flex-1 flex items-center justify-center">
        <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
          暂无会话
        </p>
      </div>
    </aside>
  );
}
