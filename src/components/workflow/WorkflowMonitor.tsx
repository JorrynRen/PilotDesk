/**
 * WorkflowMonitor — 工作流执行实例面板
 *
 * 紧凑单行显示实例信息，支持分页和节点执行详情展开。
 */

import React, { useEffect, useMemo, useState, useRef } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { Activity, Clock, CheckCircle, XCircle, AlertCircle, Ban, AlertTriangle, Square, Eye, ChevronDown, ChevronRight, Play, Flag, Timer, Trash2, Search, X, Filter, RefreshCw } from 'lucide-react';
import { Select } from '../common/Select';
import { WorkflowOutputCard, CollapsibleSection, ValuePreview } from './WorkflowOutputCard';

interface Props {
  onViewDefinition: (definitionId: string) => void;
  /** 请求删除执行记录（单条传 1 个 id；调用方负责二次确认与提示） */
  onDeleteExecutions: (executionIds: string[], summary: string) => void;
}

const STATUS_CONFIG: Record<string, { label: string; color: string; bg: string; icon: React.ReactNode }> = {
  'pending': { label: '待触发', color: '#6B7280', bg: 'rgba(107,114,128,0.1)', icon: <Clock size={11} /> },
  'running': { label: '运行中', color: '#3B82F6', bg: 'rgba(59,130,246,0.1)', icon: <Activity size={11} /> },
  'paused': { label: '已暂停', color: '#F59E0B', bg: 'rgba(245,158,11,0.1)', icon: <AlertCircle size={11} /> },
  'success': { label: '成功', color: '#10B981', bg: 'rgba(16,185,129,0.1)', icon: <CheckCircle size={11} /> },
  'failed': { label: '失败', color: '#EF4444', bg: 'rgba(239,68,68,0.1)', icon: <XCircle size={11} /> },
  'cancelled': { label: '已取消', color: '#6B7280', bg: 'rgba(107,114,128,0.1)', icon: <Ban size={11} /> },
  'timeout': { label: '超时', color: '#EF4444', bg: 'rgba(239,68,68,0.1)', icon: <AlertTriangle size={11} /> },
};

const triggerLabel = (t: string) => ({ manual: '手动', cron: '定时', event: '事件' }[t] || t);

const formatFullTime = (ts: number | undefined | null) => {
  if (!ts) return '--';
  return new Date(Number(ts) * 1000).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' });
};

const formatDuration = (start?: number, end?: number) => {
  if (!start) return '--';
  const endTime = end || Math.floor(Date.now() / 1000);
  const diff = Math.max(0, endTime - start);
  if (diff < 60) return `${diff}秒`;
  if (diff < 3600) return `${Math.floor(diff / 60)}分${diff % 60}秒`;
  return `${Math.floor(diff / 3600)}时${Math.floor((diff % 3600) / 60)}分${diff % 60}秒`;
};

const PAGE_SIZE = 10;

const STATUS_OPTIONS = [
  { key: 'all', label: '全部', color: 'var(--text-secondary)', bg: 'transparent' },
  { key: 'running', label: '运行中', color: '#3B82F6', bg: 'rgba(59,130,246,0.1)' },
  { key: 'success', label: '成功', color: '#10B981', bg: 'rgba(16,185,129,0.1)' },
  { key: 'failed', label: '失败', color: '#EF4444', bg: 'rgba(239,68,68,0.1)' },
  { key: 'pending', label: '待触发', color: '#6B7280', bg: 'rgba(107,114,128,0.1)' },
  { key: 'paused', label: '已暂停', color: '#F59E0B', bg: 'rgba(245,158,11,0.1)' },
  { key: 'cancelled', label: '已取消', color: '#6B7280', bg: 'rgba(107,114,128,0.1)' },
  { key: 'timeout', label: '超时', color: '#EF4444', bg: 'rgba(239,68,68,0.1)' },
] as const;

const TRIGGER_OPTIONS = [
  { key: 'all', label: '全部触发' },
  { key: 'manual', label: '手动' },
  { key: 'cron', label: '定时' },
  { key: 'event', label: '事件' },
] as const;

const ALL_DEFINITION_ID = '__all__';

/** 只有已终态的实例才允许删除（与后端 delete_executions_inner 同一口径）：
 *  未结束实例的执行记录一旦删掉，实例会从列表消失却仍在跑，且再也取消不掉。 */
const TERMINAL_STATUSES = ['success', 'failed', 'cancelled', 'timeout'];
const isDeletableStatus = (status: string) => TERMINAL_STATUSES.includes(status);

/**
 * 运行上下文查看器：默认「预览」（按节点归一化展示产出），可切到「原始数据」看全量 JSON。
 *
 * 预览跳过 `__`（内部变量）与 `gate_output.*`（引擎按阶段合并的中间产物，内容与节点产重复）；
 * 「原始数据」不做任何过滤，保证信息不丢。
 */
const ContextViewer: React.FC<{ context: Record<string, unknown> }> = ({ context }) => {
  const [mode, setMode] = useState<'preview' | 'raw'>('preview');
  const entries = useMemo(() => Object.entries(context || {}), [context]);
  const previewEntries = useMemo(
    () => entries.filter(([k]) => !k.startsWith('__') && !k.startsWith('gate_output.')),
    [entries],
  );
  const hiddenCount = entries.length - previewEntries.length;

  if (entries.length === 0) {
    return <span style={{ color: 'var(--text-tertiary)' }}>暂无上下文数据</span>;
  }

  return (
    <div>
      <div className="flex items-center gap-1 mb-2">
        {(['preview', 'raw'] as const).map((m) => (
          <button
            key={m}
            onClick={() => setMode(m)}
            className="pd-btn text-[10px] px-1.5 py-0.5 rounded transition-colors"
            style={{
              border: '1px solid var(--border)',
              backgroundColor: mode === m ? 'var(--accent-light)' : 'transparent',
              color: mode === m ? 'var(--accent)' : 'var(--text-tertiary)',
            }}
          >
            {m === 'preview' ? '预览' : '原始数据'}
          </button>
        ))}
      </div>

      {mode === 'raw' ? (
        <pre style={{ margin: 0, fontSize: 10, color: 'var(--text-secondary)', whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontFamily: 'var(--font-mono, "Cascadia Code", "Fira Code", monospace)', lineHeight: 1.5 }}>
          {JSON.stringify(context, null, 2)}
        </pre>
      ) : (
        <div className="space-y-2.5">
          {previewEntries.map(([key, value]) => (
            <div key={key}>
              <div
                className="text-[9px] mb-0.5"
                style={{ color: 'var(--text-tertiary)', fontFamily: 'var(--font-mono, "Cascadia Code", monospace)' }}
              >
                {key}
              </div>
              <ValuePreview value={value} />
            </div>
          ))}
          {hiddenCount > 0 && (
            <div className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
              已隐藏 {hiddenCount} 个内部键（如 gate_output.*），切到「原始数据」可看全部
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export const WorkflowMonitor: React.FC<Props> = ({ onViewDefinition, onDeleteExecutions }) => {
  const { instances, loading, loadInstances, cancelWorkflow } = useWorkflowStore();
  const [expandedInstance, setExpandedInstance] = useState<string | null>(null);
  const [tooltipInst, setTooltipInst] = useState<string | null>(null);
  const [page, setPage] = useState(0);

  // ---- 批量删除 ----
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  // 轮询回调在挂载时注册一次，用 ref 读取最新批量态（批量时不静默刷新，避免列表在勾选中跳动）
  const batchModeRef = useRef(batchMode);
  useEffect(() => { batchModeRef.current = batchMode; }, [batchMode]);

  // ---- 刷新 / 防抖状态 ----
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState<boolean>(false);
  const loadingRef = useRef<boolean>(false); // 防止手动/定时器并发调用
  /** loadingRef 的渲染期镜像：渲染期不能读 ref（react-hooks/refs），并发去重仍走 ref */
  const [inFlight, setInFlight] = useState<boolean>(false);

  // ---- 筛选状态 ----
  const [filterDefinitionId, setFilterDefinitionId] = useState<string>(ALL_DEFINITION_ID);
  const [filterStatus, setFilterStatus] = useState<string>('all');
  const [filterTrigger, setFilterTrigger] = useState<string>('all');
  const [filterKeyword, setFilterKeyword] = useState<string>('');

  // 安全刷新（并发去重 + 成功后写 lastUpdatedAt）
  const safeRefresh = async (silent: boolean) => {
    if (loadingRef.current) return;
    loadingRef.current = true;
    // 先让出同步执行栈再翻"忙"标记：effect 体内同步 setState 会触发级联渲染
    // （react-hooks/set-state-in-effect）。这里只延后一个微任务，标记的可见时机不变。
    await Promise.resolve();
    setInFlight(true);
    if (!silent) setRefreshing(true);
    try {
      await loadInstances(undefined, silent);
      setLastUpdatedAt(Date.now());
    } finally {
      if (!silent) setRefreshing(false);
      loadingRef.current = false;
      setInFlight(false);
    }
  };

  // 首次加载：显式 loading，用户知道在初始化
  // 之后每 30s 轮询：仅当存在 running / paused / pending 的活实例才执行静默刷新
  useEffect(() => {
    // effect 体内不允许同步 setState（react-hooks/set-state-in-effect）：把首次刷新推迟一个微任务，
    // 仍在同一帧内执行，观感与原先一致
    queueMicrotask(() => { void safeRefresh(false); });

    const interval = setInterval(() => {
      // 批量勾选中不刷新：列表若在勾选过程中变化（跨页选中、行消失）会让人怀疑自己选错了
      if (batchModeRef.current) return;
      const hasAlive = instances.some(
        i => i.status === 'running' || i.status === 'paused' || i.status === 'pending'
      );
      if (hasAlive) {
        void safeRefresh(true);
      }
    }, 30000);
    return () => clearInterval(interval);
    // 仅挂载时初始化一次；依赖保持空数组，useRef 直接访问 instances 避免不停重建定时器
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 生成候选工作流（definitionId -> name），按出现顺序去重
  const definitionList = useMemo(() => {
    const map = new Map<string, string>();
    instances.forEach(i => {
      if (i.definitionId && !map.has(i.definitionId)) {
        map.set(i.definitionId, i.definitionName || i.definitionId);
      }
    });
    return Array.from(map.entries()).map(([id, name]) => ({ id, name }));
  }, [instances]);

  // 多条件过滤 + 排序（最新优先）
  const filtered = useMemo(() => {
    const kw = filterKeyword.trim().toLowerCase();
    return instances
      .filter(i => {
        if (filterDefinitionId !== ALL_DEFINITION_ID && i.definitionId !== filterDefinitionId) return false;
        if (filterStatus !== 'all' && i.status !== filterStatus) return false;
        if (filterTrigger !== 'all' && i.trigger !== filterTrigger) return false;
        if (kw) {
          const hit =
            (i.definitionName || '').toLowerCase().includes(kw) ||
            (i.id || '').toLowerCase().includes(kw) ||
            (i.error || '').toLowerCase().includes(kw);
          if (!hit) return false;
        }
        return true;
      })
      .sort((a, b) => (b.createdAt || 0) - (a.createdAt || 0));
  }, [instances, filterDefinitionId, filterStatus, filterTrigger, filterKeyword]);

  const hasFilter = filterDefinitionId !== ALL_DEFINITION_ID || filterStatus !== 'all' || filterTrigger !== 'all' || filterKeyword.trim() !== '';

  // 清空筛选
  const clearFilters = () => {
    setFilterDefinitionId(ALL_DEFINITION_ID);
    setFilterStatus('all');
    setFilterTrigger('all');
    setFilterKeyword('');
  };

  // ── 批量选择 ──
  // 以下三类"派生重置"原先都写成 effect（effect 内同步 setState 会触发级联渲染），
  // 改用 React 官方的「渲染期调整状态」写法：在发现条件变化的同一次渲染里直接修正，
  // 最终渲染结果与原先一致。
  //
  // 1) 过滤条件变化 → 回第 0 页（用户主动改筛选，保持"从头看结果"的直觉）并清空选择
  //    （否则会出现"看着选的是这批、实际还含被筛掉的行"）
  const filtersKey = JSON.stringify([filterDefinitionId, filterStatus, filterTrigger, filterKeyword]);
  const [prevFiltersKey, setPrevFiltersKey] = useState(filtersKey);
  if (prevFiltersKey !== filtersKey) {
    setPrevFiltersKey(filtersKey);
    setPage(0);
    setSelectedIds(new Set());
  }
  // 2) 列表刷新后剔除已消失 / 已不可选的选中项，避免选中态指向不存在的行
  if (selectedIds.size > 0) {
    const alive = new Set(instances.filter(i => isDeletableStatus(i.status)).map(i => i.id));
    const next = new Set([...selectedIds].filter(id => alive.has(id)));
    if (next.size !== selectedIds.size) setSelectedIds(next);
  }
  // 3) 列表清空时退出批量模式，避免停留在"看不见的批量态"
  if (batchMode && instances.length === 0) {
    setBatchMode(false);
    setSelectedIds(new Set());
  }

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  // 自动纠正页码：仅当当前 page 超出总页数时才修正（静默刷新导致 filtered 减少不会把用户弹回第 1 页）
  const safePage = Math.min(page, totalPages - 1);
  useEffect(() => {
    if (safePage !== page) {
      const t = setTimeout(() => setPage(safePage), 0);
      return () => clearTimeout(t);
    }
  }, [safePage, page]);
  const paged = filtered.slice(safePage * PAGE_SIZE, (safePage + 1) * PAGE_SIZE);

  // 全选只覆盖可删（已终态）的实例；页内 / 全部筛选结果两个口径
  const selectPage = () =>
    setSelectedIds(new Set(paged.filter(i => isDeletableStatus(i.status)).map(i => i.id)));
  const selectAllFiltered = () =>
    setSelectedIds(new Set(filtered.filter(i => isDeletableStatus(i.status)).map(i => i.id)));
  const toggleSelect = (id: string) =>
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });

  /** 当前筛选结果里未结束（不可删）的条数：提示"全选"会跳过它们 */
  const undeletableCount = filtered.filter(i => !isDeletableStatus(i.status)).length;

  /** 交给调用方去二次确认：附状态分布摘要，让用户在弹窗里看清删的是什么 */
  const handleBatchDelete = () => {
    const byStatus = new Map<string, number>();
    filtered
      .filter(i => selectedIds.has(i.id))
      .forEach(i => byStatus.set(i.status, (byStatus.get(i.status) ?? 0) + 1));
    const summary = Array.from(byStatus.entries())
      .map(([s, n]) => `${STATUS_CONFIG[s]?.label ?? s} ${n}`)
      .join(' / ');
    onDeleteExecutions([...selectedIds], summary);
  };

  // 格式化"上次更新时间戳"
  const formatLastUpdate = (ts: number) => {
    const d = new Date(ts);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  };

  return (
    <div>
      {loading && instances.length === 0 ? (
        <div className="flex items-center justify-center h-32 text-xs" style={{ color: 'var(--text-tertiary)' }}>
          加载执行实例中...
        </div>
      ) : instances.length === 0 ? (
        // 空态与「工作流定义」页保持同一形态：只有图标 + 文案，不套边框卡片
        <div className="flex flex-col items-center justify-center h-48 gap-2">
          <Activity size={32} style={{ opacity: 0.25, color: 'var(--text-tertiary)' }} />
          <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无执行实例</div>
        </div>
      ) : (
        <div>
          {/* ── 筛选工具栏 ── */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 8,
              padding: '10px 12px',
              marginBottom: 12,
              borderRadius: 6,
              backgroundColor: 'var(--bg-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            {/* 单行：结构化筛选控件 + 状态药丸标签（宽屏一行，窄屏自动 wrap） */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
              <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                <Filter size={12} style={{ color: 'var(--text-tertiary)' }} />
                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)', fontWeight: 500 }}>筛选：</span>
              </div>

              {/* 工作流下拉 */}
              <Select
                value={filterDefinitionId}
                onChange={setFilterDefinitionId}
                options={[
                  { value: ALL_DEFINITION_ID, label: `全部工作流 (${definitionList.length})` },
                  ...definitionList.map(d => ({ value: d.id, label: d.name })),
                ]}
                size="sm"
                className="shrink-0"
                style={{ minWidth: 130, maxWidth: 220 }}
                title="按工作流筛选"
              />

              {/* 触发方式下拉 */}
              <Select
                value={filterTrigger}
                onChange={setFilterTrigger}
                options={TRIGGER_OPTIONS.map(t => ({ value: t.key, label: t.label }))}
                size="sm"
                className="shrink-0"
                style={{ minWidth: 88 }}
                title="按触发方式筛选"
              />

              {/* 状态药丸标签（容器固定 28px，与同行 Select(size=sm) 等高） */}
              <div
                aria-label="执行状态筛选"
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 3,
                  flexWrap: 'nowrap',
                  padding: '2px 4px',
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-primary)',
                  borderRadius: 4,
                  height: 28,
                  overflowX: 'auto',
                  maxWidth: 430,
                  scrollbarWidth: 'thin',
                }}
              >
                {STATUS_OPTIONS.map(opt => {
                  const active = filterStatus === opt.key;
                  return (
                    <button
                      key={opt.key}
                      onClick={() => setFilterStatus(opt.key)}
                      className="text-[10px] px-2 py-0.5 rounded-full transition-colors whitespace-nowrap"
                      style={{
                        border: active ? '1px solid ' + (opt.key === 'all' ? 'var(--accent)' : opt.color) : '1px solid transparent',
                        backgroundColor: active ? (opt.key === 'all' ? 'rgba(var(--accent-rgb, var(--accent)), 0.12)' : opt.bg) : 'transparent',
                        color: active ? (opt.key === 'all' ? 'var(--accent)' : opt.color) : 'var(--text-tertiary)',
                        fontWeight: active ? 500 : 400,
                        flexShrink: 0,
                      }}
                    >
                      {opt.label}
                    </button>
                  );
                })}
              </div>

              {/* 关键字搜索（高度对齐同行 Select(size=sm) 的 28px，不再靠内容撑高） */}
              <div
                className="pd-field"
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4,
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-primary)',
                  borderRadius: 4,
                  height: 28,
                  padding: '0 6px',
                  flex: '1 1 180px',
                  minWidth: 160,
                  maxWidth: 300,
                }}
              >
                <Search size={11} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
                <input
                  value={filterKeyword}
                  onChange={e => setFilterKeyword(e.target.value)}
                  placeholder="搜索工作流名 / 实例ID / 错误信息..."
                  className="flex-1 min-w-0 outline-none text-[11px]"
                  style={{ background: 'transparent', border: 'none', color: 'var(--text-primary)' }}
                />
                {filterKeyword && (
                  <button
                    onClick={() => setFilterKeyword('')}
                    className="pd-btn p-0.5 rounded hover:opacity-80 transition-opacity"
                    style={{ color: 'var(--text-tertiary)', background: 'transparent' }}
                    title="清除关键字"
                  >
                    <X size={11} />
                  </button>
                )}
              </div>

              {/* 清除全部筛选 */}
              <button
                onClick={clearFilters}
                disabled={!hasFilter}
                className="pd-btn px-2 py-1 h-7 text-[11px] rounded transition-colors whitespace-nowrap inline-flex items-center"
                style={{
                  border: '1px solid var(--border)',
                  background: hasFilter ? 'var(--bg-secondary)' : 'var(--bg-tertiary)',
                  color: hasFilter ? 'var(--text-primary)' : 'var(--text-tertiary)',
                  cursor: hasFilter ? 'pointer' : 'not-allowed',
                  opacity: hasFilter ? 1 : 0.5,
                }}
              >
                清除筛选
              </button>

              {/* 手动刷新 + 时间戳（与清除筛选右对齐） */}
              <div
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4,
                  marginLeft: 'auto',
                  flexShrink: 0,
                }}
              >
                {lastUpdatedAt && (
                  <span
                    className="text-[10px] flex items-center gap-1"
                    style={{ color: 'var(--text-tertiary)' }}
                    title={`最后更新：${new Date(lastUpdatedAt).toLocaleString()}`}
                  >
                    <Clock size={10} />
                    {formatLastUpdate(lastUpdatedAt)}
                  </span>
                )}
                <button
                  onClick={() => { setBatchMode(v => !v); setSelectedIds(new Set()); }}
                  className="pd-btn px-2 py-1 h-7 text-[11px] rounded transition-colors inline-flex items-center gap-1"
                  style={{
                    border: '1px solid var(--border)',
                    background: batchMode ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                    color: batchMode ? 'var(--accent)' : 'var(--text-secondary)',
                  }}
                  title="批量删除执行记录"
                >
                  <Trash2 size={11} />
                  批量
                </button>
                <button
                  onClick={() => void safeRefresh(false)}
                  disabled={refreshing || inFlight}
                  className="pd-btn px-2 py-1 h-7 text-[11px] rounded transition-colors inline-flex items-center gap-1"
                  style={{
                    border: '1px solid var(--border)',
                    background: 'var(--bg-secondary)',
                    color: 'var(--accent)',
                    opacity: (refreshing || inFlight) ? 0.6 : 1,
                  }}
                  title="立即刷新实例列表"
                >
                  <RefreshCw size={11} style={{ animation: (refreshing || inFlight) ? 'spin 0.8s linear infinite' : undefined }} />
                  刷新
                </button>
              </div>
            </div>

            {/* ── 批量操作条（勾选口径：全选只覆盖已结束的实例） ── */}
            {batchMode && (
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                  flexWrap: 'wrap',
                  paddingTop: 8,
                  borderTop: '1px solid var(--border)',
                }}
              >
                <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                  已选 <b style={{ color: 'var(--accent)' }}>{selectedIds.size}</b> 条
                </span>
                <button
                  onClick={selectPage}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                  title="选中本页所有已结束的实例"
                >
                  全选本页 ({paged.filter(i => isDeletableStatus(i.status)).length})
                </button>
                <button
                  onClick={selectAllFiltered}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                  title="选中当前筛选结果中所有已结束的实例（跨页）"
                >
                  全选筛选结果 ({filtered.filter(i => isDeletableStatus(i.status)).length})
                </button>
                <button
                  onClick={() => setSelectedIds(new Set())}
                  disabled={selectedIds.size === 0}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: 'var(--bg-primary)',
                    color: 'var(--text-secondary)',
                    opacity: selectedIds.size === 0 ? 0.5 : 1,
                  }}
                >
                  清空
                </button>
                {undeletableCount > 0 && (
                  <span
                    className="text-[10px]"
                    style={{ color: 'var(--text-tertiary)' }}
                    title="未结束的实例不能删除：删掉它们的执行记录会让实例从列表消失却仍在跑，而且再也取消不掉。请先停止或等待结束。"
                  >
                    已跳过未结束 {undeletableCount} 条
                  </span>
                )}
                <div style={{ flex: 1 }} />
                <button
                  onClick={handleBatchDelete}
                  disabled={selectedIds.size === 0}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: selectedIds.size === 0 ? 'var(--bg-tertiary)' : '#ef4444',
                    color: selectedIds.size === 0 ? 'var(--text-tertiary)' : '#fff',
                  }}
                >
                  删除 ({selectedIds.size})
                </button>
                <button
                  onClick={() => { setBatchMode(false); setSelectedIds(new Set()); }}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                >
                  退出
                </button>
              </div>
            )}
            </div>

          {/* ── Instance rows ── */}
          {filtered.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-32 gap-2" style={{ backgroundColor: 'var(--bg-secondary)', borderRadius: 8, border: '1px solid var(--border)' }}>
              <Filter size={20} style={{ color: 'var(--text-tertiary)', opacity: 0.5 }} />
              <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                {hasFilter ? '没有符合筛选条件的执行实例' : '暂无执行实例'}
              </span>
              {hasFilter && (
                <button
                  onClick={clearFilters}
                  className="pd-btn px-3 py-1 text-[11px] rounded transition-colors"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--accent)' }}
                >清除筛选条件</button>
              )}
            </div>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            {paged.map((instance) => {
              const cfg = STATUS_CONFIG[instance.status] || STATUS_CONFIG['pending'];
              const progress = instance.completionRate ?? 0;
              const isExpanded = expandedInstance === instance.id;
              const isHovered = tooltipInst === instance.id;
              const selectable = isDeletableStatus(instance.status);
              const checked = selectedIds.has(instance.id);

              return (
                <div key={instance.id}>
                  {/* ── Card ── */}
                  <div
                    onClick={batchMode && selectable ? () => toggleSelect(instance.id) : undefined}
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 4,
                      padding: '12px',
                      borderRadius: 6,
                      backgroundColor: checked ? 'rgba(88,166,255,0.08)' : isExpanded ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      border: checked ? '1px solid var(--accent)' : '1px solid var(--border)',
                      fontSize: 12,
                      position: 'relative',
                      cursor: batchMode && selectable ? 'pointer' : undefined,
                    }}
                  >
                    {/* ── Row 1: Name + Actions ── */}
                    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                      {batchMode && (
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={!selectable}
                          onChange={() => toggleSelect(instance.id)}
                          onClick={(e) => e.stopPropagation()}
                          title={selectable ? '选中' : '未结束的实例不能删除：请先停止或等待结束'}
                          style={{ flexShrink: 0, cursor: selectable ? 'pointer' : 'not-allowed' }}
                        />
                      )}
                      <Activity size={14} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                      <span
                        className="truncate font-medium"
                        style={{
                          color: 'var(--text-primary)',
                          flex: '0 1 280px',
                          minWidth: 0,
                          opacity: batchMode && !selectable ? 0.55 : 1,
                        }}
                        title={instance.definitionName}
                      >
                        {instance.definitionName}
                      </span>

                      {/* Spacer */}
                      <div style={{ flex: 1 }} />

                      {/* Actions（批量模式下收起：避免与勾选语义混淆、误触） */}
                      {!batchMode && (
                      <div style={{ display: 'flex', gap: 4, flexShrink: 0 }}>
                        {instance.status === 'running' && (
                          <button
                            onClick={() => cancelWorkflow(instance.id)}
                            className="text-[10px] px-2 py-0.5 rounded transition-colors"
                            style={{ border: '1px solid var(--border)', background: 'transparent', color: '#EF4444' }}
                            title="停止"
                          >
                            <Square size={10} />
                          </button>
                        )}
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            onDeleteExecutions([instance.id], `${cfg.label} 1`);
                          }}
                          className="text-[10px] px-2 py-0.5 rounded transition-colors"
                          style={{ border: '1px solid var(--border)', background: 'transparent', color: 'var(--text-tertiary)' }}
                          title={selectable ? '删除执行记录' : '未结束的实例不能删除：请先停止'}
                          disabled={!selectable}
                        >
                          <Trash2 size={10} />
                        </button>
                        <button
                          onClick={() => onViewDefinition(instance.definitionId)}
                          className="text-[10px] px-2 py-0.5 rounded transition-colors"
                          style={{ border: '1px solid var(--border)', background: 'transparent', color: 'var(--text-secondary)' }}
                          title="查看定义"
                        >
                          <Eye size={10} />
                        </button>
                        <button
                          onClick={() => setExpandedInstance(isExpanded ? null : instance.id)}
                          className="text-[10px] px-2 py-0.5 rounded transition-colors"
                          style={{ border: '1px solid var(--border)', background: 'transparent', color: 'var(--text-tertiary)' }}
                          title="输出上下文"
                        >
                          {isExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                        </button>
                      </div>
                      )}
                    </div>

                    {/* ── Row 2: Status + Trigger + Progress + Times ── */}
                    <div style={{ display: 'flex', alignItems: 'center', gap: 12, paddingLeft: 22 }}>
                      {/* Status - with error tooltip */}
                      <div
                        style={{ position: 'relative', flexShrink: 0 }}
                        onMouseEnter={() => setTooltipInst(instance.id)}
                        onMouseLeave={() => setTooltipInst(null)}
                      >
                        <span
                          style={{
                            display: 'inline-flex',
                            alignItems: 'center',
                            gap: 3,
                            padding: '1px 7px',
                            borderRadius: 9999,
                            fontSize: 11,
                            fontWeight: 500,
                            color: cfg.color,
                            backgroundColor: cfg.bg,
                            cursor: instance.error ? 'help' : 'default',
                          }}
                        >
                          {cfg.icon}
                          {cfg.label}
                        </span>
                        {/* Error tooltip */}
                        {instance.error && isHovered && (
                          <div
                            style={{
                              position: 'absolute',
                              bottom: '100%',
                              left: 0,
                              marginBottom: 4,
                              padding: '4px 8px',
                              borderRadius: 4,
                              backgroundColor: 'var(--bg-primary)',
                              border: '1px solid rgba(239,68,68,0.3)',
                              fontSize: 11,
                              color: '#EF4444',
                              whiteSpace: 'nowrap',
                              maxWidth: 280,
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                              zIndex: 100,
                              boxShadow: '0 2px 8px rgba(0,0,0,0.2)',
                            }}
                          >
                            {instance.error}
                          </div>
                        )}
                      </div>

                      {/* Trigger tag */}
                      <span
                        style={{
                          display: 'inline-flex',
                          padding: '1px 5px',
                          borderRadius: 9999,
                          fontSize: 10,
                          color: 'var(--text-tertiary)',
                          backgroundColor: 'var(--bg-tertiary)',
                          flexShrink: 0,
                        }}
                      >
                        {triggerLabel(instance.trigger)}
                      </span>

                      {/* Progress bar */}
                      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, flexShrink: 0, minWidth: 100 }}>
                        
                        <span style={{ flex: 1, height: 6, borderRadius: 3, backgroundColor: 'var(--bg-tertiary)', overflow: 'hidden', minWidth: 48 }}>
                          <span style={{ display: 'block', height: '100%', borderRadius: 3, width: `${Math.round(progress * 100)}%`, backgroundColor: 'var(--accent)', transition: 'width 0.3s ease' }} />
                        </span>
                        <span className="text-[10px] font-medium" style={{ color: 'var(--accent)', minWidth: 28 }}>
                          {Math.round(progress * 100)}%
                        </span>
                        {/* 跳过节点数：完成度已计入跳过（条件分支未走不是失败），
                            单列出来避免"100% 但有些节点没跑"被误读 */}
                        {(instance.skippedCount ?? 0) > 0 && (
                          <span
                            className="text-[10px]"
                            style={{ color: 'var(--text-tertiary)', whiteSpace: 'nowrap' }}
                            title={`条件分支未命中 / 上游无产出的节点 ${instance.skippedCount} 个（已计入完成度）`}
                          >
                            跳过 {instance.skippedCount}
                          </span>
                        )}
                      </span>

                      {/* Start time */}
                      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2, flexShrink: 0 }}>
                        <Play size={10} style={{ color: 'var(--text-tertiary)' }} />
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }} title={'开始时间：' + formatFullTime(instance.startedAt)}>
                          {formatFullTime(instance.startedAt)}
                        </span>
                      </span>

                      {/* End time */}
                      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2, flexShrink: 0 }}>
                        <Flag size={10} style={{ color: 'var(--text-tertiary)' }} />
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }} title={'结束时间：' + formatFullTime(instance.completedAt)}>
                          {formatFullTime(instance.completedAt)}
                        </span>
                      </span>

                      {/* Duration */}
                      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2, flexShrink: 0 }}>
                        <Timer size={10} style={{ color: 'var(--text-tertiary)' }} />
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }} title={'耗时：' + formatDuration(instance.startedAt, instance.completedAt)}>
                          {formatDuration(instance.startedAt, instance.completedAt)}
                        </span>
                      </span>
                    </div>
                  </div>

                  {/* ── Expanded: 最终产出（主）+ 运行上下文（折叠） ── */}
                  {isExpanded && (
                    <div style={{ marginTop: 4, display: 'flex', flexDirection: 'column', gap: 6 }}>
                      <WorkflowOutputCard instance={instance} />
                      <CollapsibleSection title="运行上下文（各节点产出）" maxHeight={420}>
                        <ContextViewer context={instance.context} />
                      </CollapsibleSection>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          )}

          {/* ── Pagination ── */}
          {filtered.length > 0 && (
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: 12, paddingTop: 12, borderTop: '1px solid var(--border)' }}>
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                {hasFilter ? (
                  <>筛选结果 {filtered.length} 条（共 {instances.length} 条），第 {safePage + 1} / {totalPages} 页</>
                ) : (
                  <>共 {filtered.length} 条，第 {safePage + 1} / {totalPages} 页</>
                )}
              </span>
              <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                <button
                  onClick={() => setPage(0)}
                  disabled={safePage <= 0}
                  className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: safePage <= 0 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                    color: safePage <= 0 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                    cursor: safePage <= 0 ? 'not-allowed' : 'pointer',
                    opacity: safePage <= 0 ? 0.5 : 1,
                  }}
                >首页</button>
                <button
                  onClick={() => setPage(p => Math.max(0, p - 1))}
                  disabled={safePage <= 0}
                  className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: safePage <= 0 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                    color: safePage <= 0 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                    cursor: safePage <= 0 ? 'not-allowed' : 'pointer',
                    opacity: safePage <= 0 ? 0.5 : 1,
                  }}
                >上一页</button>
                <button
                  onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
                  disabled={safePage >= totalPages - 1}
                  className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: safePage >= totalPages - 1 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                    color: safePage >= totalPages - 1 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                    cursor: safePage >= totalPages - 1 ? 'not-allowed' : 'pointer',
                    opacity: safePage >= totalPages - 1 ? 0.5 : 1,
                  }}
                >下一页</button>
                <button
                  onClick={() => setPage(totalPages - 1)}
                  disabled={safePage >= totalPages - 1}
                  className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                  style={{
                    border: '1px solid var(--border)',
                    background: safePage >= totalPages - 1 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                    color: safePage >= totalPages - 1 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                    cursor: safePage >= totalPages - 1 ? 'not-allowed' : 'pointer',
                    opacity: safePage >= totalPages - 1 ? 0.5 : 1,
                  }}
                >末页</button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
