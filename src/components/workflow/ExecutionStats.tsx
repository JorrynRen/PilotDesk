/**
 * ExecutionStats — 工作流执行统计面板
 *
 * 展示工作流执行统计数据，包括总执行次数、成功率、执行时长分布、
 * 每日执行时间线和节点类型使用统计。
 */

import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

// ── 类型定义 ──

interface WorkflowStats {
  totalExecutions: number;
  successCount: number;
  failedCount: number;
  cancelledCount: number;
  successRate: number;
  avgDurationMs: number;
  maxDurationMs: number;
  minDurationMs: number;
  totalNodeExecutions: number;
  nodeFailedCount: number;
  last7DaysCount: number;
  last30DaysCount: number;
}

interface TimelinePoint {
  date: string;
  total: number;
  success: number;
  failed: number;
  avgDurationMs: number;
}

interface NodeTypeStat {
  nodeType: string;
  count: number;
  failedCount: number;
  avgDurationMs: number;
}

// ── 节点类型显示名映射 ──

const NODE_TYPE_LABELS: Record<string, string> = {
  'agent': 'Agent 任务',
  'api': 'API 调用',
  'transform': '代码转换',
  'plugin': '插件命令',
  'subflow': '子工作流',
  'interact': '人工介入',
};

function getNodeTypeLabel(type: string): string {
  return NODE_TYPE_LABELS[type] || type;
}

// ── 格式化工具 ──

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms.toFixed(0)}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60000);
  const seconds = Math.floor((ms % 60000) / 1000);
  return `${minutes}m ${seconds}s`;
}

function formatPercent(value: number): string {
  return `${value.toFixed(1)}%`;
}

// ── 迷你柱状图组件 ──

const MiniBarChart: React.FC<{
  data: TimelinePoint[];
  maxBars?: number;
}> = ({ data, maxBars = 30 }) => {
  if (data.length === 0) return <div className="text-sm text-[var(--text-tertiary)]">暂无数据</div>;

  const sliced = data.slice(-maxBars);
  const maxTotal = Math.max(...sliced.map(d => d.total), 1);

  return (
    <div className="flex items-end gap-[2px] h-32 overflow-x-auto pb-1">
      {sliced.map((point) => {
        const height = (point.total / maxTotal) * 100;
        const successHeight = (point.success / maxTotal) * 100;
        return (
          <div
            key={point.date}
            className="relative flex flex-col items-center group"
            style={{ minWidth: '20px' }}
          >
            <div className="relative w-full" style={{ height: '100px', width: '16px' }}>
              <div
                className="absolute bottom-0 w-full rounded-t-sm opacity-30"
                style={{
                  height: `${height}%`,
                  backgroundColor: '#EF4444',
                }}
              />
              <div
                className="absolute bottom-0 w-full rounded-t-sm"
                style={{
                  height: `${successHeight}%`,
                  backgroundColor: '#10B981',
                }}
              />
            </div>
            <div className="absolute bottom-full mb-1 hidden group-hover:block z-10">
              <div className="bg-[var(--bg-tertiary)] text-xs px-2 py-1 rounded shadow-lg whitespace-nowrap">
                <div>{point.date}</div>
                <div>总计: {point.total} | 成功: {point.success} | 失败: {point.failed}</div>
                <div>平均耗时: {formatDuration(point.avgDurationMs)}</div>
              </div>
            </div>
            {sliced.length <= 15 && (
              <span className="text-[10px] text-[var(--text-tertiary)] mt-1 truncate" style={{ maxWidth: '28px' }}>
                {point.date.slice(5)}
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
};

// ── 统计卡片组件 ──

const StatCard: React.FC<{
  label: string;
  value: string;
  subValue?: string;
  color?: string;
  icon?: string;
}> = ({ label, value, subValue, color, icon }) => (
  <div className="bg-[var(--bg-secondary)] border border-[var(--border-default)] rounded-lg p-4 flex flex-col gap-1">
    <div className="flex items-center gap-2">
      {icon && <span className="text-lg">{icon}</span>}
      <span className="text-xs text-[var(--text-tertiary)] uppercase tracking-wider">{label}</span>
    </div>
    <span className="text-2xl font-semibold" style={color ? { color } : undefined}>{value}</span>
    {subValue && <span className="text-xs text-[var(--text-tertiary)]">{subValue}</span>}
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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [days, setDays] = useState(30);

  const loadStats = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [statsData, timelineData, nodeStatsData] = await Promise.all([
        invoke<WorkflowStats>('get_workflow_stats', { workflowId: workflowId || null }),
        invoke<TimelinePoint[]>('get_execution_timeline', { workflowId: workflowId || null, days }),
        invoke<NodeTypeStat[]>('get_node_type_stats', { workflowId: workflowId || null }),
      ]);
      setStats(statsData);
      setTimeline(timelineData);
      setNodeTypeStats(nodeStatsData);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [workflowId, days]);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-sm text-[var(--text-tertiary)]">加载统计数据...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-48 gap-3">
        <div className="text-sm text-red-500">加载失败: {error}</div>
        <button onClick={loadStats} className="btn-primary text-sm px-3 py-1 rounded">重试</button>
      </div>
    );
  }

  if (!stats) {
    return (
      <div className="flex items-center justify-center h-48">
        <div className="text-sm text-[var(--text-tertiary)]">暂无统计数据</div>
      </div>
    );
  }

  const totalCompleted = stats.successCount + stats.failedCount;

  return (
    <div className="execution-stats space-y-6">
      <div className="flex items-center justify-between">
        <h3 className="text-base font-semibold">执行统计</h3>
        <button onClick={loadStats} className="text-xs text-[var(--text-tertiary)] hover:text-[var(--text-primary)]">
          刷新
        </button>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatCard label="总执行次数" value={stats.totalExecutions.toString()} icon="📊" />
        <StatCard
          label="成功率"
          value={formatPercent(stats.successRate)}
          subValue={`${stats.successCount} / ${totalCompleted} 次完成`}
          color={stats.successRate >= 80 ? '#10B981' : stats.successRate >= 50 ? '#F59E0B' : '#EF4444'}
          icon="🎯"
        />
        <StatCard
          label="平均耗时"
          value={formatDuration(stats.avgDurationMs)}
          subValue={`最长 ${formatDuration(stats.maxDurationMs)}`}
          icon="⏱️"
        />
        <StatCard
          label="近期执行"
          value={stats.last7DaysCount.toString()}
          subValue={`近30天: ${stats.last30DaysCount} 次`}
          icon="📅"
        />
      </div>

      <div className="bg-[var(--bg-secondary)] border border-[var(--border-default)] rounded-lg p-4">
        <h4 className="text-sm font-medium mb-3">状态分布</h4>
        <div className="flex items-center gap-4">
          <div className="flex-1 h-4 rounded-full overflow-hidden bg-[var(--bg-tertiary)] flex">
            {stats.successCount > 0 && (
              <div
                className="h-full bg-green-500 transition-all"
                style={{ width: `${(stats.successCount / Math.max(stats.totalExecutions, 1)) * 100}%` }}
                title={`成功: ${stats.successCount}`}
              />
            )}
            {stats.failedCount > 0 && (
              <div
                className="h-full bg-red-500 transition-all"
                style={{ width: `${(stats.failedCount / Math.max(stats.totalExecutions, 1)) * 100}%` }}
                title={`失败: ${stats.failedCount}`}
              />
            )}
            {stats.cancelledCount > 0 && (
              <div
                className="h-full bg-gray-400 transition-all"
                style={{ width: `${(stats.cancelledCount / Math.max(stats.totalExecutions, 1)) * 100}%` }}
                title={`取消: ${stats.cancelledCount}`}
              />
            )}
          </div>
          <div className="flex gap-3 text-xs shrink-0">
            <span className="flex items-center gap-1">
              <span className="w-2 h-2 rounded-full bg-green-500" />
              成功 {stats.successCount}
            </span>
            <span className="flex items-center gap-1">
              <span className="w-2 h-2 rounded-full bg-red-500" />
              失败 {stats.failedCount}
            </span>
            <span className="flex items-center gap-1">
              <span className="w-2 h-2 rounded-full bg-gray-400" />
              取消 {stats.cancelledCount}
            </span>
          </div>
        </div>
      </div>

      <div className="bg-[var(--bg-secondary)] border border-[var(--border-default)] rounded-lg p-4">
        <div className="flex items-center justify-between mb-3">
          <h4 className="text-sm font-medium">执行时间线</h4>
          <select
            value={days}
            onChange={(e) => setDays(Number(e.target.value))}
            className="text-xs bg-[var(--bg-tertiary)] border border-[var(--border-default)] rounded px-2 py-1"
          >
            <option value={7}>最近 7 天</option>
            <option value={14}>最近 14 天</option>
            <option value={30}>最近 30 天</option>
            <option value={90}>最近 90 天</option>
          </select>
        </div>
        <MiniBarChart data={timeline} maxBars={days} />
        {timeline.length === 0 && (
          <div className="text-center text-sm text-[var(--text-tertiary)] py-8">所选时间范围内无执行记录</div>
        )}
      </div>

      {nodeTypeStats.length > 0 && (
        <div className="bg-[var(--bg-secondary)] border border-[var(--border-default)] rounded-lg p-4">
          <h4 className="text-sm font-medium mb-3">节点类型分布</h4>
          <div className="space-y-2">
            {nodeTypeStats
              .sort((a, b) => b.count - a.count)
              .map((stat) => {
                const maxCount = Math.max(...nodeTypeStats.map(s => s.count), 1);
                const barWidth = (stat.count / maxCount) * 100;
                return (
                  <div key={stat.nodeType} className="flex items-center gap-3">
                    <span className="text-xs w-24 shrink-0 text-[var(--text-secondary)] truncate" title={stat.nodeType}>
                      {getNodeTypeLabel(stat.nodeType)}
                    </span>
                    <div className="flex-1 h-5 bg-[var(--bg-tertiary)] rounded-full overflow-hidden">
                      <div
                        className="h-full rounded-full transition-all"
                        style={{
                          width: `${barWidth}%`,
                          background: 'linear-gradient(90deg, #5B7FFF, #8B5CF6)',
                        }}
                      />
                    </div>
                    <span className="text-xs text-[var(--text-secondary)] w-8 text-right">{stat.count}</span>
                  </div>
                );
              })}
          </div>
        </div>
      )}

      <div className="grid grid-cols-3 gap-3">
        <StatCard label="最短耗时" value={formatDuration(stats.minDurationMs)} icon="⚡" />
        <StatCard label="平均耗时" value={formatDuration(stats.avgDurationMs)} icon="📊" />
        <StatCard label="最长耗时" value={formatDuration(stats.maxDurationMs)} icon="🐢" />
      </div>
    </div>
  );
};
