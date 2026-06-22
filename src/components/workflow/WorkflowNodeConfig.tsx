/**
 * WorkflowNodeConfig — 节点配置面板
 *
 * 编辑工作流节点的详细参数。
 */

import React, { useState, useEffect } from 'react';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import type { WorkflowNode } from '../../types/workflow';
import { workflowNodeTypeRegistry } from '../../utils/WorkflowNodeTypeRegistry';

interface Props {
  node: WorkflowNode;
  onUpdate: (updates: Partial<WorkflowNode>) => void;
  onDelete: () => void;
}

export const WorkflowNodeConfig: React.FC<Props> = ({ node, onUpdate, onDelete }) => {
  const meta = getNodeTypeMeta(node.type);

  const [label, setLabel] = useState(node.label);
  const [pluginId, setPluginId] = useState(node.pluginId || '');
  const [commandId, setCommandId] = useState(node.commandId || '');
  const [cron, setCron] = useState(node.cron || '');
  const [delayMs, setDelayMs] = useState(node.delayMs || 1000);
  const [timeoutMs, setTimeoutMs] = useState(node.timeoutMs || 30000);
  const [retryCount, setRetryCount] = useState(node.retryCount || 0);
  const [retryDelayMs, setRetryDelayMs] = useState(node.retryDelayMs || 1000);
  const [condition, setCondition] = useState(node.condition || '');
  const [paramsJson, setParamsJson] = useState(JSON.stringify(node.params || {}, null, 2));

  useEffect(() => {
    setLabel(node.label);
    setPluginId(node.pluginId || '');
    setCommandId(node.commandId || '');
    setCron(node.cron || '');
    setDelayMs(node.delayMs || 1000);
    setTimeoutMs(node.timeoutMs || 30000);
    setRetryCount(node.retryCount || 0);
    setRetryDelayMs(node.retryDelayMs || 1000);
    setCondition(node.condition || '');
    setParamsJson(JSON.stringify(node.params || {}, null, 2));
  }, [node.id]);

  // 获取插件节点类型的配置 schema
  const pluginNodeType = node.type === 'plugin:node' && node.nodeTypeId
    ? workflowNodeTypeRegistry.get(node.nodeTypeId)
    : undefined;

  const handleApply = () => {
    const updates: Partial<WorkflowNode> = { label };

    if (node.type === 'plugin:command') {
      updates.pluginId = pluginId;
      updates.commandId = commandId;
      try {
        updates.params = JSON.parse(paramsJson);
      } catch { /* keep existing */ }
    }

    if (node.type === 'trigger:cron') updates.cron = cron;
    if (node.type === 'delay') updates.delayMs = delayMs;
    if (node.type === 'condition') updates.condition = condition;

    updates.timeoutMs = timeoutMs;
    updates.retryCount = retryCount;
    updates.retryDelayMs = retryDelayMs;

    onUpdate(updates);
  };

  return (
    <div className="node-config">
      <h4>节点配置</h4>
      <div className="config-section">
        <label>类型</label>
        <div className="config-type" style={{ color: meta.color }}>
          {meta.icon} {meta.label}
        </div>
      </div>

      <div className="config-section">
        <label>名称</label>
        <input type="text" value={label} onChange={(e) => setLabel(e.target.value)} />
      </div>

      {node.type === 'plugin:command' && (
        <>
          <div className="config-section">
            <label>插件 ID</label>
            <input type="text" value={pluginId} onChange={(e) => setPluginId(e.target.value)} placeholder="plugin-id" />
          </div>
          <div className="config-section">
            <label>命令 ID</label>
            <input type="text" value={commandId} onChange={(e) => setCommandId(e.target.value)} placeholder="command-id" />
          </div>
          <div className="config-section">
            <label>参数 (JSON)</label>
            <textarea
              value={paramsJson}
              onChange={(e) => setParamsJson(e.target.value)}
              rows={4}
              className="config-textarea"
            />
          </div>
        </>
      )}

      {node.type === 'plugin:node' && (
        <>
          <div className="config-section">
            <label>插件节点类型</label>
            <div className="config-type config-type--plugin">
              {pluginNodeType?.pluginId}.{node.nodeTypeId}
            </div>
          </div>
          {pluginNodeType?.configSchema && (
            <div className="config-section">
              <label>配置 (JSON)</label>
              <textarea
                value={paramsJson}
                onChange={(e) => setParamsJson(e.target.value)}
                rows={4}
                className="config-textarea"
              />
              <div className="config-schema-hint">
                {Object.entries(pluginNodeType.configSchema).map(([key, schema]) => (
                  <div key={key} className="schema-field">
                    <span className="schema-field-name">{key}</span>
                    <span className="schema-field-type">{schema.type}</span>
                    {schema.description && <span className="schema-field-desc">{schema.description}</span>}
                  </div>
                ))}
              </div>
            </div>
          )}
        </>
      )}

      {node.type === 'trigger:cron' && (
        <div className="config-section">
          <label>Cron 表达式</label>
          <input type="text" value={cron} onChange={(e) => setCron(e.target.value)} placeholder="0 8 * * *" />
        </div>
      )}

      {node.type === 'delay' && (
        <div className="config-section">
          <label>延迟时间 (毫秒)</label>
          <input type="number" value={delayMs} onChange={(e) => setDelayMs(Number(e.target.value))} />
        </div>
      )}

      {node.type === 'condition' && (
        <div className="config-section">
          <label>条件表达式</label>
          <input type="text" value={condition} onChange={(e) => setCondition(e.target.value)} placeholder="${steps.xxx.output} === true" />
        </div>
      )}

      <div className="config-section">
        <label>超时 (毫秒)</label>
        <input type="number" value={timeoutMs} onChange={(e) => setTimeoutMs(Number(e.target.value))} />
      </div>

      <div className="config-section">
        <label>重试次数</label>
        <input type="number" value={retryCount} onChange={(e) => setRetryCount(Number(e.target.value))} />
      </div>

      <div className="config-section">
        <label>重试间隔 (毫秒)</label>
        <input type="number" value={retryDelayMs} onChange={(e) => setRetryDelayMs(Number(e.target.value))} />
      </div>

      <div className="config-actions">
        <button onClick={handleApply} className="btn-primary">应用</button>
        <button onClick={onDelete} className="btn-danger">删除节点</button>
      </div>
    </div>
  );
};
