import { useState, useEffect, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  History, Undo2, Loader2, RefreshCw, Search, Trash2, FileText,
} from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import { Select } from '../common/Select';
import { confirmDialog } from '../../stores/confirmStore';

// ============================================================
// 文件修改历史（独立设置页）
// - 分页加载（offset/limit）+ 会话筛选 + 路径关键字搜索
// - 单条撤销 / 单条删除 / 按会话清空 / 全部清空
// ============================================================

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

/** 历史会话/房间（kind: session=正式会话 / room=群聊房间） */
interface FileHistorySessionItem {
  id: string;
  kind: 'session' | 'room';
}

const PAGE_SIZE = 50;

function formatTime(secs: number): string {
  if (!secs) return '-';
  return new Date(secs * 1000).toLocaleString();
}

function scopeLabel(sid: string, kind?: string): string {
  const short = sid.slice(0, 8);
  return kind === 'room' ? `群聊 ${short}…` : `会话 ${short}…`;
}

export function FileHistorySettings() {
  const [entries, setEntries] = useState<FileHistoryEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [sessions, setSessions] = useState<FileHistorySessionItem[]>([]);
  const [sessionFilter, setSessionFilter] = useState('');
  const [keyword, setKeyword] = useState('');
  const [undoingId, setUndoingId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  // 会话/房间 id → 场景标记（用于列表条目归属展示）
  const kindMap = useMemo(() => new Map(sessions.map((s) => [s.id, s.kind])), [sessions]);

  // 会话筛选下拉数据源
  useEffect(() => {
    (async () => {
      try {
        setSessions(await invoke<FileHistorySessionItem[]>('list_file_history_sessions'));
      } catch {
        setSessions([]);
      }
    })();
  }, []);

  const load = useCallback(async (offset: number, append: boolean) => {
    if (offset === 0) setLoading(true);
    else setLoadingMore(true);
    try {
      const page = await invoke<FileHistoryPage>('list_file_history', {
        sessionId: sessionFilter || null,
        limit: PAGE_SIZE,
        offset,
        pathKeyword: keyword || null,
      });
      setTotal(page.total ?? 0);
      setEntries((prev) => (append ? [...prev, ...(page.items ?? [])] : (page.items ?? [])));
    } catch {
      // ignore
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, [sessionFilter, keyword]);

  // 筛选条件变化时重置重载（effect 体内不允许同步 setState：把加载推迟一个微任务，观感与原先一致）
  useEffect(() => {
    queueMicrotask(() => { void load(0, false); });
  }, [load]);

  const refreshAll = useCallback(async () => {
    try {
      setSessions(await invoke<FileHistorySessionItem[]>('list_file_history_sessions'));
    } catch {
      /* ignore */
    }
    load(0, false);
  }, [load]);

  const undo = async (id: number) => {
    setUndoingId(id);
    try {
      await invoke<string>('undo_file_history', { historyId: id });
      setEntries((prev) => prev.filter((e) => e.id !== id));
      setTotal((t) => Math.max(0, t - 1));
    } catch {
      // ignore
    } finally {
      setUndoingId(null);
    }
  };

  const remove = async (scope: 'one' | 'session' | 'all', id?: number) => {
    const text =
      scope === 'all'
        ? '确定清空全部文件修改历史吗？此操作不可恢复。'
        : scope === 'session'
          ? `确定清空该${sessionFilter && kindMap.get(sessionFilter) === 'room' ? '群聊房间' : '会话'}（${sessionFilter?.slice(0, 8) || ''}…）的全部历史记录吗？`
          : '确定删除该条历史记录吗？';
    const ok = await confirmDialog({
      title: scope === 'one' ? '确认删除' : '确认清空',
      message: text,
      confirmText: scope === 'one' ? '删除' : '清空',
    });
    if (!ok) return;
    setBusy(true);
    try {
      await invoke('delete_file_history', {
        scope,
        historyId: id ?? null,
        sessionId: sessionFilter || null,
      });
      await refreshAll();
    } catch {
      // ignore
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsSection
      title="文件修改历史"
      description="Agent 通过 write_file / edit_file 修改文件前的快照，可撤销到上一个版本。"
      actions={
        <SettingsButton
          onClick={refreshAll}
          variant="secondary"
          disabled={busy}
          icon={busy ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
        >
          刷新
        </SettingsButton>
      }
    >
      {/* 筛选工具栏 */}
      <div className="flex items-center gap-2 mb-3 flex-wrap">
        <Select
          value={sessionFilter}
          onChange={(v) => setSessionFilter(v)}
          options={[
            { value: '', label: `全部（${total} 条）` },
            ...sessions.map((s) => ({ value: s.id, label: scopeLabel(s.id, s.kind) })),
          ]}
          placeholder={`全部（${total} 条）`}
          size="sm"
        />
        <div
          className="pd-field flex-1 min-w-[140px] flex items-center gap-1.5 px-2 py-1.5 rounded-lg"
          style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
        >
          <Search size={12} style={{ color: 'var(--text-tertiary)' }} />
          <input
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
            placeholder="按文件路径搜索..."
            className="flex-1 bg-transparent outline-none text-xs"
            style={{ color: 'var(--text-primary)' }}
          />
        </div>
        <SettingsButton
          onClick={() => remove('all')}
          variant="secondary"
          disabled={busy || total === 0}
          icon={<Trash2 size={12} />}
        >
          清空全部
        </SettingsButton>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-8">
          <Loader2 size={18} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
        </div>
      ) : entries.length === 0 ? (
        <div className="text-center py-8">
          <History size={24} className="mx-auto mb-2" style={{ color: 'var(--text-tertiary)' }} />
          <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            暂无文件修改记录
          </p>
        </div>
      ) : (
        <div className="space-y-1.5">
          {entries.map((e) => (
            <div
              key={e.id}
              className="flex items-center gap-2 px-3 py-2 rounded-lg"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <FileText size={12} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                  <span className="text-xs truncate font-mono" style={{ color: 'var(--text-primary)' }} title={e.filePath}>
                    {e.filePath}
                  </span>
                  {!e.fileExisted && (
                    <span
                      className="text-[9px] px-1 py-0.5 rounded shrink-0"
                      style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-tertiary)' }}
                    >
                      新建
                    </span>
                  )}
                </div>
                <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                  {formatTime(e.createdAt)}
                  {e.sessionId && <span className="ml-2">{scopeLabel(e.sessionId, kindMap.get(e.sessionId))}</span>}
                </p>
              </div>
              <SettingsButton
                onClick={() => remove('one', e.id)}
                variant="secondary"
                disabled={busy}
                icon={<Trash2 size={12} />}
                title="删除该条记录"
              >
                删除
              </SettingsButton>
              <SettingsButton
                onClick={() => undo(e.id)}
                variant="secondary"
                disabled={undoingId === e.id}
                icon={undoingId === e.id ? <Loader2 size={12} className="animate-spin" /> : <Undo2 size={12} />}
              >
                撤销
              </SettingsButton>
            </div>
          ))}
          {entries.length < total && (
            <button
              onClick={() => load(entries.length, true)}
              disabled={loadingMore}
              className="w-full py-2 rounded-lg text-xs transition-colors"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
            >
              {loadingMore ? '加载中...' : `加载更多（${entries.length}/${total}）`}
            </button>
          )}
        </div>
      )}
      <p className="text-[10px] mt-2" style={{ color: 'var(--text-tertiary)' }}>
        快照保存在本地数据库，撤销会恢复文件到修改前的版本；删除记录不会影响已恢复的文件。
      </p>
    </SettingsSection>
  );
}
