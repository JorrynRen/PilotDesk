/**
 * WorkflowMonitor — 工作流执行监控面板
 *
 * 实时显示工作流实例状态、完成率进度、节点执行日志。
 */

import React, { useEffect, useState } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { invoke } from '@tauri-apps/api/core';

interface Props {
  onViewDefinition: (definitionId: string) => void;
}

interface ExecutionLog {
  timestamp: number;
  level: string;
  message: string;
  metadata?: string;
}

const STATUS_LABELS: Record<string, { label: string; color: string }> = {
  'pending': { label: '待触发', color: '#6B7280' },
  'running': { label: '运行中', color: '#3B82F6' },
  'paused': { label: '已暂停', color: '#F59E0B' },
  'success': { label: '成功', color: '#10B981' },
  'failed': { label: '失败', color: '#EF4444' },
  'cancelled': { label: '已取消', color: '#6B7280' },
  'timeout': { label: '超时', color: '#EF4444' },
};

function useExecutionLogs(executionId: string | null) {
  const [logs, setLogs] = useState<ExecutionLog[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!executionId) { setLogs([]); return; }
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      try {
        const result = await invoke<ExecutionLog[]>('get_node_execution_logs', { executionId });
        if (!cancelled) setLogs(result);
      } catch {
        if (!cancelled) setLogs([]);
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    load();
    const interval = setInterval(load, 5000);
    return () => { cancelled = true; clearInterval(interval); };
  }, [executionId]);

  return { logs, loading };
}

export const WorkflowMonitor: React.FC<Props> = ({ onViewDefinition }) => {
  const { instances, loadInstances, cancelWorkflow, startWorkflow } = useWorkflowStore();
  const [expandedInstance, setExpandedInstance] = useState<string | null>(null);
  const { logs, loading: logsLoading } = useExecutionLogs(expandedInstance);

  useEffect(() => {
    loadInstances();
    const interval = setInterval(loadInstances, 5000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="workflow-monitor">
      {instances.length === 0 ? (
        <div className="monitor-empty">
          <p>暂无工作流执行记录</p>
        </div>
      ) : (
        <div className="monitor-list">
          {instances.map((instance) => {
            const statusInfo = STATUS_LABELS[instance.status] || { label: instance.status, color: '#6B7280' };
            const progress = instance.completionRate ?? 0;

            return (
              <div key={instance.id} className="monitor-card">
                <div className="monitor-card-header">
                  <div className="monitor-title">
                    <h3>{instance.definitionName}</h3>
                    <span className="monitor-trigger">
                      {instance.trigger === 'manual' ? '手动' : instance.trigger === 'cron' ? '定时' : '事件'}
                    </span>
                  </div>
                  <span className="monitor-status" style={{ color: statusInfo.color }}>
                    {statusInfo.label}
                  </span>
                </div>

                {/* 完成率进度条 */}
                {instance.status === 'running' && (
                  <div className="monitor-progress">
                    <div className="progress-info">
                      <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                        完成率
                      </span>
                      <span className="text-xs font-medium" style={{ color: 'var(--accent)' }}>
                        {Math.round(progress * 100)}%
                      </span>
                    </div>
                    <div
                      className="progress-bar"
                      style={{
                        height: 4,
                        borderRadius: 2,
                        backgroundColor: 'var(--bg-tertiary)',
                        marginTop: 4,
                      }}
                    >
                      <div
                        style={{
                          height: '100%',
                          borderRadius: 2,
                          width: `${Math.round(progress * 100)}%`,
                          backgroundColor: 'var(--accent)',
                          transition: 'width 0.3s ease',
                        }}
                      />
                    </div>
                  </div>
                )}

                <div className="monitor-meta">
                  <span>创建: {new Date(Number(instance.createdAt) * 1000).toLocaleString()}</span>
                  {instance.startedAt && <span>开始: {new Date(Number(instance.startedAt) * 1000).toLocaleString()}</span>}
                  {instance.completedAt && <span>完成: {new Date(Number(instance.completedAt) * 1000).toLocaleString()}</span>}
                </div>

                {instance.error && (
                  <div className="monitor-error">
                    <strong>错误:</strong> {instance.error}
                  </div>
                )}

                {/* 操作按钮 */}
                <div className="monitor-actions">
                  {instance.status === 'running' && (
                    <>
                      <button onClick={() => cancelWorkflow(instance.id)}>暂停</button>
                      <button onClick={() => cancelWorkflow(instance.id)} className="btn-danger">停止</button>
                    </>
                  )}
                  {instance.status === 'paused' && (
                    <button onClick={() => startWorkflow(instance.definitionId)} className="btn-primary">恢复</button>
                  )}
                  {(instance.status === 'failed' || instance.status === 'timeout') && (
                    <button onClick={() => startWorkflow(instance.definitionId)} className="btn-primary">重试</button>
                  )}
                  {instance.status === 'cancelled' && instance.error && (
                    <button onClick={() => startWorkflow(instance.definitionId)} className="btn-primary">重试</button>
                  )}
                  <button onClick={() => onViewDefinition(instance.definitionId)}>查看定义</button>
                  <button
                    onClick={() => setExpandedInstance(expandedInstance === instance.id ? null : instance.id)}
                    className="text-xs"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    {expandedInstance === instance.id ? '收起日志' : '查看日志'}
                  </button>
                </div>

                {/* 节点执行日志 */}
                {expandedInstance === instance.id && (
                  <div className="monitor-logs" style={{ marginTop: 8 }}>
                    {logsLoading && (
                      <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>加载中...</span>
                    )}
                    {!logsLoading && logs.length === 0 && (
                      <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无执行日志</span>
                    )}
                    {logs.map((log, idx) => (
                      <div
                        key={idx}
                        className="log-entry"
                        style={{
                          fontSize: 11,
                          padding: '3px 0',
                          borderBottom: '1px solid var(--border)',
                          color: log.level === 'warn' ? '#F59E0B' : 'var(--text-secondary)',
                        }}
                      >
                        <span style={{ color: 'var(--text-tertiary)', marginRight: 8 }}>
                          {new Date(log.timestamp * 1000).toLocaleTimeString()}
                        </span>
                        {log.message}
                        {log.metadata && (
                          <span style={{ color: 'var(--text-tertiary)', marginLeft: 8 }}>
                            {log.metadata}
                          </span>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
