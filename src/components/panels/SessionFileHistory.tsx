/**
 * SessionFileHistory — 当前会话的文件修改历史（会话右侧面板「文件历史」tab）
 *
 * 数据源与群聊房间的「文件历史」同一张表（`file_history`），只是 scope 换成 currentSessionId：
 * 每条记录是 Agent 写文件前的快照，可就地撤销回修改前版本。
 *
 * 与会话内联的「文件差异」（agent-file-diff）分工：那边看的是**这一轮改了什么**，
 * 这边是**这个会话改过哪些文件、能逐条回退**。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { FileText, Loader2, RefreshCw, Search, Undo2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useSessionStore } from '../../stores/sessionStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

interface FileHistoryEntry {
  id: number;
  sessionId: string;
  filePath: string;
  fileExisted: boolean;
  createdAt: number;
}

interface FileHistoryPage {
  total: number;
  items: FileHistoryEntry[];
}

const PAGE_SIZE = 50;

function formatTime(secs: number): string {
  if (!secs) return '-';
  const d = new Date(secs * 1000);
  const now = Date.now();
  const diff = Math.floor((now - d.getTime()) / 1000);
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return d.toLocaleDateString();
}

export function SessionFileHistory() {
  const currentSessionId = useSessionStore((s) => s.currentSessionId);

  const [items, setItems] = useState<FileHistoryEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [undoingId, setUndoingId] = useState<number | null>(null);
  const [keyword, setKeyword] = useState('');
  /** 已生效的搜索词（输入防抖后才落这里） */
  const [activeKeyword, setActiveKeyword] = useState('');

  const load = useCallback(
    async (opts: { offset?: number; append?: boolean; keyword?: string; silent?: boolean } = {}) => {
      const { offset = 0, append = false, keyword: kw = activeKeyword, silent = false } = opts;
      if (!currentSessionId) {
        setItems([]);
        setTotal(0);
        return;
      }
      if (append) setLoadingMore(true);
      else if (silent) setRefreshing(true);
      else setLoading(true);
      try {
        const page = await invoke<FileHistoryPage>('list_file_history', {
          sessionId: currentSessionId,
          limit: PAGE_SIZE,
          offset,
          pathKeyword: kw || null,
        });
        setItems((prev) => (append ? [...prev, ...(page.items ?? [])] : page.items ?? []));
        setTotal(page.total ?? 0);
      } catch (e) {
        if (!append) setItems([]);
        showToast(`读取文件历史失败: ${errorMessage(e)}`, 'error');
      } finally {
        setLoading(false);
        setLoadingMore(false);
        setRefreshing(false);
      }
    },
    [currentSessionId, activeKeyword],
  );

  // 切会话 / 搜索词变化：重新从第一页加载
  // 推到微任务：load 开头就同步 setState，属于"effect 体内同步 setState"
  // （`react-hooks/set-state-in-effect` 判为级联渲染）；同一个任务、早于绘制，行为一致。
  useEffect(() => {
    void Promise.resolve().then(() => load({ offset: 0 }));
  }, [load]);

  // 输入防抖 300ms → activeKeyword
  useEffect(() => {
    if (keyword === activeKeyword) return;
    const t = setTimeout(() => setActiveKeyword(keyword), 300);
    return () => clearTimeout(t);
  }, [keyword, activeKeyword]);

  // 实时刷新：Agent 运行中产生新快照（事件 scope 与会话 id 相同），防抖 300ms 合并连续写。
  // 依赖是 currentSessionId（不是回调）：切会话必须重新注册，否则闭包锁的是旧会话 id。
  const loadRef = useRef(load);
  // 用 effect 同步 ref：渲染期写 ref 会被 `react-hooks/refs` 判违规（读取发生在事件的防抖回调里，时序无影响）
  useEffect(() => { loadRef.current = load; }, [load]);
  useEffect(() => {
    if (!currentSessionId) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    (async () => {
      try {
        const off = await listen<{ roomId?: string; sessionId?: string }>('file-history-updated', (event) => {
          if (disposed) return;
          if (event.payload.sessionId !== currentSessionId) return;
          if (timer) clearTimeout(timer);
          timer = setTimeout(() => void loadRef.current({ offset: 0, silent: true }), 300);
        });
        if (disposed) {
          off();
          return;
        }
        unlisten = off;
      } catch (e) {
        console.warn('[SessionFileHistory] 注册文件历史事件监听失败:', e);
      }
    })();
    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      unlisten?.();
    };
  }, [currentSessionId]);

  const handleUndo = useCallback(
    async (entry: FileHistoryEntry) => {
      setUndoingId(entry.id);
      try {
        await invoke('undo_file_history', { historyId: entry.id });
        setItems((prev) => prev.filter((e) => e.id !== entry.id));
        setTotal((t) => Math.max(0, t - 1));
        showToast('已撤销到修改前版本', 'success');
      } catch (e) {
        showToast(`撤销失败: ${errorMessage(e)}`, 'error');
      } finally {
        setUndoingId(null);
      }
    },
    [],
  );

  const hasMore = items.length < total;

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* 头部：不设标题（tab 名已写明"文件历史"），只留搜索 + 计数 + 刷新，省一行高度 */}
      <div className="px-3 py-2.5 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-2">
          <div className="relative flex-1 min-w-0">
            <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
            <input
              type="text"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              disabled={!currentSessionId}
              placeholder="按文件路径搜索…"
              className="search-input"
            />
          </div>
          {currentSessionId && (
            <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>{total} 条</span>
          )}
          <button
            onClick={() => void load({ offset: 0, silent: true })}
            disabled={!currentSessionId || loading || refreshing}
            className="p-1 rounded transition-colors hover:opacity-80 shrink-0"
            style={{ color: 'var(--text-tertiary)', cursor: !currentSessionId ? 'not-allowed' : 'pointer' }}
            title="刷新文件历史"
          >
            <RefreshCw size={12} style={{ animation: refreshing ? 'spin 0.8s linear infinite' : undefined }} />
          </button>
        </div>
      </div>

      {/* 列表：左右内边距与上方搜索区一致（px-3），两边宽度对齐 */}
      <div className="flex-1 overflow-y-auto px-3 py-2 space-y-1.5 pd-scroll-stable">
        {!currentSessionId ? (
          <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
            未选中会话：文件历史按会话记录，请先选择或新建一个会话。
          </div>
        ) : loading ? (
          <div className="flex items-center justify-center py-6">
            <Loader2 size={14} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
          </div>
        ) : items.length === 0 ? (
          <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
            {activeKeyword ? '没有匹配该路径的记录' : '该会话暂无文件修改记录'}
          </div>
        ) : (
          <>
            {items.map((e) => (
              <div
                key={e.id}
                className="px-2 py-1.5 rounded-lg cursor-pointer transition-opacity hover:opacity-75"
                style={{ backgroundColor: 'var(--bg-tertiary)' }}
                onClick={() => invoke('open_path', { path: e.filePath }).catch(() => {})}
                title={`${e.filePath}（点击打开文件）`}
              >
                <div className="flex items-center gap-1">
                  <FileText size={10} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                  <span className="text-[10px] truncate font-mono flex-1" style={{ color: 'var(--text-primary)' }} title={e.filePath}>
                    {e.filePath}
                  </span>
                </div>
                <div className="flex items-center justify-between mt-0.5">
                  <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                    {formatTime(e.createdAt)}
                    {!e.fileExisted && <span className="ml-1">· 新建文件</span>}
                  </span>
                  <button
                    onClick={(ev) => { ev.stopPropagation(); void handleUndo(e); }}
                    disabled={undoingId === e.id}
                    className="flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[9px] transition-colors disabled:opacity-40"
                    style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--accent)', border: '1px solid var(--border)' }}
                    title="撤销到修改前版本"
                  >
                    {undoingId === e.id ? <Loader2 size={9} className="animate-spin" /> : <Undo2 size={9} />}
                    撤销
                  </button>
                </div>
              </div>
            ))}
            {hasMore && (
              <button
                onClick={() => void load({ offset: items.length, append: true })}
                disabled={loadingMore}
                className="w-full py-1.5 rounded-lg text-[10px] transition-colors disabled:opacity-50"
                style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                {loadingMore ? '加载中…' : `加载更多（还有 ${total - items.length} 条）`}
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
