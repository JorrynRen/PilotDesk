import { useState, useRef, useEffect, useCallback } from 'react';
import { Send, Square, Zap, Brain, GraduationCap, Lightbulb, Cpu, ChevronUp, ClipboardList, ImagePlus, FileText, FolderOpen, X } from 'lucide-react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { ChatMode, Session, Attachment } from '../../types';
import { MODE_LABELS, MODE_COLORS, getModePrompt } from '../../types';
import { InspirationPicker } from '../input/InspirationPicker';
import { SkillPicker } from '../input/SkillPicker';
import { SecurityModeSelector, type SecurityModeValue } from '../security/SecurityModeSelector';
import { SessionUsageBar } from './SessionUsageBar';
import { showToast } from '../../utils/toast';
import { useSessionStore } from '../../stores/sessionStore';

import { useAgentRegistry } from '../../hooks/useAgentRegistry';


const MODE_ICONS: Record<ChatMode, typeof Send> = {
  native: Send,
  fast: Zap,
  think: Brain,
  expert: GraduationCap,
  plan: ClipboardList,
};

interface InputBarProps {
  session: Session | null;
  onSend: (message: string, mode: ChatMode, attachments?: Attachment[]) => void;
  onStop?: () => void;
  isGenerating?: boolean;
  streamingStatus?: string;
  pendingInput?: string | null;
  onPendingConsumed?: () => void;
  /** 会话安全模式（显示在「文件」按钮右侧；None 时不渲染） */
  securityMode?: SecurityModeValue;
  onSecurityModeChange?: (v: SecurityModeValue) => void;
}

const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg']);
const MAX_ATTACHMENTS = 8;

function isImageName(name: string): boolean {
  const ext = name.split('.').pop()?.toLowerCase() || '';
  return IMAGE_EXTS.has(ext);
}

function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(new Error('读取文件失败'));
    reader.readAsDataURL(file);
  });
}

interface PendingAttachment {
  kind: 'image' | 'file';
  name: string;
  mime: string;
  path?: string;
  data?: string;
}

export function InputBar({ session, onSend, onStop, isGenerating, streamingStatus, pendingInput, onPendingConsumed, securityMode, onSecurityModeChange }: InputBarProps) {
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<ChatMode>('native');
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [draggingFiles, setDraggingFiles] = useState<{ name: string; kind: 'image' | 'file' }[]>([]);
  const [showInspirationPicker, setShowInspirationPicker] = useState(false);
  const [showSkillPicker, setShowSkillPicker] = useState(false);
  const [showModeDropdown, setShowModeDropdown] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const pickerAnchorRef = useRef<HTMLDivElement>(null);
  const modeDropdownRef = useRef<HTMLDivElement>(null);
  const inputBarRef = useRef<HTMLDivElement>(null);
  const attachmentsRef = useRef<Attachment[]>([]);

  // 同步最新附件到 ref，供拖拽/选择/粘贴超限判断使用
  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  // Load prompt descriptions for tooltip
  const { getTheme, getDisplayName } = useAgentRegistry();
  const [modeDescriptions, setModeDescriptions] = useState<Record<string, string>>({});
  useEffect(() => {
    (async () => {
      const descs: Record<string, string> = {};
      for (const m of ['native', 'fast', 'think', 'expert'] as const) {
        const p = await getModePrompt(m);
        if (p && p.trim() !== '') descs[m] = p;
        else descs[m] = '原生模式，使用默认对话风格';
      }
      setModeDescriptions(descs);
    })();
  }, []);
  useEffect(() => {
    if (pendingInput) {
      setInput(pendingInput);
      onPendingConsumed?.();
      textareaRef.current?.focus();
    }
  }, [pendingInput, onPendingConsumed]);

  // Close mode dropdown on outside click
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (modeDropdownRef.current && !modeDropdownRef.current.contains(e.target as Node)) {
        setShowModeDropdown(false);
      }
    };
    if (showModeDropdown) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [showModeDropdown]);

  // Auto-resize textarea
  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = 'auto';
      textarea.style.height = Math.min(textarea.scrollHeight, 200) + 'px';
    }
  }, [input]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Ctrl+I for inspiration picker
      if (e.ctrlKey && e.key === 'i') {
        e.preventDefault();
        setShowInspirationPicker((v) => !v);
        setShowSkillPicker(false);
        return;
      }
      // Ctrl+K for skill picker
      if (e.ctrlKey && e.key === 'k') {
        e.preventDefault();
        setShowSkillPicker((v) => !v);
        setShowInspirationPicker(false);
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSendInternal();
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [input, mode]
  );

  const handleSendInternal = useCallback(() => {
    const trimmed = input.trim();
    const hasAttachments = attachments.length > 0;
    if (!trimmed && !hasAttachments) return;
    if (!session) {
      showToast('请先选择或创建一个会话', 'info');
      return;
    }
    onSend(trimmed, mode, hasAttachments ? attachments : undefined);
    setInput('');
    setAttachments([]);
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, mode, attachments, session, onSend]);

  // 将待落盘附件交给后端（选择/拖拽走路径复制，粘贴走字节兜底）
  const saveAttachments = useCallback(async (items: PendingAttachment[]): Promise<Attachment[]> => {
    if (!session) {
      showToast('请先选择或创建一个会话', 'info');
      return [];
    }
    if (items.length === 0) return [];
    try {
      return await invoke<Attachment[]>('save_attachments', { sessionId: session.id, items });
    } catch (e) {
      showToast(`附件保存失败: ${String(e)}`, 'error');
      return [];
    }
  }, [session]);

  const addFromPaths = useCallback(async (paths: string[], forceKind?: 'image' | 'file') => {
    const items: PendingAttachment[] = paths.map((p) => {
      const name = p.split(/[\\/]/).pop() || 'file';
      const kind: 'image' | 'file' = forceKind ?? (isImageName(name) ? 'image' : 'file');
      return { kind, name, mime: '', path: p };
    });

    const current = attachmentsRef.current;
    const room = MAX_ATTACHMENTS - current.length;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${items.length} 个`, 'info');
      return;
    }

    const toSave = items.slice(0, room);
    const saved = await saveAttachments(toSave);
    if (saved.length > 0) {
      setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
    }

    const overflow = items.length - toSave.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [saveAttachments]);

  const handlePickImages = useCallback(async () => {
    const selected = await openDialog({
      multiple: true,
      filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'] }],
    });
    if (!selected) return;
    await addFromPaths(Array.isArray(selected) ? selected : [selected], 'image');
  }, [addFromPaths]);

  const handlePickFiles = useCallback(async () => {
    const selected = await openDialog({ multiple: true });
    if (!selected) return;
    await addFromPaths(Array.isArray(selected) ? selected : [selected], 'file');
  }, [addFromPaths]);

  const updateSessionCwd = useSessionStore((s) => s.updateSessionCwd);
  const handlePickProjectDir = useCallback(async () => {
    if (!session) return;
    try {
      const selected = await openDialog({ directory: true, multiple: false, title: '选择项目目录' });
      if (!selected || typeof selected !== 'string') return;
      await updateSessionCwd(session.id, selected);
      showToast('项目目录已切换，下一条消息起生效', 'info');
    } catch (e) {
      showToast(`切换项目目录失败: ${e}`, 'error');
    }
  }, [session, updateSessionCwd]);

  const handlePaste = useCallback(async (e: React.ClipboardEvent) => {
    const files: File[] = [];
    const items = e.clipboardData?.items;
    if (items) {
      for (const item of Array.from(items)) {
        if (item.kind === 'file') {
          const f = item.getAsFile();
          if (f) files.push(f);
        }
      }
    }
    if (files.length === 0) return;
    e.preventDefault();
    const pending = await Promise.all(files.map(async (f) => {
      const dataUrl = await fileToDataUrl(f);
      const mime = dataUrl.split(';')[0].split(':')[1] || f.type || '';
      const base64 = dataUrl.split(',')[1] || '';
      return {
        kind: (f.type.startsWith('image/') ? 'image' : 'file') as 'image' | 'file',
        name: f.name || 'pasted',
        mime,
        data: base64,
      };
    }));
    const current = attachmentsRef.current;
    const room = MAX_ATTACHMENTS - current.length;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${pending.length} 个`, 'info');
      return;
    }

    const toSave = pending.slice(0, room);
    const saved = await saveAttachments(toSave);
    if (saved.length > 0) {
      setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
    }

    const overflow = pending.length - toSave.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [saveAttachments]);

  // 拖拽文件落盘（悬停 InputBar 区时高亮附件区并虚拟展示，松开后落盘）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const isOverInputBar = (x: number, y: number): boolean => {
      const el = inputBarRef.current;
      if (!el) return false;
      const scale = window.devicePixelRatio || 1;
      const r = el.getBoundingClientRect();
      const lx = x / scale;
      const ly = y / scale;
      return lx >= r.left && lx <= r.right && ly >= r.top && ly <= r.bottom;
    };

    (async () => {
      try {
        unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          const type = payload.type;

          if (type === 'enter') {
            setDraggingFiles(
              payload.paths.map((p) => {
                const name = p.split(/[\\/]/).pop() || 'file';
                return { name, kind: isImageName(name) ? 'image' : 'file' } as const;
              }),
            );
            setDragActive(isOverInputBar(payload.position.x, payload.position.y));
          } else if (type === 'over') {
            setDragActive(isOverInputBar(payload.position.x, payload.position.y));
          } else if (type === 'leave') {
            setDragActive(false);
            setDraggingFiles([]);
          } else if (type === 'drop') {
            if (isOverInputBar(payload.position.x, payload.position.y)) {
              addFromPaths(payload.paths);
            }
            setDragActive(false);
            setDraggingFiles([]);
          }
        });
        if (disposed) unlisten();
      } catch (e) {
        console.error('[InputBar] 注册拖拽监听失败:', e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addFromPaths]);

  const removeAttachment = useCallback(async (idx: number) => {
    const target = attachments[idx];
    setAttachments((prev) => prev.filter((_, i) => i !== idx));
    if (target?.path && session) {
      try {
        await invoke('delete_attachment', { sessionId: session.id, path: target.path });
      } catch (e) {
        console.warn('[InputBar] 删除附件文件失败:', e);
      }
    }
  }, [attachments, session]);

  // 会话切换时清理旧会话未发送的附件（删除落盘文件 + 清空状态）
  const prevSessionIdRef = useRef<string | null>(session?.id ?? null);
  useEffect(() => {
    const currentId = session?.id ?? null;
    const prevId = prevSessionIdRef.current;
    if (prevId === currentId) return;

    const old = attachmentsRef.current;
    old.forEach((att) => {
      if (att.path && prevId) {
        invoke('delete_attachment', { sessionId: prevId, path: att.path }).catch(() => {});
      }
    });
    setAttachments([]);
    prevSessionIdRef.current = currentId;
  }, [session?.id]);

  const imageCount = attachments.filter((a) => a.kind === 'image').length;
  const fileCount = attachments.filter((a) => a.kind === 'file').length;

  // Placeholder based on context
  const agentLabel = session ? getDisplayName(session.agentType) : '';
  const placeholder = !session
    ? '选择会话开始对话...'
    : `向 ${agentLabel} 发送消息... (Ctrl+I 灵感, Ctrl+K 技能)`;

  return (
    <div className="shrink-0" ref={inputBarRef} style={{ borderTop: '1px solid var(--border)' }}>
      {/* 等待提示条 */}
      {isGenerating && (
        <div className="flex items-center gap-2 px-4 py-1.5" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          <div className="flex items-center gap-1">
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '0ms' }} />
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '150ms' }} />
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '300ms' }} />
          </div>
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
            {'思考中...'}
          </span>
        </div>
      )}

      {/* Toolbar: mode selector + inspiration + skill + send */}
      <div className="flex items-center gap-1 px-4 pt-3">
        {/* Mode dropdown */}
        <div className="relative" ref={modeDropdownRef}>
          <button
            onClick={() => setShowModeDropdown((v) => !v)}
            className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: MODE_COLORS[mode],
              border: '1px solid var(--border)',
              height: '24px',
            }}
            title={modeDescriptions[mode] || '加载中...'}
          >
            {(() => {
              const Icon = MODE_ICONS[mode];
              return <Icon size={11} />;
            })()}
            {MODE_LABELS[mode]}
            <ChevronUp size={11} style={{ color: 'var(--text-secondary)' }} />
          </button>
          {showModeDropdown && (
            <div
              className="absolute left-0 bottom-full mb-1 py-1 rounded-lg shadow-lg z-50"
              style={{
                backgroundColor: 'var(--bg-panel)',
                border: '1px solid var(--border)',
                minWidth: '110px',
              }}
            >
              {(Object.keys(MODE_LABELS) as ChatMode[]).map((m) => {
                const Icon = MODE_ICONS[m];
                const isActive = mode === m;
                return (
                  <button
                    key={m}
                    onClick={() => { setMode(m); setShowModeDropdown(false); }}
                    className="flex items-center gap-2 w-full px-3 py-1.5 text-xs transition-colors text-left"
                    style={{
                      color: isActive ? MODE_COLORS[m] : 'var(--text-primary)',
                      backgroundColor: isActive ? `${MODE_COLORS[m]}11` : 'transparent',
                    }}
                    title={modeDescriptions[m] || '加载中...'}
                  >
                    <Icon size={12} />
                    <span className="flex-1">{MODE_LABELS[m]}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* Inspiration & Skill buttons */}
        <button
          onClick={() => { if (!session) { showToast('请先选择或创建一个会话', 'info'); return; } setShowInspirationPicker((v) => !v); setShowSkillPicker(false); }}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: showInspirationPicker ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: showInspirationPicker ? 'var(--border)' : 'transparent',
          }}
          title="灵感搜索 (Ctrl+I)"
        >
          <Lightbulb size={12} />
          灵感
        </button>
        <button
          onClick={() => { if (!session) { showToast('请先选择或创建一个会话', 'info'); return; } setShowSkillPicker((v) => !v); setShowInspirationPicker(false); }}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: showSkillPicker ? getTheme(session?.agentType ?? '').color : 'var(--text-secondary)',
            backgroundColor: showSkillPicker ? 'var(--border)' : 'transparent',
          }}
          title="技能列表 (Ctrl+K)"
        >
          <Cpu size={12} />
          技能
        </button>
        <button
          onClick={handlePickImages}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: imageCount > 0 ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: imageCount > 0 ? 'var(--border)' : 'transparent',
          }}
          title="添加图片（支持拖拽/粘贴，最多 8 个）"
        >
          <ImagePlus size={12} />
          {imageCount > 0 ? `${imageCount} 图` : '图片'}
        </button>
        <button
          onClick={handlePickFiles}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: fileCount > 0 ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: fileCount > 0 ? 'var(--border)' : 'transparent',
          }}
          title="添加文件（支持拖拽/粘贴，最多 8 个）"
        >
          <FileText size={12} />
          {fileCount > 0 ? `${fileCount} 文件` : '文件'}
        </button>

        {/* 会话安全模式选择器（显示在「文件」按钮右侧） */}
        {securityMode && onSecurityModeChange && (
          <SecurityModeSelector value={securityMode} onChange={onSecurityModeChange} />
        )}

        {/* 项目目录选择（安全模式右侧；切换后对后续消息生效） */}
        <button
          onClick={handlePickProjectDir}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0 max-w-[180px]"
          style={{ color: session?.cwd ? 'var(--text-primary)' : 'var(--text-tertiary)', backgroundColor: 'transparent' }}
          title={session?.cwd ? `项目目录：${session.cwd}\n点击可切换（下一条消息生效）` : '未设置项目目录（默认全局工作空间），点击选择'}
        >
          <FolderOpen size={12} style={{ flexShrink: 0 }} />
          <span className="truncate">{session?.cwd ? session.cwd.split(/[\\/]/).pop() || session.cwd : '默认项目'}</span>
        </button>

        {/* Spacer */}
        <div className="flex-1" />

        {/* Stop / Send button */}
        {isGenerating ? (
          <button
            onClick={onStop}
            className="pd-btn p-1.5 rounded-lg transition-colors"
            style={{ backgroundColor: '#EF444422', color: '#EF4444' }}
            title="停止生成"
          >
            <Square size={14} />
          </button>
        ) : (
          <button
            onClick={handleSendInternal}
            disabled={(!input.trim() && attachments.length === 0) || !session}
            className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors disabled:opacity-30"
            style={{
              backgroundColor: (input.trim() || attachments.length > 0) && session ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: (input.trim() || attachments.length > 0) && session ? '#fff' : 'var(--text-secondary)',
            }}
            title="发送"
          >
            <Send size={11} />
            发送
          </button>
        )}
      </div>

      {/* 工具栏底部分隔线（上下留白均衡垂直居中；宽度与工具栏内容一致） */}
      <div className="px-4 py-1">
        <div style={{ borderBottom: '1px dashed var(--border)' }} />
      </div>

      {/* Input area */}
      <div className="flex items-end px-4 pt-1 pb-3" ref={pickerAnchorRef}>
        <div className="flex-1 relative">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            placeholder={placeholder}
            rows={1}
            className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none focus:outline-none focus-visible:outline-none"
            style={{
              backgroundColor: 'transparent',
              color: 'var(--text-primary)',
              border: 'none',
              boxShadow: 'none',
              outline: 'none',
              minHeight: '36px',
              maxHeight: '200px',
            }}
          />
          {/* 附件预览：图片缩略图 + 文件 chip（拖拽悬停时高亮并虚拟展示） */}
          {(attachments.length > 0 || dragActive) && (
            <div
              className="flex flex-wrap gap-2 mt-2 p-2 rounded-lg transition-colors"
              style={{
                backgroundColor: dragActive ? 'var(--accent)0d' : 'transparent',
                border: `1px dashed ${dragActive ? 'var(--accent)' : 'var(--border)'}`,
              }}
            >
              {attachments.map((att, idx) => (
                <div
                  key={`${idx}-${att.path}`}
                  className="relative rounded-lg overflow-hidden shrink-0 flex items-center justify-center"
                  style={{
                    width: att.kind === 'image' ? 64 : 120,
                    height: att.kind === 'image' ? 64 : 40,
                    border: '1px solid var(--border)',
                    backgroundColor: 'var(--bg-tertiary)',
                  }}
                >
                  {att.kind === 'image' ? (
                    <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                  ) : (
                    <div className="flex items-center gap-1 px-2 max-w-full">
                      <FileText size={12} style={{ color: 'var(--text-secondary)', flexShrink: 0 }} />
                      <span className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>{att.name}</span>
                    </div>
                  )}
                  <button
                    onClick={() => removeAttachment(idx)}
                    className="absolute top-0.5 right-0.5 w-4 h-4 rounded-full flex items-center justify-center transition-opacity hover:opacity-90"
                    style={{ backgroundColor: 'rgba(0,0,0,0.6)', color: '#fff' }}
                    title="移除附件"
                  >
                    <X size={10} />
                  </button>
                </div>
              ))}
              {/* 拖拽中的虚拟附件占位 */}
              {dragActive && draggingFiles.map((f, i) => (
                <div
                  key={`drag-${i}-${f.name}`}
                  className="relative rounded-lg overflow-hidden shrink-0 flex items-center justify-center opacity-70"
                  style={{
                    width: f.kind === 'image' ? 64 : 120,
                    height: f.kind === 'image' ? 64 : 40,
                    border: '1px dashed var(--accent)',
                    backgroundColor: 'var(--bg-tertiary)',
                  }}
                >
                  {f.kind === 'image' ? (
                    <ImagePlus size={16} style={{ color: 'var(--accent)' }} />
                  ) : (
                    <div className="flex items-center gap-1 px-2 max-w-full">
                      <FileText size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                      <span className="text-[11px] truncate" style={{ color: 'var(--text-secondary)' }}>{f.name}</span>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
          {/* Picker panels */}
          {showInspirationPicker && (
            <InspirationPicker
              onSelect={(content) => {
                const ta = textareaRef.current;
                if (ta) {
                  const cursorPos = ta.selectionStart ?? ta.value.length;
                  const text = ta.value;
                  const before = text.slice(0, cursorPos);
                  const after = text.slice(cursorPos);
                  setInput(before + content + after);
                  requestAnimationFrame(() => {
                    ta.focus();
                    ta.selectionStart = ta.selectionEnd = cursorPos + content.length;
                  });
                } else {
                  setInput((prev) => prev + content);
                }
                setShowInspirationPicker(false);
              }}
              onClose={() => setShowInspirationPicker(false)}
            />
          )}
          {showSkillPicker && (
            <SkillPicker
              agentType={session?.agentType ?? ''}
              onSelect={(name) => {
                const ta = textareaRef.current;
                if (ta) {
                  const cursorPos = ta.selectionStart ?? ta.value.length;
                  const text = ta.value;
                  const before = text.slice(0, cursorPos);
                  const after = text.slice(cursorPos);
                  const insertion = `@${name} `;
                  setInput(before + insertion + after);
                  // Restore cursor position after insertion
                  requestAnimationFrame(() => {
                    ta.focus();
                    ta.selectionStart = ta.selectionEnd = cursorPos + insertion.length;
                  });
                } else {
                  setInput((prev) => prev + `@${name} `);
                }
                setShowSkillPicker(false);
              }}
              onClose={() => setShowSkillPicker(false)}
            />
          )}
        </div>
      </div>

      {session && <SessionUsageBar sessionId={session.id} agentType={session.agentType ?? null} />}
    </div>
  );
}
