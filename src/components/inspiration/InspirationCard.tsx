import { useEffect, useRef, useState } from 'react';
import { MoreHorizontal, Send, Star, Trash2, Upload } from 'lucide-react';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import type { InspirationItem } from '../../stores/inspirationStore';
import { elide } from '../../utils/text';
import { TagChips } from './TagChips';

interface InspirationCardProps {
  inspiration: InspirationItem;
  onToggleFavorite: (id: string) => void;
  onSendToSession: (content: string) => void;
  onDelete: (id: string) => void;
  onEdit: (inspiration: InspirationItem) => void;
  /** 导出为独立 JSON 文件（备份、分享或兼容市场格式） */
  onExport: (inspiration: InspirationItem) => void;
}

export function InspirationCard({ inspiration, onToggleFavorite, onSendToSession, onDelete, onEdit, onExport }: InspirationCardProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const { getTheme } = useAgentRegistry();
  const sourceTheme = getTheme(inspiration.sourceAgent);
  const sourceLabel = sourceTheme.label;
  const sourceColor = sourceTheme.color;

  const timeStr = new Date(inspiration.updatedAt * 1000).toLocaleDateString('zh-CN', {
    month: 'short',
    day: 'numeric',
  });
  const fullTime = new Date(inspiration.updatedAt * 1000).toLocaleString('zh-CN');

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [menuOpen]);

  return (
    <div
      className="pd-inspiration-card"
      style={{
        backgroundColor: 'var(--bg-raised)',
      }}
      onClick={() => onEdit(inspiration)}
      onKeyDown={(event) => {
        if (event.target === event.currentTarget && (event.key === 'Enter' || event.key === ' ')) {
          event.preventDefault();
          onEdit(inspiration);
        }
      }}
      role="button"
      tabIndex={0}
      aria-label={`编辑灵感：${inspiration.title}`}
    >
      {/* Header */}
      <div className="pd-inspiration-card-header">
        <span className="pd-inspiration-card-icon">{inspiration.icon}</span>
        <div className="pd-inspiration-card-heading">
          <h4 className="pd-inspiration-card-title" title={inspiration.title}>
            {inspiration.title}
          </h4>
          <div className="pd-inspiration-card-meta">
            <span style={{ color: sourceColor, backgroundColor: `color-mix(in srgb, ${sourceColor} 12%, transparent)` }}>
              {sourceLabel}
            </span>
            <span title={fullTime}>{timeStr}</span>
          </div>
        </div>
        <div className="pd-inspiration-card-actions" ref={menuRef}>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onToggleFavorite(inspiration.id);
            }}
            className="pd-inspiration-icon-btn"
            style={{ color: inspiration.isFavorite ? '#FBBF24' : 'var(--text-tertiary)' }}
            title={inspiration.isFavorite ? '取消收藏' : '收藏灵感'}
            aria-label={inspiration.isFavorite ? '取消收藏' : '收藏灵感'}
          >
            <Star size={14} fill={inspiration.isFavorite ? '#FBBF24' : 'none'} />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              setMenuOpen((open) => !open);
            }}
            className="pd-inspiration-icon-btn"
            style={{
              color: menuOpen ? 'var(--accent)' : 'var(--text-tertiary)',
              backgroundColor: menuOpen ? 'var(--accent-light)' : undefined,
            }}
            title="更多操作"
            aria-label="更多操作"
            aria-expanded={menuOpen}
          >
            <MoreHorizontal size={14} />
          </button>
          {menuOpen && (
            <div className="pd-menu pd-inspiration-card-menu" role="menu">
              <button
                type="button"
                role="menuitem"
                className="pd-menu-item"
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuOpen(false);
                  onExport(inspiration);
                }}
              >
                <Upload size={13} />
                JSON 导出
              </button>
              <button
                type="button"
                role="menuitem"
                className="pd-menu-item"
                data-tone="danger"
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuOpen(false);
                  onDelete(inspiration.id);
                }}
              >
                <Trash2 size={13} />
                删除灵感
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Content preview */}
      <p
        className="pd-inspiration-card-content line-clamp-3"
        style={{ color: 'var(--text-secondary)' }}
      >
        {elide(inspiration.content, 220, '...')}
      </p>

      {/* Tags：与市场、会话侧栏共用 TagChips，避免三处各画一套 */}
      <div className="pd-inspiration-card-tags">
        {inspiration.tags?.length ? (
          <TagChips tags={inspiration.tags} max={4} />
        ) : (
          <span>未添加标签</span>
        )}
      </div>

      {/* Footer */}
      <div className="pd-inspiration-card-footer">
        <span className="pd-inspiration-card-updated" title={fullTime}>
          更新于 {timeStr}
        </span>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onSendToSession(inspiration.content);
          }}
          className="pd-inspiration-send-btn"
          title="把灵感内容放入当前会话输入框"
        >
          <Send size={10} />
          发送到会话
        </button>
      </div>
    </div>
  );
}
