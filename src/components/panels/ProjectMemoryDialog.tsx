/**
 * ProjectMemoryDialog — 项目记忆预览弹窗
 *
 * 只读预览「当前会话工作区」的记忆根下的 MEMORY.md，并提供跳转到设置页编辑的入口。
 *
 * 为什么是弹窗而不是右侧面板的常驻 tab：它只有一个只读预览 + 一个跳设置按钮，
 * 常驻会白占一个窄面板（260px）顶部的 tab 位；而"看一眼 Agent 现在记住了什么"属于
 * 低频、看一眼就走的动作，弹窗更合适（入口在输入框工具栏）。
 *
 * 口径提示：记忆根由当前会话的 cwd 解析，**未选中会话时回退兜底记忆根**——
 * 这种情况必须在界面上写明，否则用户会把兜底记忆根误读成"当前项目记忆"。
 */

import { useEffect, useState } from 'react';
import { MemoryStick, FileText, X } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useSessionStore } from '../../stores/sessionStore';
import { getProjectMemory, type ProjectMemoryInfo } from '../../types';
import { elide } from '../../utils/text';

interface ProjectMemoryDialogProps {
  onClose: () => void;
}

export function ProjectMemoryDialog({ onClose }: ProjectMemoryDialogProps) {
  const navigate = useNavigate();
  const currentSession = useSessionStore((s) => s.sessions.find((x) => x.id === s.currentSessionId));
  const [mem, setMem] = useState<ProjectMemoryInfo | null>(null);
  const [loading, setLoading] = useState(true);

  const sessionCwd = currentSession?.cwd || '';
  const sessionTitle = currentSession?.title || '';

  useEffect(() => {
    let disposed = false;
    (async () => {
      setLoading(true);
      try {
        const m = await getProjectMemory(sessionCwd || undefined);
        if (!disposed) setMem(m);
      } catch {
        if (!disposed) setMem(null);
      } finally {
        if (!disposed) setLoading(false);
      }
    })();
    return () => {
      disposed = true;
    };
  }, [sessionCwd]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  const openSettings = () => {
    onClose();
    navigate('/settings?tab=memory');
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[560px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div className="flex items-center gap-2 px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <MemoryStick size={15} style={{ color: 'var(--accent)', flexShrink: 0 }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>项目记忆</span>
          <span className="text-[11px] truncate min-w-0" style={{ color: 'var(--text-tertiary)' }}>
            {sessionCwd
              ? `当前工作区：${sessionTitle || sessionCwd}`
              : '未选中会话'}
          </span>
          <div className="flex-1" />
          <button
            onClick={onClose}
            className="pd-btn p-1 rounded hover:opacity-80 shrink-0"
            style={{ color: 'var(--text-tertiary)' }}
            title="关闭"
          >
            <X size={14} />
          </button>
        </div>

        {/* 记忆根来源 —— 未选中会话时显示的是兜底记忆根，必须写明，避免误读成当前项目 */}
        <div className="px-5 py-2 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          {sessionCwd ? (
            <span className="text-[11px] break-all" style={{ color: 'var(--text-tertiary)' }}>
              记忆根：{mem?.root ?? '…'}
            </span>
          ) : (
            <span className="text-[11px]" style={{ color: 'var(--status-warning, #f59e0b)' }}>
              当前未选中会话，展示的是兜底记忆根（非某个项目的记忆）
            </span>
          )}
        </div>

        {/* 内容 */}
        <div className="flex-1 overflow-y-auto px-5 py-4 pd-scroll-stable">
          {loading ? (
            <div className="text-xs py-8 text-center" style={{ color: 'var(--text-tertiary)' }}>加载中…</div>
          ) : !mem || !mem.exists ? (
            <div className="flex flex-col items-center justify-center gap-3 py-8">
              <MemoryStick size={26} style={{ color: 'var(--text-tertiary)' }} />
              <span className="text-xs text-center" style={{ color: 'var(--text-secondary)' }}>
                该工作区还没有 MEMORY.md
              </span>
              {mem && (
                <span className="text-[10px] max-w-full truncate" style={{ color: 'var(--text-tertiary)' }} title={mem.root}>
                  记忆根：{mem.root}
                </span>
              )}
              <button
                onClick={openSettings}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs"
                style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
              >
                去设置创建
              </button>
            </div>
          ) : (
            <pre
              className="text-xs leading-relaxed whitespace-pre-wrap break-words m-0 rounded-lg px-3 py-2"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            >
              {elide(mem.content, 4000, '\n…（内容过长，预览截断）')}
            </pre>
          )}
        </div>

        {/* 底部操作 */}
        {!loading && mem?.exists && (
          <div className="px-5 py-3 shrink-0 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
            <button
              onClick={openSettings}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs flex items-center gap-1.5"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            >
              <FileText size={12} />
              在设置中编辑
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
