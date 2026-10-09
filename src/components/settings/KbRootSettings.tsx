/**
 * KbRootSettings — 设置 › 知识库 › 根目录
 *
 * 知识库根目录默认落在「全局工作区/Knowledge」（用户看得见、可换盘、可整体备份），可改成别处。
 *
 * 换根时对**已入库的原文**有两种处理，由用户当场选：
 *   - 迁移并切换（默认推荐）：先把原文搬到新目录，**全部成功才写设置**；有失败则不改设置并列出失败项（可重试）
 *   - 仅切换：立刻生效、不动文件；旧原文留在原处（其「打开原文」会失效，条目与检索不受影响）
 *
 * 数据库不需要任何改动：库里存的是相对路径 + 内容指纹命名的文件，换根只是换个前缀。
 */

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { FolderOpen, Loader2, RotateCcw, AlertCircle } from 'lucide-react';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useKnowledgeStore } from '../../stores/knowledgeStore';

interface KbRootInfo {
  root: string;
  isCustom: boolean;
  fileCount: number;
  bytes: number;
}

interface KbRootChangeOutcome {
  root: string;
  moved: number;
  skipped: number;
  failed: string[];
  cleaned: boolean;
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function KbRootSettings() {
  const [info, setInfo] = useState<KbRootInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [failures, setFailures] = useState<string[]>([]);
  /** 待确认的换根请求（旧目录有文件时才需要问） */
  const [pending, setPending] = useState<{ root: string | null; label: string } | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setInfo(await invoke<KbRootInfo>('kb_root_info'));
    } catch {
      setInfo(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // effect 体内不允许同步 setState（react-hooks/set-state-in-effect）：把首次加载推迟一个微任务，
    // 仍在同一帧内执行，观感与原先一致
    queueMicrotask(() => { void refresh(); });
  }, [refresh]);

  const apply = async (root: string | null, migrate: boolean) => {
    setBusy(true);
    setFailures([]);
    setPending(null);
    try {
      const out = await invoke<KbRootChangeOutcome>('kb_set_root', { root, migrate });
      if (out.failed.length > 0) {
        // 设置没改动：旧目录仍是完整可用的一套，列出来让用户处理完再重试
        setFailures(out.failed);
        showToast(`有 ${out.failed.length} 个文件没搬成功，已保持原目录不变（可重试）`, 'error');
      } else {
        const extra = [
          out.moved > 0 ? `迁移 ${out.moved} 个文件` : '',
          out.skipped > 0 ? `跳过 ${out.skipped} 个（内容已存在）` : '',
          out.cleaned ? '旧目录空壳已清理' : '',
          migrate || out.moved > 0 ? '' : '未迁移旧文件',
        ].filter(Boolean);
        showToast(`知识库目录已切换：${out.root}${extra.length ? `（${extra.join('；')}）` : ''}`, 'success');
      }
      await refresh();
      // 文件列表里的绝对路径跟着根目录变，顺手让知识库页重新拉一次
      const baseId = useKnowledgeStore.getState().activeBaseId;
      if (baseId) void useKnowledgeStore.getState().selectBase(baseId);
    } catch (e) {
      showToast(`切换失败: ${errorMessage(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  /** 选好目录后：旧目录有文件就先问，没有就直接迁移（无文件时迁移与仅切换等价） */
  const handlePick = async () => {
    try {
      const picked = await openDialog({ directory: true, title: '选择知识库根目录' });
      const path = typeof picked === 'string' ? picked : '';
      if (!path) return;
      if ((info?.fileCount ?? 0) > 0) {
        setPending({ root: path, label: path });
      } else {
        await apply(path, false);
      }
    } catch (e) {
      showToast(`选择目录失败: ${errorMessage(e)}`, 'error');
    }
  };

  const handleRestoreDefault = async () => {
    if ((info?.fileCount ?? 0) > 0) {
      setPending({ root: null, label: '默认位置（全局工作区/Knowledge）' });
    } else {
      await apply(null, false);
    }
  };

  return (
    <div className="space-y-4">
      <section>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>文件根目录</h3>

        <div
          className="rounded-lg px-3 py-2.5 space-y-2"
          style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        >
          <div className="flex items-center gap-2">
            <span className="shrink-0 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>当前</span>
            {loading ? (
              <Loader2 size={11} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
            ) : (
              <>
                <span
                  className="flex-1 min-w-0 text-[11px] truncate"
                  style={{ color: 'var(--text-primary)', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace' }}
                  title={info?.root ?? ''}
                >
                  {info?.root ?? '（无法确定）'}
                </span>
                <span
                  className="shrink-0 px-1.5 py-[1px] rounded text-[10px]"
                  style={{
                    backgroundColor: info?.isCustom ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                    color: info?.isCustom ? 'var(--accent)' : 'var(--text-tertiary)',
                  }}
                >
                  {info?.isCustom ? '自定义' : '默认'}
                </span>
              </>
            )}
          </div>

          {info && (
            <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              原文 {info.fileCount} 个文件，共 {fmtSize(info.bytes)}（含去重后的本地文件与网页正文副本）
            </div>
          )}

          <div className="flex items-center gap-2 pt-0.5">
            <button
              onClick={() => void handlePick()}
              disabled={busy}
              className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
              style={{
                color: 'var(--text-secondary)',
                backgroundColor: 'var(--bg-field)',
                border: '1px solid var(--border)',
                opacity: busy ? 0.5 : 1,
              }}
            >
              {busy ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
              更改目录…
            </button>
            <button
              onClick={() => void handleRestoreDefault()}
              disabled={busy || !info?.isCustom}
              className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
              style={{
                color: 'var(--text-secondary)',
                backgroundColor: 'var(--bg-field)',
                border: '1px solid var(--border)',
                opacity: busy || !info?.isCustom ? 0.5 : 1,
              }}
              title="恢复到默认位置：全局工作区/Knowledge"
            >
              <RotateCcw size={11} />
              恢复默认
            </button>
          </div>
        </div>

        <div className="mt-2 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
          原文按内容去重后集中存放（<code>files/&lt;内容指纹&gt;-&lt;原名&gt;</code>），多个知识库共享同一份。
          库里只记相对路径，所以换目录不会让条目与检索失效。注意 <b>MEMORY.db 不随该目录移动</b> —— 它必须留在本地，
          放到同步盘会损坏数据库。
        </div>
      </section>

      {failures.length > 0 && (
        <div
          className="rounded-lg px-3 py-2.5 text-[11px] space-y-1"
          style={{ backgroundColor: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.3)', color: 'var(--text-secondary)' }}
        >
          <div className="flex items-center gap-1.5" style={{ color: 'var(--status-danger, #EF4444)' }}>
            <AlertCircle size={12} />
            <span>以下文件没能搬过去（设置未改动，修好后可重试）：</span>
          </div>
          {failures.slice(0, 10).map((f) => (
            <div key={f} className="break-all" style={{ color: 'var(--text-tertiary)' }}>{f}</div>
          ))}
          {failures.length > 10 && (
            <div style={{ color: 'var(--text-tertiary)' }}>…另有 {failures.length - 10} 项</div>
          )}
        </div>
      )}

      {/* 旧目录有文件时先问清楚：迁移还是仅切换 */}
      {pending && info && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => setPending(null)}
        >
          <div
            className="w-[520px] max-w-[92vw] rounded-xl shadow-2xl"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-5 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
              <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>更改知识库目录</span>
            </div>
            <div className="px-5 py-4 space-y-3 text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
              <div>
                新目录：<span style={{ color: 'var(--text-primary)' }}>{pending.label}</span>
              </div>
              <div>
                当前目录里有 <b>{info.fileCount} 个原文文件</b>（{fmtSize(info.bytes)}）。要怎么处理它们？
              </div>
              <div className="rounded-lg px-3 py-2 space-y-1" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
                <div><b>迁移并切换</b>：先搬到新目录，全部成功才生效；失败则不改设置并列出失败项（可重试）。</div>
                <div><b>仅切换</b>：立刻生效、不动文件；旧原文留在原处，它们的「打开原文」会失效，
                  但条目与检索不受影响（正文早已入库）。适合想把文件留在原处慢慢搬的情况。</div>
              </div>
            </div>
            <div className="px-5 py-3 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
              <div className="flex-1" />
              <button
                onClick={() => setPending(null)}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
              >
                取消
              </button>
              <button
                onClick={() => void apply(pending.root, false)}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                title="只改设置，不搬文件"
              >
                仅切换
              </button>
              <button
                onClick={() => void apply(pending.root, true)}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs"
                style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
              >
                迁移并切换
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
