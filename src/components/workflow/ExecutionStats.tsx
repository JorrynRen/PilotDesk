/**
 * ExecutionStats — 工作流统计面板
 *
 * 展示工作流执行统计数据，包括工作流数量、总执行次数、成功率、
 * 执行时长分布、每日执行时间线、节点类型使用统计、Top 工作流排行与 Top 错误聚合。
 *
 * 优化点（参见 handoff）：
 *   - 首次加载才显示整面板 loading；切换 days 静默刷新（safeRefresh + loadingRef）
 *   - 刷新按钮带 RefreshCw 图标 + spin 动画 + disabled 状态
 *   - 核心指标卡片随 days 下拉联动（后端 get_workflow_stats 支持 days）
 *   - 状态分布补全 7 种状态（成功/失败/取消/运行中/待触发/已暂停/超时）
 *   - 节点类型分布展示失败率（红色失败比率条）
 *   - Top 工作流排行（最频繁 + 失败率最高）
 *   - Top 错误汇总
 *   - 90 天柱状图使用稀疏日期标签
 *   - 空数据耗时显示 "--"
 */

import React, { useEffect, useRef, useState, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  BarChart3, Clock, Zap, PieChart, Target,
  RefreshCw, AlertTriangle, Trophy, Flame, MinusCircle, CheckCircle, XCircle,
  PlayCircle, PauseCircle, Hourglass, Calendar, Hash, Layers,
} from 'lucide-react';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import { elide } from '../../utils/text';
import { Select } from '../common/Select';
import { errorMessage } from '../../utils/errorMessage';

// ── 类型定义 ──

interface WorkflowStats {
  totalExecutions: number;
  successCount: number;
  failedCount: number;
  cancelledCount: number;
  runningCount: number;
  pendingCount: number;
  pausedCount: number;
  timeoutCount: number;
  successRate: number;
  /** 耗时字段：null = 无可用样本；0 = 该执行不足 1 秒（事件时间戳为秒级） */
  avgDurationMs: number | null;
  maxDurationMs: number | null;
  minDurationMs: number | null;
  totalNodeExecutions: number;
  nodeFailedCount: number;
  last7DaysCount: number;
  last30DaysCount: number;
  lastNDaysCount: number;
  days: number;
  workflowCount?: number;
  // 区间内统计（按 days 窗口，days<=0 表示全量）
  rangeTotal: number;
  rangeSuccess: number;
  rangeFailed: number;
  rangeCancelled: number;
  rangeRunning: number;
  rangePending: number;
  rangePaused: number;
  rangeTimeout: number;
  rangeSuccessRate: number;
  rangeAvgDurationMs: number | null;
  rangeMaxDurationMs: number | null;
  rangeMinDurationMs: number | null;
}

interface TimelinePoint {
  date: string;
  total: number;
  success: number;
  failed: number;
  cancelled: number;
  /** 该分桶平均耗时（ms）；null = 该分桶无已完成执行 */
  avgDurationMs: number | null;
  granularity?: 'day' | 'week' | 'month';
}

interface NodeTypeStat {
  nodeType: string;
  count: number;
  failedCount: number;
  avgDurationMs: number;
}

interface TopWorkflowStat {
  definitionId: string;
  definitionName: string;
  total: number;
  success: number;
  failed: number;
  cancelled: number;
  failedRate: number;
}

interface TopErrorStat {
  error: string;
  count: number;
  lastOccurredAt: number;
}

// ── 状态元数据（颜色 / 图标 / 文案） ──

interface StatusMeta {
  key: 'success' | 'failed' | 'cancelled' | 'running' | 'pending' | 'paused' | 'timeout';
  label: string;
  color: string;
  icon: React.ReactNode;
}

const STATUS_META: StatusMeta[] = [
  { key: 'success',   label: '成功',   color: '#10B981', icon: <CheckCircle size={10} /> },
  { key: 'failed',    label: '失败',   color: '#EF4444', icon: <XCircle size={10} /> },
  { key: 'running',   label: '运行中', color: '#3B82F6', icon: <PlayCircle size={10} /> },
  { key: 'pending',   label: '待触发', color: '#9CA3AF', icon: <Hourglass size={10} /> },
  { key: 'paused',    label: '已暂停', color: '#F59E0B', icon: <PauseCircle size={10} /> },
  { key: 'cancelled', label: '取消',   color: '#6B7280', icon: <MinusCircle size={10} /> },
  { key: 'timeout',   label: '超时',   color: '#DC2626', icon: <AlertTriangle size={10} /> },
];

// ── 格式化工具 ──

/**
 * 耗时展示。
 * - null / 非有限值 = 无样本 → "--"
 * - 0 = 有样本但不足 1 秒（事件时间戳为秒级，短执行会取整为 0）→ "<1s"
 *   —— 不能把 0 当成"无数据"，否则任何一次秒内完成的执行都会让最短耗时永远显示为空
 */
function formatDuration(ms?: number | null): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return '--';
  if (ms === 0) return '<1s';
  if (ms < 1000) return `${ms.toFixed(0)}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60000);
  const seconds = Math.floor((ms % 60000) / 1000);
  return `${minutes}m ${seconds}s`;
}

function formatPercent(value: number): string {
  return `${value.toFixed(1)}%`;
}

function formatRelativeTime(unixSec: number): string {
  if (!unixSec) return '--';
  const diff = Math.floor(Date.now() / 1000 - unixSec);
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} 天前`;
  return new Date(unixSec * 1000).toISOString().slice(0, 10);
}

// ── 迷你柱状图组件（按选择天数固定柱宽，支持稀疏日期标签） ──

const BAR_GAP = 1;

/** Generate all dates in the last N days (ISO format YYYY-MM-DD) */
function generateDateRange(days: number): string[] {
  const result: string[] = [];
  const now = new Date();
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now);
    d.setDate(d.getDate() - i);
    result.push(d.toISOString().slice(0, 10));
  }
  return result;
}

/**
 * 把 YYYY-Www 格式的 ISO 周字符串转换为该周的起始日期（周一），用于排序和显示
 */
function weekLabelToDate(weekStr: string): Date {
  const m = weekStr.match(/^(\d{4})-W(\d{2})$/);
  if (!m) return new Date(0);
  const year = Number(m[1]);
  const week = Number(m[2]);
  // ISO 8601：第一周一定包含 1 月 4 日
  const jan4 = new Date(year, 0, 4);
  // 周一为每周第一天：周一=1
  const jan4DOW = (jan4.getDay() + 6) % 7;
  const mondayOfWeek1 = new Date(jan4);
  mondayOfWeek1.setDate(jan4.getDate() - jan4DOW);
  mondayOfWeek1.setDate(mondayOfWeek1.getDate() + (week - 1) * 7);
  return mondayOfWeek1;
}

/**
 * 按月/周粒度生成所有 bucket（补齐空段）；日粒度直接用现有generateDateRange
 */
function generateBuckets(maxBars: number, granularity: 'day' | 'week' | 'month'): string[] {
  if (granularity === 'day') return generateDateRange(maxBars);

  const now = new Date();
  const result: string[] = [];

  if (granularity === 'week') {
    // 取约等于 maxBars 天对应的周数（ceil(maxBars/7)，但 maxBars 这里是 effective_days）
    const weeks = Math.max(1, Math.ceil(maxBars / 7));
    for (let i = weeks - 1; i >= 0; i--) {
      const d = new Date(now);
      d.setDate(d.getDate() - i * 7);
      result.push(`${d.getFullYear()}-W${String((d.getTime() - new Date(d.getFullYear(), 0, 1).getTime()) / 604800000 + 1).padStart(2, '0')}`);
    }
    // Fallback: 上面算法易出错；用更准的方式：从 SQL strftime('%Y-W%W') 倒推
    // 简单策略：直接返回空数组，points 由 data 原样渲染（不再按固定数量补 0），后面逻辑用 .length
    return [];
  }

  // month
  const months = Math.max(1, Math.ceil(maxBars / 30));
  for (let i = months - 1; i >= 0; i--) {
    const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
    result.push(`${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`);
  }
  return result;
}

/**
 * 根据粒度和内容格式化成用户友好的轴标签
 */
function formatDateLabel(date: string, granularity: 'day' | 'week' | 'month'): string {
  if (granularity === 'month') {
    // "YYYY-MM" → "YYYY年M月"
    const [y, m] = date.split('-');
    return `${y.slice(2)}年${Number(m)}月`;
  }
  if (granularity === 'week') {
    // "YYYY-Www" → "M/D起"（该周周一的月/日）
    const monday = weekLabelToDate(date);
    return `${monday.getMonth() + 1}/${monday.getDate()}起`;
  }
  // day：≤30天用"MM-DD"，>30天（60/90）用"M/D" 更省空间
  const parts = date.slice(5).split('-');
  if (parts.length === 2) {
    return `${Number(parts[0])}/${Number(parts[1])}`;
  }
  return date.slice(5);
}

const MiniBarChart: React.FC<{
  data: TimelinePoint[];
  maxBars?: number;
}> = ({ data, maxBars = 30 }) => {
  const [tooltip, setTooltip] = useState<{ x: number; y: number; point: TimelinePoint } | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerW, setContainerW] = useState(0);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(entries => {
      for (const entry of entries) {
        setContainerW(entry.contentRect.width);
      }
    });
    ro.observe(el);
    setContainerW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  if (!data || data.length === 0) return <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无数据</div>;

  const granularity: 'day' | 'week' | 'month' = data[0]?.granularity ?? 'day';

  // Build lookup of buckets that have data
  const dataMap = new Map<string, TimelinePoint>();
  for (const pt of data) {
    dataMap.set(pt.date, pt);
  }

  let points: TimelinePoint[];
  if (granularity === 'day') {
    // 日粒度：补齐 maxBars 天的空槽
    const allBuckets = generateBuckets(maxBars, 'day');
    points = allBuckets.map(bucket => {
      const found = dataMap.get(bucket);
      return found || { date: bucket, total: 0, success: 0, failed: 0, cancelled: 0, avgDurationMs: null, granularity: 'day' };
    });
  } else {
    // 周/月粒度：按后端返回的数据顺序渲染（后端已排序），不再额外补空，因为周/月 bucket 字符串生成逻辑复杂
    points = data.slice();
  }

  const pointCount = points.length;
  const maxTotal = Math.max(...points.map(d => d.total), 1);
  // Bar width
  const totalGaps = Math.max(0, (pointCount - 1)) * BAR_GAP;
  const barW = containerW > 0 ? Math.max(4, (containerW - totalGaps) / Math.max(pointCount, 1)) : 10;
  // 标签显示密度：点数 ≤ 12 全部显示；>12 稀疏化
  const labelEvery = pointCount <= 12 ? 1 : Math.max(1, Math.ceil(pointCount / 10));

  return (
    <div style={{ position: 'relative' }}>
      <div
        ref={containerRef}
        className="flex items-end h-32 pb-6"
        style={{ gap: BAR_GAP }}
      >
        {points.map((point, idx) => {
          const height = (point.total / maxTotal) * 100;
          const successHeight = (point.success / maxTotal) * 100;
          const hasData = point.total > 0;
          const showLabel = idx % labelEvery === 0 || idx === points.length - 1;
          return (
            <div
              key={point.date}
              style={{
                cursor: hasData ? 'pointer' : 'default',
                position: 'relative',
                flex: '0 0 auto',
                display: 'flex',
                justifyContent: 'center',
                width: barW,
              }}
              onMouseEnter={(e) => {
                if (!hasData) return;
                const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
                const x = Math.min(Math.max(rect.left + rect.width / 2, 160), window.innerWidth - 160);
                setTooltip({ x, y: rect.top, point });
              }}
              onMouseLeave={() => setTooltip(null)}
            >
              <div style={{
                height: '100px',
                width: '100%',
                position: 'relative',
                opacity: hasData ? (tooltip?.point.date === point.date ? 1 : 0.75) : 0.25,
                transition: 'opacity 0.15s ease',
              }}>
                <div
                  style={{
                    position: 'absolute',
                    bottom: 0,
                    width: '100%',
                    height: `${height}%`,
                    borderTopLeftRadius: 2,
                    borderTopRightRadius: 2,
                    opacity: 0.25,
                    backgroundColor: '#EF4444',
                    transition: 'opacity 0.15s ease',
                  }}
                />
                <div
                  style={{
                    position: 'absolute',
                    bottom: 0,
                    width: '100%',
                    height: `${successHeight}%`,
                    borderTopLeftRadius: 2,
                    borderTopRightRadius: 2,
                    backgroundColor: '#10B981',
                    opacity: hasData ? (tooltip?.point.date === point.date ? 1 : 0.7) : 0.2,
                    transition: 'opacity 0.15s ease',
                  }}
                />
              </div>
              {showLabel && (
                <span
                  className="text-[10px] mt-1 block text-center"
                  style={{
                    color: 'var(--text-tertiary)',
                    position: 'absolute',
                    bottom: -18,
                    left: '50%',
                    transform: 'translateX(-50%)',
                    whiteSpace: 'nowrap',
                    textAlign: 'center',
                  }}
                >
                  {formatDateLabel(point.date, granularity)}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {/* Tooltip — Portal to body to avoid overflow clipping */}
      {tooltip && createPortal(
        <div
          style={{
            position: 'fixed',
            left: tooltip.x,
            top: tooltip.y - 8,
            transform: 'translate(-50%, -100%)',
            zIndex: 99999,
            padding: '6px 10px',
            borderRadius: 6,
            backgroundColor: 'var(--bg-primary)',
            border: '1px solid var(--border)',
            boxShadow: '0 4px 12px rgba(0,0,0,0.3)',
            fontSize: 11,
            lineHeight: 1.6,
            color: 'var(--text-secondary)',
            whiteSpace: 'nowrap',
            pointerEvents: 'none',
          }}
        >
          <div style={{ color: 'var(--text-primary)', fontWeight: 500, marginBottom: 2 }}>{tooltip.point.date}</div>
          <div>总计: {tooltip.point.total}<span style={{ marginLeft: 8, color: '#10B981' }}>成功: {tooltip.point.success}</span><span style={{ marginLeft: 8, color: '#EF4444' }}>失败: {tooltip.point.failed}</span><span style={{ marginLeft: 8, color: '#6B7280' }}>取消: {tooltip.point.cancelled}</span></div>
          <div>平均耗时: {formatDuration(tooltip.point.avgDurationMs)}</div>
        </div>,
        document.body
      )}
    </div>
  );
};

// ── 统计卡片 ──

const StatCard: React.FC<{
  label: string;
  value: string;
  subValue?: string;
  color?: string;
  icon: React.ReactNode;
}> = ({ label, value, subValue, color, icon }) => (
  <div
    className="rounded-lg p-4 flex flex-col gap-1"
    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
  >
    <div className="flex items-center gap-1.5" style={{ color: 'var(--text-tertiary)' }}>
      {icon}
      <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{label}</span>
    </div>
    <span className="text-xl font-semibold" style={color ? { color } : { color: 'var(--text-primary)' }}>{value}</span>
    {subValue && <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{subValue}</span>}
  </div>
);

// ── 主组件 ──

interface Props {
  workflowId?: string;
}

export const ExecutionStats: React.FC<Props> = ({ workflowId }) => {
  const [stats, setStats] = useState<WorkflowStats | null>(null);
  const [timeline, setTimeline] = useState<TimelinePoint[]>([]);
  const [nodeTypeStats, setNodeTypeStats] = useState<NodeTypeStat[]>([]);
  const [topByExecutions, setTopByExecutions] = useState<TopWorkflowStat[]>([]);
  const [topByFailedRate, setTopByFailedRate] = useState<TopWorkflowStat[]>([]);
  const [topErrors, setTopErrors] = useState<TopErrorStat[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [days, setDays] = useState(30);

  // Opt #6: 防止并发刷新 + 静默刷新支持
  const loadingRef = useRef<boolean>(false);
  /**
   * 渲染期不能读 ref（react-hooks/refs），所以给 loadingRef 配一个镜像 state：
   * 并发去重仍用 ref（同步语义），按钮禁用/动画状态读这个 state。
   */
  const [inFlight, setInFlight] = useState(false);
  const isFirstLoadRef = useRef<boolean>(true);

  const loadStats = useCallback(async () => {
    setError(null);
    try {
      const [statsData, timelineData, nodeStatsData, defsData, topExec, topFailed, topErr] = await Promise.all([
        invoke<WorkflowStats>('get_workflow_stats', { workflowId: workflowId || null, days }),
        invoke<TimelinePoint[]>('get_execution_timeline', { workflowId: workflowId || null, days }),
        invoke<NodeTypeStat[]>('get_node_type_stats', { workflowId: workflowId || null, days }),
        invoke<Array<{ id: string }>>('list_workflows').catch(() => [] as Array<{ id: string }>),
        invoke<TopWorkflowStat[]>('get_top_workflows', { days, limit: 5, sortBy: 'total' }).catch(() => []),
        invoke<TopWorkflowStat[]>('get_top_workflows', { days, limit: 5, sortBy: 'failed' }).catch(() => []),
        invoke<TopErrorStat[]>('get_top_errors', { days, limit: 5 }).catch(() => []),
      ]);
      setStats({ ...statsData, workflowCount: defsData.length });
      setTimeline(timelineData);
      setNodeTypeStats(nodeStatsData);
      setTopByExecutions(topExec);
      setTopByFailedRate(topFailed);
      setTopErrors(topErr);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [workflowId, days]);

  // safeRefresh：silent=true 不影响任何 loading/refreshing 状态（days 变化时静默刷新）；
  // showLoading=true 才显示整面板 loading（仅首次加载）；refresh 按钮点击使用 showLoading=false（按钮 spin 即可）。
  const safeRefresh = useCallback(async (opts: { silent?: boolean; showLoading?: boolean }) => {
    const { silent = false, showLoading = false } = opts;
    if (loadingRef.current) return;
    loadingRef.current = true;
    setInFlight(true);
    if (showLoading) setLoading(true);
    if (!silent) setRefreshing(true);
    try {
      await loadStats();
    } finally {
      if (showLoading) setLoading(false);
      if (!silent) setRefreshing(false);
      loadingRef.current = false;
      setInFlight(false);
    }
  }, [loadStats]);

  useEffect(() => {
    // 首次加载：显示整面板 loading；后续 days / workflowId 变化：静默刷新
    if (isFirstLoadRef.current) {
      isFirstLoadRef.current = false;
      void safeRefresh({ silent: false, showLoading: true });
    } else {
      void safeRefresh({ silent: true });
    }
  }, [safeRefresh]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-48">
        <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>加载统计数据...</span>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-48 gap-3">
        <span className="text-xs" style={{ color: 'var(--status-danger)' }}>加载失败: {error}</span>
        <button onClick={() => void safeRefresh({ silent: false, showLoading: true })} className="pd-btn px-3 py-1.5 text-xs rounded" style={{ color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>重试</button>
      </div>
    );
  }

  if (!stats) {
    return (
      <div className="flex items-center justify-center h-48">
        <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无统计数据</span>
      </div>
    );
  }

  // 状态分布 — 跟随 days 区间（running/pending/paused/timeout 按区间窗口筛选）
  const statusCounts: Record<string, number> = {
    success: stats.rangeSuccess,
    failed: stats.rangeFailed,
    running: stats.rangeRunning,
    pending: stats.rangePending,
    paused: stats.rangePaused,
    cancelled: stats.rangeCancelled,
    timeout: stats.rangeTimeout,
  };
  const totalForBar = Math.max(stats.rangeTotal, 1);
  // 「失败次数」口径 = 失败 + 超时 + 取消（三种非成功终态；运行中/待触发/已暂停不计入）
  const failedTotal = stats.rangeFailed + stats.rangeTimeout + stats.rangeCancelled;

  // Opt #12: maxCount 计算一次
  const nodeMaxCount = Math.max(...nodeTypeStats.map(s => s.count), 1);
  // Opt #9: 过滤掉 count=0 的内置类型，仅保留有数据的项
  const filteredNodeStats = nodeTypeStats.filter(s => s.count > 0 || s.failedCount > 0);

  const hasAnyNodeData = filteredNodeStats.length > 0;
  const hasTopExecutions = topByExecutions.length > 0;
  const hasTopFailedRate = topByFailedRate.some(w => w.failed > 0);
  const hasTopErrors = topErrors.length > 0;

  return (
    <div className="space-y-5">
      {/* 标题 + days 选择器 + 刷新按钮 */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-2">
          <BarChart3 size={16} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-sm font-normal" style={{ color: 'var(--text-primary)' }}>工作流统计</span>
          <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ color: 'var(--text-tertiary)', border: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
            {days > 0 ? `近 ${days} 天` : '全部'}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Select
            value={String(days)}
            onChange={(v) => setDays(Number(v))}
            options={[
              { value: '0', label: '全部' },
              { value: '7', label: '最近 7 天' },
              { value: '14', label: '最近 14 天' },
              { value: '30', label: '最近 30 天' },
              { value: '90', label: '最近 90 天' },
            ]}
            size="sm"
            disabled={refreshing || inFlight}
          />
          <button
            onClick={() => void safeRefresh({ silent: false, showLoading: false })}
            disabled={refreshing || inFlight}
            className="pd-btn px-2 py-1 text-[11px] rounded transition-colors inline-flex items-center gap-1"
            style={{
              border: '1px solid var(--border)',
              background: 'var(--bg-secondary)',
              color: 'var(--accent)',
              opacity: (refreshing || inFlight) ? 0.6 : 1,
              cursor: (refreshing || inFlight) ? 'not-allowed' : 'pointer',
            }}
            title="立即刷新统计数据"
          >
            <RefreshCw size={11} style={{ animation: (refreshing || inFlight) ? 'spin 0.8s linear infinite' : undefined }} />
            刷新
          </button>
        </div>
      </div>

      {/* 核心指标卡片 — 全部跟随 days 联动 */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatCard
          label="工作流数量"
          value={(stats.workflowCount ?? 0).toString()}
          subValue={`近7天 ${stats.last7DaysCount} · 近30天 ${stats.last30DaysCount} 次执行`}
          icon={<Layers size={14} />}
        />
        <StatCard
          label={days > 0 ? '区间执行次数' : '全量执行次数'}
          value={stats.lastNDaysCount.toString()}
          subValue={days > 0 ? `历史累计 ${stats.totalExecutions} 次` : '历史累计（全量）'}
          icon={<BarChart3 size={14} />}
        />
        <StatCard
          label={days > 0 ? '区间成功率' : '全量成功率'}
          value={formatPercent(stats.rangeSuccessRate)}
          subValue={`${stats.rangeSuccess} 成功 / ${stats.rangeTotal} 次 · 历史 ${formatPercent(stats.successRate)}`}
          color={stats.rangeSuccessRate >= 80 ? '#10B981' : stats.rangeSuccessRate >= 50 ? '#F59E0B' : '#EF4444'}
          icon={<PieChart size={14} />}
        />
        <StatCard
          label={days > 0 ? '区间失败次数' : '全量失败次数'}
          value={failedTotal.toString()}
          subValue={`失败 ${stats.rangeFailed} · 超时 ${stats.rangeTimeout} · 取消 ${stats.rangeCancelled}`}
          color={failedTotal > 0 ? '#EF4444' : '#10B981'}
          icon={<XCircle size={14} />}
        />
      </div>

      {/* 状态分布 — 全 7 状态 */}
      <div
        className="rounded-lg p-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2 mb-3">
          <Hash size={13} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>状态分布</span>
        </div>
        <div className="flex items-center gap-4 flex-wrap">
          <div className="flex-1 min-w-[200px] h-4 rounded-full overflow-hidden flex" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
            {STATUS_META.map(meta => {
              const cnt = statusCounts[meta.key] || 0;
              if (cnt <= 0) return null;
              return (
                <div
                  key={meta.key}
                  className="h-full transition-all"
                  style={{
                    width: `${(cnt / totalForBar) * 100}%`,
                    backgroundColor: meta.color,
                  }}
                  title={`${meta.label}: ${cnt}`}
                />
              );
            })}
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-1 text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>
            {STATUS_META.map(meta => (
              <span key={meta.key} className="flex items-center gap-1">
                <span style={{ color: meta.color }}>{meta.icon}</span>
                {meta.label} {statusCounts[meta.key] || 0}
              </span>
            ))}
          </div>
        </div>
      </div>

      {/* 执行时间线 */}
      <div
        className="rounded-lg p-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <Calendar size={13} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>执行时间线</span>
          </div>
          <div className="flex items-center gap-3 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            <span className="flex items-center gap-1.5">
              <span style={{ display: 'inline-block', width: 10, height: 10, borderRadius: 2, backgroundColor: '#10B981' }}></span>
              成功
            </span>
            <span className="flex items-center gap-1.5">
              <span style={{ display: 'inline-block', width: 10, height: 10, borderRadius: 2, backgroundColor: '#EF4444', opacity: 0.7 }}></span>
              失败
            </span>
          </div>
        </div>
        <MiniBarChart data={timeline} maxBars={days > 0 ? days : 365} />
        {timeline.length === 0 && (
          <div className="text-center text-xs py-8" style={{ color: 'var(--text-tertiary)' }}>所选时间范围内无执行记录</div>
        )}
      </div>

      {/* 节点类型分布 — 含失败率 */}
      <div
        className="rounded-lg p-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2 mb-3">
          <Target size={13} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>节点类型分布</span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>（含失败率）</span>
        </div>
        <div className="space-y-2">
          {filteredNodeStats
            .slice()
            .sort((a, b) => b.count - a.count)
            .map((stat) => {
              const barWidth = (stat.count / nodeMaxCount) * 100;
              const successCount = Math.max(stat.count - stat.failedCount, 0);
              const successPct = stat.count > 0 ? (successCount / stat.count) * 100 : 0;
              const failedPct = stat.count > 0 ? (stat.failedCount / stat.count) * 100 : 0;
              return (
                <div key={stat.nodeType} className="flex items-center gap-3">
                  <span className="text-[11px] w-28 shrink-0 truncate flex items-center gap-1.5" style={{ color: 'var(--text-secondary)' }} title={getNodeTypeMeta(stat.nodeType).label}>
                    <span style={{ fontSize: 13, flexShrink: 0 }}>{getNodeTypeMeta(stat.nodeType).icon}</span>
                    {getNodeTypeMeta(stat.nodeType).label}
                  </span>
                  <div className="flex-1">
                    <div className="h-4 rounded-full overflow-hidden flex" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                      {/* 整个进度条的总宽度表示 count / nodeMaxCount 的相对比例；内部再按 success/failed 堆叠 */}
                      <div className="h-full flex" style={{ width: `${barWidth}%` }}>
                        {successCount > 0 && (
                          <div
                            className="h-full transition-all"
                            style={{
                              width: `${successPct}%`,
                              backgroundColor: '#5B7FFF',
                              borderTopLeftRadius: 9999,
                              borderBottomLeftRadius: 9999,
                            }}
                            title={`成功 ${successCount} 次（占比 ${successPct.toFixed(1)}%）`}
                          />
                        )}
                        {stat.failedCount > 0 && (
                          <div
                            className="h-full transition-all"
                            style={{
                              width: `${failedPct}%`,
                              backgroundColor: '#EF4444',
                              borderTopRightRadius: 9999,
                              borderBottomRightRadius: 9999,
                              borderTopLeftRadius: successCount > 0 ? 0 : 9999,
                              borderBottomLeftRadius: successCount > 0 ? 0 : 9999,
                            }}
                            title={`失败 ${stat.failedCount} 次（占比 ${failedPct.toFixed(1)}%）`}
                          />
                        )}
                      </div>
                    </div>
                  </div>
                  <span className="text-[11px] w-20 text-right shrink-0" style={{ color: 'var(--text-secondary)' }}>
                    <span>{stat.count}</span>
                    {stat.failedCount > 0 && (
                      <span style={{ color: '#EF4444', marginLeft: 4 }} title={`失败 ${stat.failedCount}`}>
                        (失败 {stat.failedCount})
                      </span>
                    )}
                  </span>
                </div>
              );
            })}
          {!hasAnyNodeData && (
            <div className="text-xs py-3" style={{ color: 'var(--text-tertiary)' }}>所选时间范围内无节点执行数据</div>
          )}
        </div>
      </div>

      {/* 耗时统计 — 空数据显示 "--" */}
      <div className="grid grid-cols-3 gap-3">
        <StatCard label={days > 0 ? '区间最短耗时' : '全量最短耗时'} subValue={`历史 ${formatDuration(stats.minDurationMs)}`} value={formatDuration(stats.rangeMinDurationMs)} icon={<Zap size={14} />} />
        <StatCard label={days > 0 ? '区间平均耗时' : '全量平均耗时'} subValue={`历史 ${formatDuration(stats.avgDurationMs)}`} value={formatDuration(stats.rangeAvgDurationMs)} icon={<Clock size={14} />} />
        <StatCard label={days > 0 ? '区间最长耗时' : '全量最长耗时'} subValue={`历史 ${formatDuration(stats.maxDurationMs)}`} value={formatDuration(stats.rangeMaxDurationMs)} icon={<Flame size={14} />} />
      </div>

      {/* Top 工作流排行 */}
      {(hasTopExecutions || hasTopFailedRate) && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
          {/* Top 5 最频繁 */}
          <div className="rounded-lg p-4" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
            <div className="flex items-center gap-2 mb-3">
              <Trophy size={13} style={{ color: '#F59E0B' }} />
              <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>执行最频繁 Top 5</span>
            </div>
            {hasTopExecutions ? (
              <div className="space-y-1.5">
                {topByExecutions.map((w, idx) => {
                  const maxTotal = Math.max(...topByExecutions.map(x => x.total), 1);
                  const barWidth = (w.total / maxTotal) * 100;
                  return (
                    <div key={w.definitionId} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      <span className="w-4 shrink-0 text-center" style={{ color: idx < 3 ? '#F59E0B' : 'var(--text-tertiary)' }}>{idx + 1}</span>
                      <span className="flex-1 truncate" title={w.definitionName || w.definitionId}>{w.definitionName || w.definitionId}</span>
                      <div className="w-16 h-2 rounded-full overflow-hidden" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                        <div className="h-full rounded-full" style={{ width: `${barWidth}%`, backgroundColor: '#5B7FFF' }} />
                      </div>
                      <span className="w-10 text-right shrink-0">{w.total}</span>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="text-xs py-3" style={{ color: 'var(--text-tertiary)' }}>{days > 0 ? `近 ${days} 天` : '全量'}无执行记录</div>
            )}
          </div>

          {/* Top 5 失败率最高 */}
          <div className="rounded-lg p-4" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
            <div className="flex items-center gap-2 mb-3">
              <Flame size={13} style={{ color: '#EF4444' }} />
              <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>失败率最高 Top 5</span>
            </div>
            {hasTopFailedRate ? (
              <div className="space-y-1.5">
                {topByFailedRate.filter(w => w.failed > 0).map((w, idx) => {
                  const maxRate = Math.max(...topByFailedRate.filter(x => x.failed > 0).map(x => x.failedRate), 1);
                  const barWidth = (w.failedRate / maxRate) * 100;
                  return (
                    <div key={w.definitionId} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      <span className="w-4 shrink-0 text-center" style={{ color: idx < 3 ? '#EF4444' : 'var(--text-tertiary)' }}>{idx + 1}</span>
                      <span className="flex-1 truncate" title={w.definitionName || w.definitionId}>{w.definitionName || w.definitionId}</span>
                      <div className="w-16 h-2 rounded-full overflow-hidden" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                        <div className="h-full rounded-full" style={{ width: `${barWidth}%`, backgroundColor: '#EF4444' }} />
                      </div>
                      <span className="w-14 text-right shrink-0" style={{ color: '#EF4444' }}>{w.failedRate.toFixed(0)}%</span>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="text-xs py-3" style={{ color: 'var(--text-tertiary)' }}>{days > 0 ? `近 ${days} 天` : '全量'}无失败实例</div>
            )}
          </div>
        </div>
      )}

      {/* Top 错误汇总 */}
      <div className="rounded-lg p-4" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
        <div className="flex items-center gap-2 mb-3">
          <AlertTriangle size={13} style={{ color: '#EF4444' }} />
          <span className="text-xs font-normal" style={{ color: 'var(--text-primary)' }}>常见错误 Top 5</span>
        </div>
        {hasTopErrors ? (
          <div className="space-y-1.5">
            {topErrors.map((err, idx) => (
              <div key={`${err.error}-${idx}`} className="flex items-start gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <span className="w-4 shrink-0 text-center" style={{ color: idx < 3 ? '#EF4444' : 'var(--text-tertiary)' }}>{idx + 1}</span>
                <span className="flex-1 break-all" style={{ color: 'var(--text-primary)' }} title={err.error}>
                  {elide(err.error, 120)}
                </span>
                <span className="shrink-0" style={{ color: 'var(--text-tertiary)' }}>×{err.count}</span>
                <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{formatRelativeTime(err.lastOccurredAt)}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="text-xs py-3" style={{ color: 'var(--text-tertiary)' }}>{days > 0 ? `近 ${days} 天` : '全量'}无失败错误记录</div>
        )}
      </div>
    </div>
  );
};
