/**
 * WorkflowMonitor — 工作流执行监控面板
 *
 * 实时显示工作流实例状态、步骤执行进度和错误信息。
 */

import React, { useEffect } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';

interface Props {
  onViewDefinition: (definitionId: string) => void;
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

const STEP_STATUS_LABELS: Record<string, { label: string; color: string }> = {
  'pending': { label: '待执行', color: '#6B7280' },
  'running': { label: '执行中', color: '#3B82F6' },
  'success': { label: '成功', color: '#10B981' },
  'failed': { label: '失败', color: '#EF4444' },
  'skipped': { label: '已跳过', color: '#9CA3AF' },
  'retrying': { label: '重试中', color: '#F59E0B' },
};

export const WorkflowMonitor: React.FC<Props> = ({ onViewDefinition }) => {
  const { instances, loadInstances, cancelWorkflow, startWorkflow } = useWorkflowStore();

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
            const stepEntries = Object.entries(instance.steps ?? {});

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

                {/* 步骤列表 */}
                {stepEntries.length > 0 && (
                  <div className="monitor-steps">
                    <h4>执行步骤 ({stepEntries.length})</h4>
                    {stepEntries.map(([nodeId, step]) => {
                      const stepStatus = STEP_STATUS_LABELS[step.status] || { label: step.status, color: '#6B7280' };
                      return (
                        <div key={nodeId} className="monitor-step">
                          <div className="step-indicator" style={{ backgroundColor: stepStatus.color }} />
                          <div className="step-info">
                            <span className="step-label">{nodeId}</span>
                            {step.error && <span className="step-error">{step.error}</span>}
                          </div>
                          <span className="step-status" style={{ color: stepStatus.color }}>
                            {stepStatus.label}
                            {step.retryCount > 0 && ` (${step.retryCount})`}
                          </span>
                          {step.duration !== undefined && (
                            <span className="step-duration">{(step.duration / 1000).toFixed(1)}s</span>
                          )}
                        </div>
                      );
                    })}
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
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
