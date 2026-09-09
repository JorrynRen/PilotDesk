import { useEffect, useState } from 'react';
import { Brain, FileText } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useSessionStore } from '../../stores/sessionStore';
import { getProjectMemory, type ProjectMemoryInfo } from '../../types';

/** 右侧面板「记忆」：当前会话记忆根的 MEMORY.md 只读预览（会话记忆 v1，不做 KV） */
export function ProjectMemoryPreview() {
  const navigate = useNavigate();
  const currentSession = useSessionStore((s) => s.sessions.find((x) => x.id === s.currentSessionId));
  const [mem, setMem] = useState<ProjectMemoryInfo | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let disposed = false;
    (async () => {
      setLoading(true);
      try {
        const cwd = currentSession?.cwd || '';
        const m = await getProjectMemory(cwd || undefined);
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
  }, [currentSession?.id, currentSession?.cwd]);

  const openSettings = () => navigate('/settings?tab=memory');

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="px-4 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
          项目记忆
        </h3>
        <p className="text-[11px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
          当前会话工作区的 MEMORY.md
        </p>
      </div>

      <div className="flex-1 overflow-y-auto px-3 py-2">
        {loading ? (
          <div className="text-xs py-6 text-center" style={{ color: 'var(--text-secondary)' }}>加载中…</div>
        ) : !mem || !mem.exists ? (
          <div className="flex flex-col items-center justify-center h-40 gap-2">
            <Brain size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs text-center px-2" style={{ color: 'var(--text-secondary)' }}>
              该工作区还没有 MEMORY.md
            </span>
            <button
              onClick={openSettings}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs"
              style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
            >
              去设置创建/编辑
            </button>
            {mem && (
              <span className="text-[10px] max-w-full truncate px-2 text-center" style={{ color: 'var(--text-tertiary)' }}>
                记忆根：{mem.root}
              </span>
            )}
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            <pre
              className="text-xs leading-relaxed whitespace-pre-wrap break-words m-0 rounded-lg px-3 py-2"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            >
              {mem.content.length > 1600 ? mem.content.slice(0, 1600) + '\n…（内容过长，预览截断）' : mem.content}
            </pre>
            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={openSettings}
                className="pd-btn px-2.5 py-1 rounded-lg text-xs flex items-center gap-1.5"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              >
                <FileText size={12} />
                在设置中编辑
              </button>
              <span className="text-[10px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                记忆根：{mem.root}
              </span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
