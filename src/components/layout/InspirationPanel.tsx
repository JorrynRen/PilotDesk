import { useState, useEffect, useCallback } from 'react';
import { Plus, Star, Search, Send, Trash2, Edit3, X } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
import { useInspirationStore, type InspirationItem } from '../../stores/inspirationStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { useTerminal, injectToActiveTerminal } from '../../TerminalManager';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { TagChips } from '../inspiration/TagChips';
import { TagEditor } from '../inspiration/TagEditor';
import { TagFilterBar } from '../inspiration/TagFilterBar';

import { EMOJI_OPTIONS } from '../../constants';

export function InspirationPanel() {
  const { viewMode } = useTerminal();
  const isTerminalMode = viewMode === 'terminal';

  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  const {
    inspirations,
    tags,
    loading,
    fetchInspirations,
    fetchTags,
    createInspiration,
    updateInspiration,
    deleteInspiration,
    toggleFavorite,
  } = useInspirationStore();

  const [searchQuery, setSearchQuery] = useState('');
  const [favoriteOnly, setFavoriteOnly] = useState(false);
  /**
   * 标签筛选用**本地 state**，不复用 store 的 activeTag：
   * 那个是独立「灵感库」页的筛选状态（会触发后端按 tag 过滤），共用会导致
   * "在侧栏点了个标签，切到灵感库页发现筛选被改了"。侧栏只在自己这 300px 内过滤。
   */
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [editingInspiration, setEditingInspiration] = useState<InspirationItem | null>(null);
  useEffect(() => {
    fetchInspirations();
    // 标签全集：侧栏的筛选条要用（只读，不碰 store 的 activeTag）
    fetchTags();
  }, [fetchInspirations, fetchTags]);

  // Filter inspirations locally
  const filteredInspirations = inspirations.filter((insp) => {
    if (favoriteOnly && !insp.isFavorite) return false;
    if (activeTag && !insp.tags.includes(activeTag)) return false;
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      return (
        insp.title.toLowerCase().includes(q) ||
        insp.content.toLowerCase().includes(q)
      );
    }
    return true;
  });

  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query);
  }, []);

  const handleDelete = async (id: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: '确定要删除这条灵感吗？',
      confirmText: '删除',
    });
    if (ok) {
      await deleteInspiration(id);
    }
  };

  const handleSendToSession = (content: string) => {
    // 终端模式：注入到当前活跃终端输入点（term.paste）；无终端时提示先新建
    if (isTerminalMode) {
      if (injectToActiveTerminal(content)) {
        showToast('已发送到终端输入点', 'success');
      } else {
        showToast('请先新建终端，再发送到终端', 'warning');
      }
      return;
    }
    // 会话模式：写入共享 store，由会话输入框消费
    usePendingInputStore.getState().set(content);
  };

  const handleSave = async (data: {
    icon?: string;
    title: string;
    content: string;
    sourceAgent?: string;
    tags?: string[];
  }) => {
    if (editingInspiration) {
      await updateInspiration({ ...data, id: editingInspiration.id });
    } else {
      await createInspiration(data);
    }
    setShowForm(false);
    setEditingInspiration(null);
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Toolbar: search + actions */}
      <div className="shrink-0 px-3 py-2 space-y-2" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-1.5">
          {/* Search */}
          <div className="relative flex-1">
            <Search size={12} className="absolute left-2 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => handleSearch(e.target.value)}
              placeholder="搜索灵感..."
              className="w-full pl-7 pr-2 py-1 rounded-md text-[11px] outline-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
          </div>
          {/* Favorite toggle */}
          <button
            onClick={() => setFavoriteOnly(!favoriteOnly)}
            className="p-1 rounded-md transition-colors shrink-0"
            title="收藏筛选"
            style={{
              backgroundColor: favoriteOnly ? '#FBBF2422' : 'var(--bg-tertiary)',
              color: favoriteOnly ? '#FBBF24' : 'var(--text-secondary)',
            }}
          >
            <Star size={12} fill={favoriteOnly ? '#FBBF24' : 'none'} />
          </button>
          {/* New button */}
          <button
            onClick={() => {
              setEditingInspiration(null);
              setShowForm(true);
            }}
            className="flex items-center gap-0.5 px-2 py-1 rounded-md text-[11px]  shrink-0 transition-colors"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            title="新建灵感"
          >
            <Plus size={11} />
            新建
          </button>
        </div>
        {/* 标签筛选：与独立页同一套外观（紧凑档 = 单行横向滚动，适配侧栏宽度） */}
        <TagFilterBar tags={tags} activeTag={activeTag} onSelect={setActiveTag} compact />

        <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          {filteredInspirations.length} 条灵感
        </div>
      </div>

      {/* Form (inline) */}
      {showForm && (
        <InspirationInlineForm
          initialData={editingInspiration}
          sourceAgent={currentSession?.agentType}
          onSave={handleSave}
          onCancel={() => {
            setShowForm(false);
            setEditingInspiration(null);
          }}
        />
      )}

      {/* List */}
      <div className="flex-1 overflow-y-auto px-3 py-2 space-y-2 pd-scroll-stable">
        {loading && !inspirations.length ? (
          <div className="flex items-center justify-center h-20">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : filteredInspirations.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-20 gap-1.5">
            <span className="text-lg">💡</span>
            <p className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配的灵感' : '还没有灵感，点击新建开始收集'}
            </p>
          </div>
        ) : (
          filteredInspirations.map((insp) => (
            <InspirationRow
              key={insp.id}
              inspiration={insp}
              onToggleFavorite={toggleFavorite}
              onSendToSession={handleSendToSession}
              onDelete={handleDelete}
              onEdit={(insp) => {
                setEditingInspiration(insp);
                setShowForm(true);
              }}
              sendLabel={isTerminalMode ? '发送到终端' : '发送到会话'}
            />
          ))
        )}
      </div>
    </div>
  );
}

/* ─── Compact Inspiration Row ─── */

interface InspirationRowProps {
  inspiration: InspirationItem;
  onToggleFavorite: (id: string) => void;
  onSendToSession: (content: string) => void;
  onDelete: (id: string) => void;
  onEdit: (insp: InspirationItem) => void;
  /** 发送按钮文案：会话模式「发送到会话」，终端模式「发送到终端」 */
  sendLabel: string;
}

function InspirationRow({ inspiration, onToggleFavorite, onSendToSession, onDelete, onEdit, sendLabel }: InspirationRowProps) {
  const { getTheme } = useAgentRegistry();
  const sourceTheme = getTheme(inspiration.sourceAgent ?? '');
  const sourceLabel = sourceTheme?.label ?? inspiration.sourceAgent;
  const sourceColor = sourceTheme?.color ?? '#6B7280';

  return (
    <div
      className="group rounded-lg p-2 transition-colors"
      style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
    >
      {/* Header */}
      <div className="flex items-start gap-1.5 mb-1">
        <span className="text-sm leading-none shrink-0">{inspiration.icon || '💡'}</span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1">
            <span className="text-[11px]  truncate" style={{ color: 'var(--text-primary)' }}>
              {inspiration.title}
            </span>
            <span
              className="text-[9px] px-1.5 py-0.5 rounded shrink-0 font-medium"
              style={{
                backgroundColor: sourceColor,
                color: '#fff',
                opacity: 0.85,
              }}
            >
              {sourceLabel ?? inspiration.sourceAgent}
            </span>
          </div>
        </div>
        {/* Actions (show on hover) */}
        <div className="flex items-center gap-0.5 shrink-0">
          <button
            onClick={() => onToggleFavorite(inspiration.id)}
            className="p-0.5 rounded transition-colors hover:bg-black/5"
            title="收藏"
          >
            <Star size={11} fill={inspiration.isFavorite ? '#FBBF24' : 'none'} style={{ color: inspiration.isFavorite ? '#FBBF24' : 'var(--text-tertiary)' }} />
          </button>
          <button
            onClick={() => onEdit(inspiration)}
            className="p-0.5 rounded transition-colors hover:bg-black/5"
            title="编辑"
          >
            <Edit3 size={11} style={{ color: 'var(--text-secondary)' }} />
          </button>
          <button
            onClick={() => onDelete(inspiration.id)}
            className="p-0.5 rounded transition-colors hover:bg-red-50"
            title="删除"
          >
            <Trash2 size={11} style={{ color: 'var(--text-tertiary)' }} />
          </button>
        </div>
      </div>
      {/* Content preview */}
      <p className="text-[10px] leading-relaxed mb-1.5" style={{ color: 'var(--text-secondary)', display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>
        {inspiration.content}
      </p>
      {/* Tags：紧凑档 + 最多 4 个（侧栏窄，超出折叠成 +N） */}
      <TagChips tags={inspiration.tags} compact max={4} className="mb-1.5" />
      {/* Send to session */}
      <button
        onClick={() => onSendToSession(inspiration.content)}
        className="flex items-center gap-1 text-[10px] transition-colors"
        style={{ color: 'var(--accent)' }}
      >
        <Send size={10} />
        {sendLabel}
      </button>
    </div>
  );
}

/* ─── Inline Creation / Edit Form ─── */

interface InspirationInlineFormProps {
  initialData?: InspirationItem | null;
  sourceAgent?: string;
  onSave: (data: { icon?: string; title: string; content: string; sourceAgent?: string; tags?: string[] }) => Promise<void>;
  onCancel: () => void;
}

function InspirationInlineForm({ initialData, sourceAgent, onSave, onCancel }: InspirationInlineFormProps) {
  const [icon, setIcon] = useState(initialData?.icon || '💡');
  const [title, setTitle] = useState(initialData?.title || '');
  const [content, setContent] = useState(initialData?.content || '');
  // 标签可编辑：此前只在保存时原样回传 initialData.tags，等于"看得见改不了"
  const [tags, setTags] = useState<string[]>(initialData?.tags ?? []);
  const [saving, setSaving] = useState(false);

  const handleSave = async () => {
    if (!title.trim() || !content.trim()) return;
    setSaving(true);
    try {
      await onSave({
        icon,
        title: title.trim(),
        content: content.trim(),
        tags,
        // 编辑时不动来源：来源是"这条从哪来"的记录（用户自建 manual / 从市场导入 market /
        // 某个 Agent 生成的），不该被一次编辑动作改写成"当前会话的 Agent"。
        // 传 undefined 时后端保留原值（update_inspiration 对 None 走 unwrap_or(existing)）。
        sourceAgent: initialData ? undefined : (sourceAgent || 'manual'),
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="shrink-0 px-3 py-2 space-y-2" style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-tertiary)' }}>
      {/* Icon picker */}
      <div>
        <span className="text-[10px] mb-1 block" style={{ color: 'var(--text-tertiary)' }}>图标:</span>
        <div className="grid grid-cols-12 gap-0.5">
          {EMOJI_OPTIONS.map((emoji) => (
            <button
              key={emoji}
              type="button"
              onClick={() => setIcon(emoji)}
              className="text-sm p-0.5 rounded transition-colors text-center"
              style={{
                backgroundColor: icon === emoji ? 'var(--border)' : 'transparent',
                outline: icon === emoji ? '1.5px solid var(--accent)' : 'none',
                outlineOffset: '-1px',
              }}
            >
              {emoji}
            </button>
          ))}
        </div>
      </div>
      {/* Title */}
      <input
        type="text"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        placeholder="灵感标题"
        className="w-full px-2 py-1 rounded-md text-[11px] outline-none"
        style={{
          backgroundColor: 'var(--bg-primary)',
          color: 'var(--text-primary)',
          border: '1px solid var(--border)',
        }}
        autoFocus
      />
      {/* Content */}
      <textarea
        value={content}
        onChange={(e) => setContent(e.target.value)}
        placeholder="灵感prompt..."
        rows={3}
        className="w-full px-2 py-1 rounded-md text-[11px] outline-none resize-none"
        style={{
          backgroundColor: 'var(--bg-primary)',
          color: 'var(--text-primary)',
          border: '1px solid var(--border)',
        }}
      />
      {/* Tags */}
      <div>
        <span className="text-[10px] mb-1 block" style={{ color: 'var(--text-tertiary)' }}>标签:</span>
        <TagEditor tags={tags} onChange={setTags} compact />
      </div>
      {/* Actions */}
      <div className="flex items-center justify-end gap-1.5">
        <button
          onClick={onCancel}
          className="pd-btn flex items-center gap-0.5 px-2 py-1 rounded-md text-[11px] transition-colors"
          style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-primary)' }}
        >
          <X size={10} />
          取消
        </button>
        <button
          onClick={handleSave}
          disabled={!title.trim() || !content.trim() || saving}
          className="pd-btn flex items-center gap-0.5 px-2 py-1 rounded-md text-[11px]  transition-colors disabled:opacity-40"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          {saving ? '保存中...' : initialData ? '更新' : '保存'}
        </button>
      </div>
    </div>
  );
}
