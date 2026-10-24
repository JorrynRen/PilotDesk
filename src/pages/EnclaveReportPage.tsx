/**
 * 团队报告（周报 / 月报 + 绩效数据）。
 *
 * 全部数据由主机侧 `enclave_team_stats` 只读聚合：任务、确认汇报、知识产出与成员计数。
 * 这里只做“周期选择 + 呈现 + 复制 Markdown”，不做评分、不写回任何业务数据。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BarChart3, CheckCircle2, ChevronLeft, ChevronRight, ClipboardCopy, Download, Loader2 } from 'lucide-react';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useAccountStore } from '../stores/accountStore';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import { EnclaveToolbarActions, EnclaveFilesRootPanel } from '../components/enclave/EnclaveToolbar';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import type { TeamStats } from '../types/enclave';

type PeriodKind = 'week' | 'month' | 'year' | 'custom';

interface Range {
  from: number;
  to: number;
  label: string;
  /** 自定义区间填反 / 未填时不可查询 */
  valid: boolean;
}

const DAY_SEC = 86_400;

function fmtDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** 本地时区的固定周期：周一起算 / 自然月 / 自然年；`offset` = 0 当期、-1 上一期 */
function periodRange(kind: Exclude<PeriodKind, 'custom'>, offset: number): Range {
  const now = new Date();
  let start: Date;
  let end: Date;
  if (kind === 'week') {
    start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const dow = (start.getDay() + 6) % 7; // 周一 = 0
    start.setDate(start.getDate() - dow + offset * 7);
    end = new Date(start);
    end.setDate(end.getDate() + 7);
  } else if (kind === 'month') {
    start = new Date(now.getFullYear(), now.getMonth() + offset, 1);
    end = new Date(now.getFullYear(), now.getMonth() + offset + 1, 1);
  } else {
    start = new Date(now.getFullYear() + offset, 0, 1);
    end = new Date(now.getFullYear() + offset + 1, 0, 1);
  }
  const last = new Date(end.getTime() - DAY_SEC * 1000);
  return {
    from: Math.floor(start.getTime() / 1000),
    to: Math.floor(end.getTime() / 1000),
    label: `${fmtDate(start)} – ${fmtDate(last)}`,
    valid: true,
  };
}

/** 自定义区间：`YYYY-MM-DD` 输入，按本地日期整天（含结束日） */
function customRange(fromDate: string, toDate: string): Range {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(fromDate) || !/^\d{4}-\d{2}-\d{2}$/.test(toDate)) {
    return { from: 0, to: 0, label: '请选择起止日期', valid: false };
  }
  const start = new Date(`${fromDate}T00:00:00`);
  const end = new Date(`${toDate}T00:00:00`);
  if (!Number.isFinite(start.getTime()) || !Number.isFinite(end.getTime()) || start > end) {
    return { from: 0, to: 0, label: '起止日期不正确', valid: false };
  }
  const endExclusive = new Date(end.getTime() + DAY_SEC * 1000);
  return {
    from: Math.floor(start.getTime() / 1000),
    to: Math.floor(endExclusive.getTime() / 1000),
    label: `${fromDate} – ${toDate}`,
    valid: true,
  };
}

function hoursLabel(h: number): string {
  if (!h) return '—';
  if (h < 1) return `${Math.round(h * 60)} 分钟`;
  if (h < 48) return `${h.toFixed(1)} 小时`;
  return `${(h / 24).toFixed(1)} 天`;
}

/** 完成时间：本地时区 `MM-DD HH:mm` */
function fmtDone(unixSec: number): string {
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function EnclaveReportPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const detail = useEnclaveOrgStore((s) => s.detail);
  const accountKey = useAccountStore((s) => s.account?.email ?? '');

  const [kind, setKind] = useState<PeriodKind>('week');
  const [offset, setOffset] = useState(0);
  /** 自定义区间（本地日期，含结束日） */
  const [fromDate, setFromDate] = useState(() => {
    const now = new Date();
    return fmtDate(new Date(now.getFullYear(), now.getMonth(), 1));
  });
  const [toDate, setToDate] = useState(() => fmtDate(new Date()));
  const [stats, setStats] = useState<TeamStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rootOpen, setRootOpen] = useState(false);

  const range = useMemo(
    () => (kind === 'custom' ? customRange(fromDate, toDate) : periodRange(kind, offset)),
    [kind, offset, fromDate, toDate],
  );
  const members = detail?.members ?? [];
  const memberName = useCallback(
    (id: string) => {
      const m = members.find((x) => x.userId === id);
      return m?.displayName || m?.email || id;
    },
    [members],
  );

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const st = useEnclaveOrgStore.getState();
      if (!(await st.ensureIdentity()) || cancelled) return;
      await st.loadOrgs();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountKey]);

  const load = useCallback(async () => {
    if (!activeOrgId) return;
    if (!range.valid) {
      setStats(null);
      setError(range.label);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<TeamStats>('enclave_team_stats', {
        orgId: activeOrgId,
        from: range.from,
        to: range.to,
      });
      setStats(data);
    } catch (e) {
      setError(errorMessage(e));
      setStats(null);
    } finally {
      setLoading(false);
    }
  }, [activeOrgId, range.from, range.to, range.valid, range.label]);

  useEffect(() => {
    void load();
  }, [load]);

  // 团队数据变化（任务 / 知识）后自动重拉，1s 节流
  const lastReload = useRef(0);
  useEffect(() => {
    if (!activeOrgId) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<{ orgId: string; topic?: string; type?: string }>('enclave:changed', (ev) => {
      const { orgId, topic, type } = ev.payload ?? {};
      if (orgId !== activeOrgId || type === 'realtime') return;
      if (topic !== 'task' && topic !== 'kb') return;
      const now = Date.now();
      if (now - lastReload.current < 1000) return;
      lastReload.current = now;
      void load();
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeOrgId, load]);

  const handleModeChange = useCallback(
    (mode: ViewMode) => {
      setMode(mode);
      navigate('/');
    },
    [setMode, navigate],
  );

  const copyMarkdown = () => {
    if (!stats) return;
    const title =
      kind === 'week' ? '团队周报' : kind === 'month' ? '团队月报' : kind === 'year' ? '团队年报' : '团队报告';
    const lines = [
      `# ${title}（${range.label}）`,
      '',
      `- 新增任务：${stats.created}`,
      `- 完成任务：${stats.completed}`,
      `- 确认汇报：${stats.reportsConfirmed}`,
      `- 新增知识：${stats.entriesAdded}`,
      `- 新增交付物：${stats.filesAdded}`,
      '',
      '## 成员',
      '',
      '| 成员 | 完成 | 进行中 | 待确认 | 逾期 | 平均耗时 |',
      '| --- | ---: | ---: | ---: | ---: | ---: |',
      ...stats.members.map(
        (m) =>
          `| ${memberName(m.userId)} | ${m.completed} | ${m.inProgress} | ${m.submitted} | ${m.overdue} | ${hoursLabel(m.avgHours)} |`,
      ),
      '',
      '## 已完成任务',
      '',
      ...(stats.completedTasks.length > 0
        ? stats.completedTasks.map(
            (t) =>
              `- ${t.title}（${memberName(t.assigneeId)}，${fmtDone(t.completedAt)}）${t.hasOutput ? '' : ' · 产出未沉淀'}`,
          )
        : ['- 无']),
    ];
    void navigator.clipboard
      .writeText(lines.join('\n'))
      .then(() => showToast('已复制 Markdown', 'success'))
      .catch(() => showToast('复制失败（浏览器未授权剪贴板）', 'error'));
  };

  /** 导出 CSV：概览 + 成员 + 完成明细（带 BOM，Excel 打开不乱码） */
  const exportCsv = () => {
    if (!stats) return;
    const quote = (v: string | number) => `"${String(v).replace(/"/g, '""')}"`;
    const rows: (string | number)[][] = [
      ['周期', range.label],
      ['新增任务', stats.created],
      ['完成任务', stats.completed],
      ['确认汇报', stats.reportsConfirmed],
      ['新增知识', stats.entriesAdded],
      ['新增交付物', stats.filesAdded],
      [],
      ['成员', '完成', '进行中', '待确认', '逾期', '平均耗时(小时)'],
      ...stats.members.map((m) => [
        memberName(m.userId),
        m.completed,
        m.inProgress,
        m.submitted,
        m.overdue,
        m.avgHours ? m.avgHours.toFixed(2) : '',
      ]),
      [],
      ['已完成任务', '负责人', '完成时间', '产出已沉淀'],
      ...stats.completedTasks.map((t) => [
        t.title,
        memberName(t.assigneeId),
        fmtDone(t.completedAt),
        t.hasOutput ? '是' : '否',
      ]),
    ];
    const csv = rows.map((r) => r.map(quote).join(',')).join('\r\n');
    const blob = new Blob([`\ufeff${csv}`], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `团队报告-${range.label}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const cards: { label: string; value: number; hint: string }[] = stats
    ? [
        { label: '新增任务', value: stats.created, hint: '周期内创建的任务' },
        { label: '完成任务', value: stats.completed, hint: '周期内完成的任务' },
        { label: '确认汇报', value: stats.reportsConfirmed, hint: '周期内确认的完成汇报' },
        { label: '新增知识', value: stats.entriesAdded, hint: '沉淀进组织知识库的条目' },
        { label: '新增交付物', value: stats.filesAdded, hint: '知识条目关联的原件（去重）' },
      ]
    : [];

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        enclaveOpen
        onOpenEnclave={() => navigate('/enclave')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
        onOpenMarket={() => navigate('/market')}
        onOpenInspirations={() => navigate('/inspirations')}
      />

      <div className="flex-1 min-h-0 px-2 flex flex-col overflow-hidden">
        <div
          className="shrink-0 rounded-lg overflow-hidden"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <EnclaveNavBar
            active="report"
            onRefresh={load}
            refreshing={loading}
            right={
              <EnclaveToolbarActions
                rootOpen={rootOpen}
                onToggleRoot={() => setRootOpen((v) => !v)}
              />
            }
          />
        </div>

        <EnclaveFilesRootPanel open={rootOpen} />

        {/* 周期选择：固定周期（周/月/年）或自定义起止 */}
        <div className="shrink-0 px-1 py-2 flex items-center gap-2 flex-wrap">
          {(
            [
              { k: 'week', label: '本周' },
              { k: 'month', label: '本月' },
              { k: 'year', label: '年报' },
              { k: 'custom', label: '自定义' },
            ] as { k: PeriodKind; label: string }[]
          ).map(({ k, label }) => {
            const on = kind === k;
            return (
              <button
                key={k}
                onClick={() => {
                  setKind(k);
                  setOffset(0);
                }}
                className="text-xs px-2.5 py-1 rounded-lg"
                style={{
                  backgroundColor: on ? 'var(--accent)' : 'var(--bg-tertiary)',
                  color: on ? '#fff' : 'var(--text-secondary)',
                  border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                }}
              >
                {label}
              </button>
            );
          })}
          {kind === 'custom' ? (
            <div
              className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <input
                type="date"
                value={fromDate}
                max={toDate}
                onChange={(e) => setFromDate(e.target.value)}
                className="bg-transparent outline-none text-xs"
                style={{ color: 'var(--text-secondary)' }}
              />
              <span style={{ color: 'var(--text-tertiary)' }}>至</span>
              <input
                type="date"
                value={toDate}
                min={fromDate}
                onChange={(e) => setToDate(e.target.value)}
                className="bg-transparent outline-none text-xs"
                style={{ color: 'var(--text-secondary)' }}
              />
              {!range.valid && (
                <span style={{ color: 'var(--status-danger)' }}>· {range.label}</span>
              )}
            </div>
          ) : (
            <div
              className="flex items-center gap-1 rounded-lg px-1 py-0.5"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <button
                onClick={() => setOffset((v) => v - 1)}
                className="p-0.5 rounded"
                style={{ color: 'var(--text-secondary)' }}
                title="上一周期"
              >
                <ChevronLeft size={13} />
              </button>
              <span className="text-xs tabular-nums" style={{ color: 'var(--text-secondary)' }}>
                {range.label}
              </span>
              <button
                onClick={() => setOffset((v) => Math.min(0, v + 1))}
                disabled={offset >= 0}
                className="p-0.5 rounded"
                style={{ color: 'var(--text-secondary)', opacity: offset >= 0 ? 0.35 : 1 }}
                title="下一周期"
              >
                <ChevronRight size={13} />
              </button>
            </div>
          )}
          <div className="flex-1" />
          {loading && <Loader2 size={13} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
          <button
            onClick={copyMarkdown}
            disabled={!stats}
            className="pd-btn text-xs px-2 py-1 rounded-lg flex items-center gap-1"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
              opacity: stats ? 1 : 0.5,
            }}
            title="把当前周期复制成 Markdown（可直接粘到群里或文档里）"
          >
            <ClipboardCopy size={12} />
            复制 Markdown
          </button>
          <button
            onClick={exportCsv}
            disabled={!stats}
            className="pd-btn text-xs px-2 py-1 rounded-lg flex items-center gap-1"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
              opacity: stats ? 1 : 0.5,
            }}
            title="导出当前周期（概览 + 成员 + 完成明细）为 CSV"
          >
            <Download size={12} />
            导出 CSV
          </button>
        </div>

        <div className="flex-1 min-h-0 overflow-auto flex flex-col gap-2 pb-2">
          {!activeOrgId && (
            <div
              className="rounded-lg p-6 text-center text-xs"
              style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)', color: 'var(--text-tertiary)' }}
            >
              先选择组织
            </div>
          )}

          {error && (
            <div
              className="rounded-lg px-3 py-2 text-xs"
              style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)', color: 'var(--status-danger)' }}
            >
              {error}
            </div>
          )}

          {stats && (
            <>
              {/* 概览 */}
              <div className="grid grid-cols-5 gap-2">
                {cards.map((c) => (
                  <div
                    key={c.label}
                    className="rounded-lg px-3 py-2.5"
                    style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
                    title={c.hint}
                  >
                    <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                      {c.label}
                    </div>
                    <div className="text-xl font-semibold tabular-nums mt-0.5" style={{ color: 'var(--text-primary)' }}>
                      {c.value}
                    </div>
                  </div>
                ))}
              </div>

              {/* 成员绩效数据 */}
              <div
                className="rounded-lg overflow-hidden"
                style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
              >
                <div
                  className="h-9 px-3 flex items-center gap-2"
                  style={{ borderBottom: '1px solid var(--border)' }}
                >
                  <BarChart3 size={13} style={{ color: 'var(--accent)' }} />
                  <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                    成员数据
                  </span>
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {stats.members.length}
                  </span>
                  <div className="flex-1" />
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    只计数，不做评分；逾期按当前未完成且已过截止时间统计
                  </span>
                </div>
                <div className="overflow-x-auto">
                  <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
                    <thead>
                      <tr style={{ color: 'var(--text-tertiary)' }}>
                        <th className="text-left px-3 py-1.5 font-normal">成员</th>
                        <th className="text-right px-3 py-1.5 font-normal">完成</th>
                        <th className="text-right px-3 py-1.5 font-normal">进行中</th>
                        <th className="text-right px-3 py-1.5 font-normal">待确认</th>
                        <th className="text-right px-3 py-1.5 font-normal">逾期</th>
                        <th className="text-right px-3 py-1.5 font-normal">平均耗时</th>
                      </tr>
                    </thead>
                    <tbody>
                      {stats.members.map((m) => (
                        <tr
                          key={m.userId}
                          style={{ color: 'var(--text-secondary)', borderTop: '1px solid var(--border)' }}
                        >
                          <td className="px-3 py-1.5">{memberName(m.userId)}</td>
                          <td className="px-3 py-1.5 text-right tabular-nums">{m.completed}</td>
                          <td className="px-3 py-1.5 text-right tabular-nums">{m.inProgress}</td>
                          <td className="px-3 py-1.5 text-right tabular-nums">{m.submitted}</td>
                          <td
                            className="px-3 py-1.5 text-right tabular-nums"
                            style={{ color: m.overdue > 0 ? 'var(--status-danger)' : 'var(--text-secondary)' }}
                          >
                            {m.overdue}
                          </td>
                          <td className="px-3 py-1.5 text-right tabular-nums">{hoursLabel(m.avgHours)}</td>
                        </tr>
                      ))}
                      {stats.members.length === 0 && (
                        <tr>
                          <td
                            colSpan={6}
                            className="px-3 py-5 text-center"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            该周期没有指派给成员的任务
                          </td>
                        </tr>
                      )}
                    </tbody>
                  </table>
                </div>
              </div>

              {/* 周期内完成明细：周报正文的素材 */}
              <div
                className="rounded-lg overflow-hidden"
                style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
              >
                <div
                  className="h-9 px-3 flex items-center gap-2"
                  style={{ borderBottom: '1px solid var(--border)' }}
                >
                  <CheckCircle2 size={13} style={{ color: 'var(--accent)' }} />
                  <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                    已完成任务
                  </span>
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {stats.completedTasks.length}
                  </span>
                  <div className="flex-1" />
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    最多展示 200 条；「未沉淀」表示该任务还没有对应的知识条目
                  </span>
                </div>
                <div className="overflow-x-auto">
                  <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
                    <thead>
                      <tr style={{ color: 'var(--text-tertiary)' }}>
                        <th className="text-left px-3 py-1.5 font-normal">任务</th>
                        <th className="text-left px-3 py-1.5 font-normal">负责人</th>
                        <th className="text-left px-3 py-1.5 font-normal">完成时间</th>
                        <th className="text-left px-3 py-1.5 font-normal">产出</th>
                      </tr>
                    </thead>
                    <tbody>
                      {stats.completedTasks.map((t) => (
                        <tr
                          key={t.taskId}
                          style={{ color: 'var(--text-secondary)', borderTop: '1px solid var(--border)' }}
                        >
                          <td className="px-3 py-1.5 max-w-[420px] truncate" title={t.title}>
                            {t.title}
                          </td>
                          <td className="px-3 py-1.5 whitespace-nowrap">{memberName(t.assigneeId)}</td>
                          <td className="px-3 py-1.5 whitespace-nowrap tabular-nums">{fmtDone(t.completedAt)}</td>
                          <td className="px-3 py-1.5 whitespace-nowrap">
                            {t.hasOutput ? (
                              <span style={{ color: 'var(--status-success, #2f9e44)' }}>已沉淀</span>
                            ) : (
                              <span style={{ color: 'var(--text-tertiary)' }}>未沉淀</span>
                            )}
                          </td>
                        </tr>
                      ))}
                      {stats.completedTasks.length === 0 && (
                        <tr>
                          <td
                            colSpan={4}
                            className="px-3 py-5 text-center"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            该周期没有完成的任务
                          </td>
                        </tr>
                      )}
                    </tbody>
                  </table>
                </div>
              </div>
            </>
          )}
        </div>
      </div>

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
