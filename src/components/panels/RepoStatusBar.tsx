/**
 * RepoStatusBar — 「文件历史」面板顶部的仓库状态条（只读）。
 *
 * 与面板里的快照列表分工：
 * - 列表：本会话 / 本房间 Agent 通过 write_file、edit_file 写过什么，可逐条撤销；
 * - 这里：该目录在 Git 下的真实状态（含用户手改、未跟踪文件），不提供任何写操作。
 *
 * 目录不是 Git 仓库、或本机没有 Git 时整条隐藏（Git 属推荐依赖，缺失不影响其它功能）。
 */

import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, GitBranch, Loader2, RefreshCw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface GitStatusEntry {
  path: string;
  status: string;
}

interface RepoStatus {
  isRepo: boolean;
  root: string | null;
  branch: string | null;
  head: string | null;
  changedCount: number;
  untrackedCount: number;
  entries: GitStatusEntry[];
}

interface RepoStatusBarProps {
  /** 会话 cwd / 房间产物目录；为 null 时后端回退到全局工作区 */
  cwd: string | null;
  /** 变化时重新拉取（新快照、撤销、切换目录等） */
  refreshToken?: number;
  /** 本会话 / 本房间的快照条数：仓库有未提交但没有快照时提示"不能在这里撤销" */
  snapshotCount?: number;
}

const STATUS_LABEL: Record<string, string> = {
  M: '改',
  A: '增',
  D: '删',
  R: '重命名',
  C: '复制',
  U: '冲突',
  '?': '新',
};

const STATUS_COLOR: Record<string, string> = {
  M: '#F59E0B',
  A: '#10B981',
  D: '#EF4444',
  R: '#6366F1',
  C: '#6366F1',
  U: '#EF4444',
  '?': '#10B981',
};

/** 仓库相对路径 → 绝对路径（用于点击打开文件） */
function toAbsolute(root: string | null, path: string): string {
  if (!root) return path;
  return `${root.replace(/[\\/]+$/, '')}/${path}`;
}

export function RepoStatusBar({ cwd, refreshToken = 0, snapshotCount = 0 }: RepoStatusBarProps) {
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState(false);
  /** 手动刷新计数（与 cwd / refreshToken 一起作为拉取依赖） */
  const [manualTick, setManualTick] = useState(0);

  useEffect(() => {
    let alive = true;
    const run = async () => {
      if (!alive) return;
      setLoading(true);
      try {
        const res = await invoke<RepoStatus>('git_repo_status', { cwd: cwd ?? null });
        if (alive) setStatus(res?.isRepo ? res : null);
      } catch {
        // 未装 Git / 目录不可读等：整条隐藏，不打扰用户
        if (alive) setStatus(null);
      } finally {
        if (alive) setLoading(false);
      }
    };
    void Promise.resolve().then(run);
    return () => {
      alive = false;
    };
  }, [cwd, refreshToken, manualTick]);

  if (!status) return null;

  const dirty = status.changedCount + status.untrackedCount;
  const branchText = status.branch ?? (status.head ? `HEAD ${status.head}` : '尚无提交');

  return (
    <div className="px-3 py-1.5 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
      <div className="flex items-center gap-1 min-w-0">
        <button
          onClick={() => setExpanded((v) => !v)}
          className="flex items-center gap-1.5 min-w-0 flex-1 text-left"
          title={status.root ? `仓库：${status.root}` : undefined}
        >
          {expanded ? (
            <ChevronDown size={10} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
          ) : (
            <ChevronRight size={10} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
          )}
          <GitBranch size={10} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
          <span
            className="text-[10px] truncate font-mono"
            style={{ color: 'var(--text-secondary)' }}
          >
            {branchText}
          </span>
          {status.branch && status.head && (
            <span className="text-[9px] font-mono shrink-0" style={{ color: 'var(--text-tertiary)' }}>
              {status.head}
            </span>
          )}
          <span
            className="text-[9px] shrink-0"
            style={{ color: dirty ? '#F59E0B' : '#10B981' }}
          >
            {dirty ? `${status.changedCount} 改 · ${status.untrackedCount} 新` : '工作区干净'}
          </span>
        </button>
        {loading ? (
          <Loader2 size={10} className="animate-spin shrink-0" style={{ color: 'var(--text-tertiary)' }} />
        ) : (
          <button
            onClick={() => setManualTick((t) => t + 1)}
            className="p-0.5 rounded shrink-0 transition-colors hover:opacity-80"
            style={{ color: 'var(--text-tertiary)' }}
            title="刷新仓库状态"
          >
            <RefreshCw size={10} />
          </button>
        )}
      </div>

      {expanded && (
        <div className="mt-1.5 space-y-0.5 max-h-52 overflow-y-auto pd-scroll-stable">
          <div
            className="text-[9px] truncate font-mono px-1"
            style={{ color: 'var(--text-tertiary)' }}
            title={status.root ?? ''}
          >
            {status.root}
          </div>
          {dirty === 0 ? (
            <div className="text-[9px] px-1" style={{ color: 'var(--text-tertiary)' }}>
              没有未提交改动
            </div>
          ) : (
            status.entries.map((e) => {
              const color = STATUS_COLOR[e.status] ?? 'var(--text-tertiary)';
              return (
                <div
                  key={`${e.status}-${e.path}`}
                  className="flex items-center gap-1.5 px-1 py-0.5 rounded cursor-pointer transition-opacity hover:opacity-70"
                  onClick={() => invoke('open_path', { path: toAbsolute(status.root, e.path) }).catch(() => {})}
                  title={`${e.path}（点击打开文件）`}
                >
                  <span
                    className="text-[8px] px-1 rounded shrink-0"
                    style={{ backgroundColor: `${color}1A`, color }}
                  >
                    {STATUS_LABEL[e.status] ?? e.status}
                  </span>
                  <span
                    className="text-[9px] truncate font-mono"
                    style={{ color: 'var(--text-secondary)' }}
                  >
                    {e.path}
                  </span>
                </div>
              );
            })
          )}
          {dirty > status.entries.length && (
            <div className="text-[9px] px-1" style={{ color: 'var(--text-tertiary)' }}>
              仅显示前 {status.entries.length} 条（共 {dirty} 条）
            </div>
          )}
          {dirty > 0 && snapshotCount === 0 && (
            <div
              className="text-[9px] leading-relaxed px-1.5 py-1 rounded"
              style={{ backgroundColor: 'rgba(245, 158, 11, 0.08)', color: '#F59E0B' }}
            >
              这些改动没有对应的写入快照（多为手工修改或其他工具所为），不能在这里撤销。
            </div>
          )}
        </div>
      )}
    </div>
  );
}
