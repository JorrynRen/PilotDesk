import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { createPortal } from 'react-dom';
import { Copy, Edit3, Pencil, Bookmark, Check, User, X, FileText } from 'lucide-react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { MarkdownRenderer, linkifyUrls } from './MarkdownRenderer';
import { ThinkingChain } from './ThinkingChain';
import { ConfirmationCard, type ConfirmationBlockData } from '../confirmation/ConfirmationCard';
import { MODE_LABELS, MODE_COLORS } from '../../types';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useInspirationStore } from '../../stores/inspirationStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { showToast } from '../../utils/toast';
import { useSessionStore } from '../../stores/sessionStore';
import { type Message } from '../../types';
import { AgentIcon } from '../common/AgentIcon';
import { isApiSession } from '../../utils/sessionType';
import type { ThinkingChainStep } from '../layout/MainPanel';

interface MessageBubbleProps {
  message: Message;
  agentType: string;
  apiProviderId?: string;
  apiModel?: string;
  thinkingChain?: ThinkingChainStep[];
  isStreaming?: boolean;
  isHighlighted?: boolean;
  /** ask_user 确认块（仅最后一条 assistant 消息内嵌渲染）。 */
  confirmation?: ConfirmationBlockData | null;
  onEdit?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
  onResend?: (content: string) => void;
}

function formatTimestamp(ts: number): string {
  const date = new Date(ts * 1000);
  const time = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}/${m}/${d} ${time}`;
}

export function MessageBubble({ message, agentType, apiProviderId, apiModel, thinkingChain, isStreaming, isHighlighted, confirmation, onEdit, onSaveInspiration, onResend }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const [isEditing, setIsEditing] = useState(false);
  const [editContent, setEditContent] = useState('');
  const [previewImage, setPreviewImage] = useState<string | null>(null);
  const [previewScale, setPreviewScale] = useState(1);
  const [previewOffset, setPreviewOffset] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const editRef = useRef<HTMLTextAreaElement>(null);
  const dragRef = useRef<{ startX: number; startY: number; offsetX: number; offsetY: number; moved: boolean } | null>(null);
  const justDraggedRef = useRef(false);
  const { updateMessage } = useSessionStore();
  const { getTheme, getDisplayName } = useAgentRegistry();
  const isUser = message.role === 'user';

  const providers = useApiProviderStore((s) => s.providers);
  const { fetchProviders } = useApiProviderStore();
  const providerName = apiProviderId
    ? providers.find(p => p.id === apiProviderId)?.name
    : undefined;

  useEffect(() => {
    if (apiProviderId && providers.length === 0) {
      fetchProviders().catch(() => {});
    }
  }, [apiProviderId, providers.length, fetchProviders]);

  const buildAgentLabel = () => {
    const typeLabel = getDisplayName(agentType) || agentType;
    const time = formatTimestamp(message.timestamp);
    const parts = [typeLabel];
    if (isApiSession(agentType)) {
      if (providerName) parts.push(providerName);
      if (apiModel) parts.push(apiModel);
    }
    return parts.join(' | ') + ' ' + time;
  };

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(message.content);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = message.content;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [message.content]);

  const handleSaveInspiration = useCallback(async () => {
    const createInspiration = useInspirationStore.getState().createInspiration;
    const preview = message.content.slice(0, 30).replace(/\n/g, ' ');
    await createInspiration({
      title: preview.length >= 30 ? preview + '...' : preview,
      content: message.content,
      sourceAgent: agentType,
    });
    showToast('已添加到灵感市集', 'success');
  }, [message.content, agentType]);

  const handleStartEdit = useCallback(() => {
    setEditContent(message.content);
    setIsEditing(true);
    setTimeout(() => editRef.current?.focus(), 0);
  }, [message.content]);

  const handleSaveEdit = useCallback(async () => {
    if (!editContent.trim()) return;
    await updateMessage(message.id, editContent);
    setIsEditing(false);
    showToast('消息已更新', 'success');
  }, [message.id, editContent, updateMessage]);

  const handleCancelEdit = useCallback(() => {
    setIsEditing(false);
    setEditContent('');
  }, []);

  const handleResend = useCallback(() => {
    onResend?.(message.content);
  }, [message.content, onResend]);

  const handleEditKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSaveEdit();
    }
    if (e.key === 'Escape') {
      handleCancelEdit();
    }
  }, [handleSaveEdit, handleCancelEdit]);

  // 图片放大预览：Escape 关闭
  useEffect(() => {
    if (!previewImage) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPreviewImage(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewImage]);

  // 打开图片预览（每次打开重置缩放比例与位置）
  const openPreview = useCallback((src: string) => {
    setPreviewScale(1);
    setPreviewOffset({ x: 0, y: 0 });
    setDragging(false);
    dragRef.current = null;
    justDraggedRef.current = false;
    setPreviewImage(src);
  }, []);

  // 鼠标滑轮缩放：向上滚放大、向下滚缩小，比例限制在 0.25x ~ 6x
  const handlePreviewWheel = useCallback((e: React.WheelEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const factor = Math.exp(-e.deltaY * 0.002);
    setPreviewScale((s) => Math.min(6, Math.max(0.25, s * factor)));
  }, []);

  // 拖动图片移动位置（仅左键，记录起始坐标与偏移）
  const handlePreviewMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    dragRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      offsetX: previewOffset.x,
      offsetY: previewOffset.y,
      moved: false,
    };
    justDraggedRef.current = false;
    setDragging(true);
  }, [previewOffset]);

  const handlePreviewMouseMove = useCallback((e: React.MouseEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const dx = e.clientX - drag.startX;
    const dy = e.clientY - drag.startY;
    if (!drag.moved && Math.hypot(dx, dy) > 3) {
      drag.moved = true;
      justDraggedRef.current = true;
    }
    if (drag.moved) {
      setPreviewOffset({ x: drag.offsetX + dx, y: drag.offsetY + dy });
    }
  }, []);

  const handlePreviewMouseUp = useCallback(() => {
    dragRef.current = null;
    setDragging(false);
  }, []);

  // 背景点击关闭：拖动后释放不触发关闭
  const handleOverlayClick = useCallback(() => {
    if (justDraggedRef.current) {
      justDraggedRef.current = false;
      return;
    }
    setPreviewImage(null);
  }, []);

  const messageMode = message.mode as keyof typeof MODE_LABELS;
  const modeColor = MODE_COLORS[messageMode];
  const modeLabel = MODE_LABELS[messageMode];

  const mergedThinkingChain = useMemo(() => {
    if (thinkingChain && thinkingChain.length > 0) {
      return thinkingChain;
    }
    const steps: ThinkingChainStep[] = [];
    if (message.toolCalls) {
      try {
        const chain = JSON.parse(message.toolCalls);
        if (Array.isArray(chain)) {
          for (const s of chain) {
            if (s.type === 'reasoning') {
              steps.push({
                id: `persisted-reasoning-${s.id || steps.length}`,
                type: 'reasoning',
                content: s.content ?? '',
                timestamp: s.ts || message.timestamp,
              });
            } else if (s.type === 'tool_start') {
              steps.push({
                id: `persisted-tool-${s.id || steps.length}`,
                type: 'tool_start',
                toolName: s.toolName,
                toolArgs: s.args,
                timestamp: s.ts || message.timestamp,
              });
            } else if (s.type === 'tool_result') {
              steps.push({
                id: `persisted-result-${s.id || steps.length}`,
                type: 'tool_result',
                toolName: s.toolName,
                toolResult: s.result,
                toolSuccess: s.success !== false,
                timestamp: s.ts || message.timestamp,
              });
            } else if (s.type === 'file_diff') {
              steps.push({
                id: `persisted-diff-${s.id || steps.length}`,
                type: 'file_diff',
                filePath: s.filePath,
                fileDiff: s.fileDiff,
                timestamp: s.ts || message.timestamp,
              });
            }
          }
        }
      } catch { /* ignore */ }
    }
    return steps;
  }, [thinkingChain, message.toolCalls, message.timestamp]);

  const agentTheme = getTheme(agentType);
  const agentColor = agentTheme.color;
  const agentInitial = agentTheme.initial;
  // 搜索定位高亮：outline + offset 在气泡外留出间隙，避免与同色气泡（如紫色 user 气泡）融为一体
  const highlightStyle = isHighlighted ? { outline: '2px solid var(--accent)', outlineOffset: '2px' } : undefined;

  // ── 统一父容器：所有内容块共享同一个 flex 布局，自然撑满一致宽度 ──
  return (
    <>
      <div className="w-full group flex gap-2.5 items-start px-4 py-[3px]">
      {/* Avatar column */}
      <div className="shrink-0 pt-0.5 w-7">
        {isUser ? (
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
            style={{ backgroundColor: 'rgba(59, 130, 246, 0.15)', color: '#3B82F6' }}
          >
            <User size={14} />
          </div>
        ) : message.role === 'system' ? (
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12 1v2m0 18v2M4.22 4.22l1.42 1.42m12.72 12.72 1.42 1.42M1 12h2m18 0h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>
          </div>
        ) : agentTheme.icon ? (
          <div className="w-7 h-7 rounded-full flex items-center justify-center overflow-hidden">
            <AgentIcon icon={agentTheme.icon} size={22} fallback={
              <div className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
                style={{ backgroundColor: `${agentColor}20`, color: agentColor }}
              >
                {agentInitial}
              </div>
            } />
          </div>
        ) : (
          <div className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
            style={{ backgroundColor: `${agentColor}20`, color: agentColor }}
          >
            {agentInitial}
          </div>
        )}
      </div>

      {/* Content column */}
      <div className="flex-1 min-w-0">
        {/* Label row */}
        <div className="flex items-center gap-2 mb-1">
          {isUser ? (
            <>
              <span className="text-[11px]" style={{ color: 'var(--text-primary)' }}>User</span>
              <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{formatTimestamp(message.timestamp)}</span>
            </>
          ) : message.role === 'system' ? (
            <>
              <span className="text-[11px] opacity-60" style={{ color: 'var(--text-tertiary)' }}>系统提示</span>
              <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{formatTimestamp(message.timestamp)}</span>
            </>
          ) : (
            <>
              <span className="text-[11px]" style={{ color: 'var(--text-primary)' }}>{buildAgentLabel()}</span>
              {message.mode !== 'native' && (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full"
                  style={{ backgroundColor: `${modeColor}22`, color: modeColor }}
                >
                  {modeLabel}
                </span>
              )}
            </>
          )}
        </div>

        {/* Message card — w-full 确保所有类型宽度一致 */}
        <div className="w-full">
          {isUser ? (
            <div className="rounded-xl px-3.5 py-2.5 w-full" style={{ backgroundColor: 'var(--accent)', color: '#fff', ...highlightStyle }}>
              {isEditing ? (
                <textarea
                  ref={editRef}
                  value={editContent}
                  onChange={(e) => setEditContent(e.target.value)}
                  onKeyDown={handleEditKeyDown}
                  className="w-full text-[13px] leading-relaxed outline-none resize-none"
                  style={{ color: '#fff', backgroundColor: 'transparent', minHeight: '60px' }}
                />
              ) : (
                <>
                  {message.attachments && message.attachments.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 mb-1.5">
                      {message.attachments.map((att, idx) =>
                        att.kind === 'image' ? (
                          <button
                            key={`${idx}-${att.path}`}
                            onClick={() => openPreview(convertFileSrc(att.path))}
                            className="block w-20 h-20 rounded-lg overflow-hidden shrink-0 cursor-zoom-in p-0"
                            style={{ border: '1px solid rgba(255,255,255,0.35)' }}
                            aria-label={`预览图片 ${idx + 1}`}
                          >
                            <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                          </button>
                        ) : (
                          <div
                            key={`${idx}-${att.path}`}
                            className="flex items-center gap-1.5 px-2 py-1 rounded-md text-[11px] shrink-0 max-w-[200px]"
                            style={{ backgroundColor: 'rgba(255,255,255,0.18)', color: '#fff' }}
                            title={att.path}
                          >
                            <FileText size={12} style={{ flexShrink: 0 }} />
                            <span className="truncate">{att.name}</span>
                          </div>
                        ),
                      )}
                    </div>
                  )}
                  {message.content && (
                    <p className="text-[13px] leading-relaxed whitespace-pre-wrap m-0">{linkifyUrls(message.content)}</p>
                  )}
                </>
              )}
            </div>
          ) : message.role === 'system' ? (
            (() => {
              const isTimeout = message.content.startsWith('⏱');
              const isError = message.content.startsWith('❗');
              const isStopped = message.content.startsWith('⏸');
              const accentColor = isError ? 'var(--danger, #EF4444)' : isTimeout ? 'var(--warning, #F59E0B)' : isStopped ? 'var(--text-tertiary, #9CA3AF)' : 'var(--info, #3B82F6)';
              const displayContent = message.content.replace(/^[⏱❗⏸ℹ]\s*/, '');
              return (
                <div className="rounded-lg w-full" style={{ backgroundColor: 'var(--bg-tertiary)', borderLeft: `3px solid ${accentColor}`, ...highlightStyle }}>
                  <div className="flex items-center gap-2 px-3.5 py-2">
                    <span className="shrink-0" style={{ color: accentColor }}>
                      {isError ? (
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
                      ) : isTimeout ? (
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
                      ) : isStopped ? (
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="8" y1="12" x2="16" y2="12"/></svg>
                      ) : (
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
                      )}
                    </span>
                    <p className="text-[12px] leading-relaxed m-0 flex-1 min-w-0" style={{ color: 'var(--text-secondary)' }}>{displayContent}</p>
                  </div>
                </div>
              );
            })()
          ) : (
            <div className="rounded-xl w-full px-3.5 py-2.5" style={{ backgroundColor: 'var(--bg-secondary)', ...highlightStyle }}>
              {/* 思维链 */}
              <ThinkingChain steps={mergedThinkingChain} defaultCollapsed={!isStreaming} />
              <MarkdownRenderer content={message.content} />
              {/* ask_user 确认块：内嵌在工具调用链之后 */}
              {confirmation && (
                <ConfirmationCard
                  confirmation={confirmation.request}
                  countdown={confirmation.countdown}
                  submittedText="已提交，等待模型继续…"
                  openHint="模型正在等待你的回复，请直接回复。"
                  onSubmit={confirmation.onSubmit}
                />
              )}
            </div>
          )}

          {/* Action buttons — 统一放置在卡片底部 */}
          {isUser ? (
            <div className="flex items-center gap-1 mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
              <button onClick={handleCopy} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="复制">
                {copied ? <Check size={11} /> : <Copy size={11} />}{copied ? '已复制' : '复制'}
              </button>
              {isEditing ? (
                <>
                  <button onClick={handleSaveEdit} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: '#22c55e' }} title="保存"><Check size={11} />保存</button>
                  <button onClick={handleCancelEdit} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="取消"><Edit3 size={11} />取消</button>
                </>
              ) : onEdit && (
                <button onClick={handleStartEdit} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="编辑"><Pencil size={11} />编辑</button>
              )}
              <button onClick={handleSaveInspiration} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="收藏灵感"><Bookmark size={11} />收藏灵感</button>
              {onResend && (
                <button onClick={handleResend} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--accent)' }} title="重发">
                  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>重发
                </button>
              )}
            </div>
          ) : message.role !== 'system' ? (
            <div className="flex items-center gap-1 mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
              <button onClick={handleCopy} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="复制">
                {copied ? <Check size={11} /> : <Copy size={11} />}{copied ? '已复制' : '复制'}
              </button>
              <button onClick={handleSaveInspiration} className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95" style={{ color: 'var(--text-secondary)' }} title="收藏灵感"><Bookmark size={11} />收藏灵感</button>
            </div>
          ) : null}
        </div>
      </div>
      </div>

      {previewImage && createPortal(
        <div
          className="fixed inset-0 z-50 flex items-center justify-center p-6 cursor-zoom-out select-none"
          style={{ backgroundColor: 'rgba(0,0,0,0.85)' }}
          onClick={handleOverlayClick}
          onWheel={handlePreviewWheel}
          onMouseMove={handlePreviewMouseMove}
          onMouseUp={handlePreviewMouseUp}
        >
          <button
            onClick={() => setPreviewImage(null)}
            className="absolute top-4 right-4 w-9 h-9 rounded-full flex items-center justify-center text-white/80 hover:text-white hover:bg-white/10 transition-colors"
            aria-label="关闭预览"
          >
            <X size={20} />
          </button>
          <img
            src={previewImage}
            alt="图片预览"
            draggable={false}
            className={`max-w-full max-h-full object-contain rounded-lg shadow-2xl ${dragging ? 'cursor-grabbing' : 'cursor-grab'}`}
            style={{ transform: `translate(${previewOffset.x}px, ${previewOffset.y}px) scale(${previewScale})` }}
            onMouseDown={handlePreviewMouseDown}
            onClick={(e) => e.stopPropagation()}
          />
        </div>,
        document.body
      )}
    </>
  );
}
