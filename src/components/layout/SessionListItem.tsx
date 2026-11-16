import { useState, useEffect, useRef, memo } from 'react';
import { createPortal } from 'react-dom';
import { Archive, ArchiveRestore, Trash2, Pencil, Check, FolderKanban, MoreHorizontal } from 'lucide-react';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { AgentBadge } from '../common/AgentBadge';
import { PROJECT_BADGE_LIST_CLASS, ProjectBadge } from '../project/ProjectPicker';
import { useGeneratingStore } from '../../stores/generatingStore';
import { useApprovalStore, selectPendingCount } from '../../stores/approvalStore';
import type { Session } from '../../types';

interface SessionListItemProps {
  session: Session;
  isActive: boolean;
  onRename?: (id: string, newTitle: string) => void;
  batchMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (id: string) => void;
  /** 当前列表是否为归档视图：决定归档按钮是「归档」还是「取消归档」 */
  archived?: boolean;
  onArchive?: (id: string) => void;
  onDelete?: (id: string) => void;
  onSetProject?: (session: Session) => void;
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();

  const timeStr = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });

  if (isToday) return `今天 ${timeStr}`;
  if (isYesterday) return `昨天 ${timeStr}`;
  return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

export const SessionListItem = memo(function SessionListItem({
  session,
  isActive,
  onRename,
  batchMode,
  selected,
  onToggleSelect,
  archived,
  onArchive,
  onDelete,
  onSetProject,
}: SessionListItemProps) {
  const [isEditing, setIsEditing] = useState(false);
  const [menuPosition, setMenuPosition] = useState<{ top: number; left: number } | null>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const isGenerating = useGeneratingStore((s) => !!s.generatingMap[session.id]);
  // 待审批徽标：该会话当前待用户决策的工具审批数量（内联审批卡共享同一集合）
  const pendingApprovalCount = useApprovalStore(selectPendingCount(session.id));
  const [editTitle, setEditTitle] = useState(session.title);

  // Fallback display title when title is empty
  const { getTheme } = useAgentRegistry();
  const displayTitle = session.title || (() => {
    const theme = getTheme(session.agentType);
    return `${theme.label} 新会话`;
  })();

  const handleStartEdit = () => {
    setEditTitle(session.title);
    setIsEditing(true);
  };

  const handleConfirmEdit = () => {
    const trimmed = editTitle.trim();
    if (!trimmed) return;
    if (trimmed !== session.title && onRename) {
      onRename(session.id, trimmed);
    }
    setIsEditing(false);
  };

  const handleCancelEdit = () => {
    setIsEditing(false);
    setEditTitle(session.title);
  };

  const closeMenu = () => setMenuPosition(null);

  const openMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    const width = 168;
    const height = onSetProject ? 148 : 116;
    const gap = 4;
    const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
    const top = rect.bottom + gap + height > window.innerHeight - 8
      ? rect.top - height - gap
      : rect.bottom + gap;
    setMenuPosition({ top: Math.max(8, top), left });
  };

  useEffect(() => {
    if (!menuPosition) return;
    const closeOnOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!menuRef.current?.contains(target) && !menuButtonRef.current?.contains(target)) {
        setMenuPosition(null);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuPosition(null);
    };
    const closeOnScroll = () => setMenuPosition(null);
    document.addEventListener('mousedown', closeOnOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('scroll', closeOnScroll, true);
    return () => {
      document.removeEventListener('mousedown', closeOnOutside);
      document.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('scroll', closeOnScroll, true);
    };
  }, [menuPosition]);

  useEffect(() => {
    if (!isEditing) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [isEditing]);

  useEffect(() => {
    if (!isEditing) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setIsEditing(false);
      setEditTitle(session.title);
    };
    document.addEventListener('keydown', closeOnEscape);
    return () => document.removeEventListener('keydown', closeOnEscape);
  }, [isEditing, session.title]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      handleConfirmEdit();
    }
  };

  return (
    <div
      className={`pd-session-item group cursor-pointer transition-colors ${isActive ? 'is-active' : ''} ${selected ? 'is-selected' : ''}`}
      data-session-id={session.id}
      onClick={isEditing ? undefined : batchMode ? (e) => { e.stopPropagation(); onToggleSelect?.(session.id); } : undefined}
    >
      <div className="flex items-center gap-2.5">
        {batchMode && (
          <div className="shrink-0" onClick={(e) => { e.stopPropagation(); onToggleSelect?.(session.id); }}>
            <div
              className="w-4 h-4 rounded border flex items-center justify-center transition-colors cursor-pointer"
              style={{
                borderColor: selected ? 'var(--accent)' : 'var(--border)',
                backgroundColor: selected ? 'var(--accent)' : 'transparent',
              }}
            >
              {selected && <Check size={10} color="#fff" />}
            </div>
          </div>
        )}
        {/* 头像放大到 28px（原 16px 图标），并与两行文本垂直居中；
            消息数不再挤在头像下方，见第二行右端 */}
        <div className="shrink-0">
          <AgentBadge agentType={session.agentType} size="lg" isGenerating={isGenerating} />
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-1 min-w-0">
              <span className="text-xs truncate">{displayTitle}</span>
              {pendingApprovalCount > 0 && (
                <span
                  className="text-[9px] leading-none px-1 py-0.5 rounded-full shrink-0"
                  style={{ backgroundColor: 'var(--warning-bg, rgba(234,179,8,0.15))', color: 'var(--warning, #eab308)' }}
                  title={`${pendingApprovalCount} 项等待审批`}
                >
                  待审批 {pendingApprovalCount}
                </span>
              )}
            </div>
            <span className="text-[10px] shrink-0 ml-1" style={{ color: 'var(--text-secondary)' }}>
              {formatTime(session.updatedAt)}
            </span>
          </div>
          <div className="flex items-center gap-2 mt-0.5 min-w-0">
            <ProjectBadge projectId={session.projectId} className={PROJECT_BADGE_LIST_CLASS} />
            {session.lastMessagePreview && (
              <p className="text-[10px] truncate flex-1 min-w-0" style={{ color: 'var(--text-secondary)' }}>
                {session.lastMessagePreview}
              </p>
            )}
            {/* 消息数挂在第二行右端：它与预览同属"元信息"，
                既不占头像区，也不随预览长短左右抖动（ml-auto + shrink-0）。 */}
            {session.messageCount > 0 && (
              <span
                className="shrink-0 ml-auto text-[10px] tabular-nums"
                style={{ color: 'var(--text-tertiary)' }}
                title={`共 ${session.messageCount} 条消息`}
              >
                {session.messageCount} 条
              </span>
            )}
          </div>
        </div>
        <div className="hidden group-hover:flex items-center shrink-0 self-start">
          <button
            ref={menuButtonRef}
            type="button"
            onClick={openMenu}
            className="pd-btn p-0.5 rounded transition-all hover:bg-black/10 dark:hover:bg-white/10 active:scale-90"
            style={{ color: 'var(--text-secondary)' }}
            title="更多会话操作"
            aria-label="更多会话操作"
            aria-expanded={!!menuPosition}
          >
            <MoreHorizontal size={12} />
          </button>
        </div>
      </div>
      {menuPosition && createPortal(
        <div
          ref={menuRef}
          role="menu"
          className="pd-menu"
          style={{
            position: 'fixed',
            zIndex: 120,
            width: 168,
            top: menuPosition.top,
            left: menuPosition.left,
          }}
          onClick={(event) => event.stopPropagation()}
        >
          {onRename && (
            <button
              type="button"
              role="menuitem"
              className="pd-menu-item"
              onClick={() => {
                closeMenu();
                handleStartEdit();
              }}
            >
              <Pencil size={13} />
              <span className="flex-1">重命名</span>
            </button>
          )}
          {onSetProject && (
            <button
              type="button"
              role="menuitem"
              className="pd-menu-item"
              onClick={() => {
                closeMenu();
                onSetProject(session);
              }}
            >
              <FolderKanban size={13} />
              <span className="flex-1">项目归属</span>
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            className="pd-menu-item"
            onClick={() => {
              closeMenu();
              onArchive?.(session.id);
            }}
          >
            {archived ? <ArchiveRestore size={13} /> : <Archive size={13} />}
            <span className="flex-1">{archived ? '取消归档' : '归档'}</span>
          </button>
          <button
            type="button"
            role="menuitem"
            className="pd-menu-item"
            onClick={() => {
              closeMenu();
              onDelete?.(session.id);
            }}
            style={{ color: 'var(--status-danger, #ef4444)' }}
          >
            <Trash2 size={13} />
            <span className="flex-1">删除</span>
          </button>
        </div>,
        document.body,
      )}
      {isEditing && createPortal(
        <div
          className="fixed inset-0 z-[120] flex items-center justify-center p-4"
          style={{ backgroundColor: 'rgba(0,0,0,0.48)' }}
          onClick={handleCancelEdit}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-label="重命名会话"
            className="w-full max-w-[380px] overflow-hidden rounded-xl shadow-2xl"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
            onClick={(event) => event.stopPropagation()}
          >
            <div
              className="flex items-center gap-2 px-4 py-3"
              style={{ borderBottom: '1px solid var(--border)' }}
            >
              <Pencil size={14} style={{ color: 'var(--accent)', flexShrink: 0 }} />
              <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
                重命名会话
              </span>
            </div>
            <div className="px-4 py-4">
              <label
                htmlFor={`session-rename-${session.id}`}
                className="mb-1.5 block text-[11px]"
                style={{ color: 'var(--text-secondary)' }}
              >
                会话名称
              </label>
              <input
                ref={renameInputRef}
                id={`session-rename-${session.id}`}
                type="text"
                value={editTitle}
                onChange={(event) => setEditTitle(event.target.value)}
                onKeyDown={handleKeyDown}
                className="w-full rounded-lg px-3 py-2 text-sm outline-none"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-primary)',
                  border: '1px solid var(--accent)',
                  outline: 'none',
                  boxShadow: 'none',
                }}
              />
            </div>
            <div
              className="flex items-center justify-end gap-2 px-4 py-3"
              style={{ borderTop: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
            >
              <button
                type="button"
                onClick={handleCancelEdit}
                className="pd-btn rounded-lg px-3 py-1.5 text-xs"
                style={{
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                }}
              >
                取消
              </button>
              <button
                type="button"
                onClick={handleConfirmEdit}
                disabled={!editTitle.trim()}
                className="pd-btn rounded-lg px-3 py-1.5 text-xs disabled:cursor-not-allowed disabled:opacity-45"
                style={{ backgroundColor: 'var(--accent)', color: 'var(--text-on-accent)' }}
              >
                保存
              </button>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
});
