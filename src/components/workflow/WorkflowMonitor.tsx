/**
 * WorkflowMonitor — 工作流执行实例面板
 *
 * 紧凑单行显示实例信息，支持分页和节点执行详情展开。
 */

import React, { useEffect, useMemo, useState, useRef } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { Activity, Clock, CheckCircle, XCircle, AlertCircle, Ban, AlertTriangle, Square, Eye, ChevronDown, ChevronRight, ChevronLeft, GitBranch, Target, Play, Flag, Timer, Trash2, Search, X, Filter, RefreshCw } from 'lucide-react';

interface Props {
  onViewDefinition: (definitionId: string) => void;
  onDeleteExecution: (executionId: string, definitionName: string) => void;
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

const formatTime = (ts: number | undefined | null) => {
  if (!ts) return '';
  return new Date(Number(ts) * 1000).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
};

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

export const WorkflowMonitor: React.FC<Props> = ({ onViewDefinition, onDeleteExecution }) => {
  const { instances, loading, loadInstances, cancelWorkflow } = useWorkflowStore();
  const [expandedInstance, setExpandedInstance] = useState<string | null>(null);
  const [tooltipInst, setTooltipInst] = useState<string | null>(null);
  const [page, setPage] = useState(0);

  // ---- 刷新 / 防抖状态 ----
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState<boolean>(false);
  const loadingRef = useRef<boolean>(false); // 防止手动/定时器并发调用

  // ---- 筛选状态 ----
  const [filterDefinitionId, setFilterDefinitionId] = useState<string>(ALL_DEFINITION_ID);
  const [filterStatus, setFilterStatus] = useState<string>('all');
  const [filterTrigger, setFilterTrigger] = useState<string>('all');
  const [filterKeyword, setFilterKeyword] = useState<string>('');

  // 安全刷新（并发去重 + 成功后写 lastUpdatedAt）
  const safeRefresh = async (silent: boolean) => {
    if (loadingRef.current) return;
    loadingRef.current = true;
    if (!silent) setRefreshing(true);
    try {
      await loadInstances(undefined, silent);
      setLastUpdatedAt(Date.now());
    } finally {
      if (!silent) setRefreshing(false);
      loadingRef.current = false;
    }
  };

  // 首次加载：显式 loading，用户知道在初始化
  // 之后每 30s 轮询：仅当存在 running / paused / pending 的活实例才执行静默刷新
  useEffect(() => {
    void safeRefresh(false);

    const interval = setInterval(() => {
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

  // 过滤条件变化时重置到第 0 页（用户主动改筛选，保持"从头看结果"的直觉）
  useEffect(() => {
    setPage(0);
  }, [filterDefinitionId, filterStatus, filterTrigger, filterKeyword]);

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

  // 格式化"上次更新时间戳"
  const formatLastUpdate = (ts: number) => {
    const d = new Date(ts);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  };

  return (
    <div>
      {loading && instances.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-32 gap-2" style={{ backgroundColor: 'var(--bg-secondary)', borderRadius: 8, border: '1px solid var(--border)' }}>
          <Activity size={20} style={{ color: 'var(--text-tertiary)', opacity: 0.5 }} />
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>加载执行实例中...</span>
        </div>
      ) : instances.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-32 gap-2" style={{ backgroundColor: 'var(--bg-secondary)', borderRadius: 8, border: '1px solid var(--border)' }}>
          <Activity size={20} style={{ color: 'var(--text-tertiary)', opacity: 0.5 }} />
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无执行实例</span>
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
              <select
                value={filterDefinitionId}
                onChange={e => setFilterDefinitionId(e.target.value)}
                className="pd-btn text-[11px] rounded py-1 px-2"
                style={{
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-primary)',
                  color: 'var(--text-primary)',
                  outline: 'none',
                  minWidth: 130,
                  maxWidth: 220,
                }}
                title="按工作流筛选"
              >
                <option value={ALL_DEFINITION_ID}>全部工作流 ({definitionList.length})</option>
                {definitionList.map(d => (
                  <option key={d.id} value={d.id}>{d.name}</option>
                ))}
              </select>

              {/* 触发方式下拉 */}
              <select
                value={filterTrigger}
                onChange={e => setFilterTrigger(e.target.value)}
                className="pd-btn text-[11px] rounded py-1 px-2"
                style={{
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-primary)',
                  color: 'var(--text-primary)',
                  outline: 'none',
                  minWidth: 88,
                }}
                title="按触发方式筛选"
              >
                {TRIGGER_OPTIONS.map(t => (
                  <option key={t.key} value={t.key}>{t.label}</option>
                ))}
              </select>

              {/* 状态药丸标签（同行） */}
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

              {/* 关键字搜索 */}
              <div
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4,
                  border: '1px solid var(--border)',
                  backgroundColor: 'var(--bg-primary)',
                  borderRadius: 4,
                  padding: '2px 6px',
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
                className="pd-btn px-2 py-1 text-[11px] rounded transition-colors whitespace-nowrap"
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
                  onClick={() => void safeRefresh(false)}
                  disabled={refreshing || loadingRef.current}
                  className="pd-btn px-2 py-1 text-[11px] rounded transition-colors inline-flex items-center gap-1"
                  style={{
                    border: '1px solid var(--border)',
                    background: 'var(--bg-secondary)',
                    color: 'var(--accent)',
                    opacity: (refreshing || loadingRef.current) ? 0.6 : 1,
                  }}
                  title="立即刷新实例列表"
                >
                  <RefreshCw size={11} style={{ animation: (refreshing || loadingRef.current) ? 'spin 0.8s linear infinite' : undefined }} />
                  刷新
                </button>
              </div>
            </div>
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

              return (
                <div key={instance.id}>
                  {/* ── Card ── */}
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 4,
                      padding: '12px',
                      borderRadius: 6,
                      backgroundColor: isExpanded ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      border: '1px solid var(--border)',
                      fontSize: 12,
                      position: 'relative',
                    }}
                  >
                    {/* ── Row 1: Name + Actions ── */}
                    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                      <Activity size={14} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                      <span
                        className="truncate font-medium"
                        style={{ color: 'var(--text-primary)', flex: '0 1 280px', minWidth: 0 }}
                        title={instance.definitionName}
                      >
                        {instance.definitionName}
                      </span>

                      {/* Spacer */}
                      <div style={{ flex: 1 }} />

                      {/* Actions */}
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
                            onDeleteExecution(instance.id, instance.definitionName);
                          }}
                          className="text-[10px] px-2 py-0.5 rounded transition-colors"
                          style={{ border: '1px solid var(--border)', background: 'transparent', color: 'var(--text-tertiary)' }}
                          title="删除执行记录"
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

                  {/* ── Expanded: 输出上下文 ── */}
                  {isExpanded && (
                    <div
                      style={{
                        marginTop: 2,
                        padding: '8px 12px',
                        borderRadius: 6,
                        backgroundColor: 'var(--bg-tertiary)',
                        border: '1px solid var(--border)',
                        fontSize: 11,
                        maxHeight: 200,
                        overflowY: 'auto',
                      }}
                    >
                      <div style={{ fontSize: 10, color: 'var(--text-tertiary)', marginBottom: 4, fontWeight: 500 }}>输出上下文</div>
                      {instance.context && Object.keys(instance.context).length > 0 ? (
                        <pre style={{ margin: 0, fontSize: 11, color: 'var(--text-primary)', whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontFamily: 'var(--font-mono, "Cascadia Code", "Fira Code", monospace)', lineHeight: 1.5 }}>
                          {JSON.stringify(instance.context, null, 2)}
                        </pre>
                      ) : (
                        <span style={{ color: 'var(--text-tertiary)' }}>暂无上下文数据</span>
                      )}
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
