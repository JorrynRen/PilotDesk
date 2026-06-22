/**
 * WorkflowEditor — 工作流可视化编辑器
 *
 * 拖拽式工作流编排画布，支持节点添加、连接、配置。
 * 内置节点类型 + 插件注册的节点类型动态合并。
 */

import React, { useState, useEffect } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { getNodeTypeMeta, generateId } from '../../workflow/WorkflowDefinition';
import { workflowNodeTypeRegistry } from '../../utils/WorkflowNodeTypeRegistry';
import { WorkflowNodeConfig } from './WorkflowNodeConfig';
import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType } from '../../types/workflow';

interface Props {
  definitionId: string;
  onClose: () => void;
}

const BUILTIN_NODE_TYPES: { type: WorkflowNodeType; label: string }[] = [
  { type: 'trigger:manual', label: '手动触发' },
  { type: 'trigger:cron', label: '定时触发' },
  { type: 'trigger:event', label: '事件触发' },
  { type: 'plugin:command', label: '插件命令' },
  { type: 'condition', label: '条件判断' },
  { type: 'parallel', label: '并行执行' },
  { type: 'delay', label: '延迟等待' },
  { type: 'approval', label: '人工审批' },
];

/** 合并内置节点类型和插件注册的节点类型 */
function useNodeTypes(): { type: WorkflowNodeType; label: string; pluginId?: string }[] {
  const pluginTypes = workflowNodeTypeRegistry.getPlugin().map((e) => ({
    type: 'plugin:node' as WorkflowNodeType,
    label: `[${e.pluginId}] ${e.name}`,
    pluginId: e.pluginId,
  }));
  return [...BUILTIN_NODE_TYPES, ...pluginTypes];
}

export const WorkflowEditor: React.FC<Props> = ({ definitionId, onClose }) => {
  const { definitions, updateDefinition } = useWorkflowStore();
  const def = definitions.find((d) => d.id === definitionId);

  const [nodes, setNodes] = useState<WorkflowNode[]>(def?.nodes || []);
  const [edges, setEdges] = useState<WorkflowEdge[]>(def?.edges || []);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [name, setName] = useState(def?.name || '');
  const [description, setDescription] = useState(def?.description || '');
  const [dragging, setDragging] = useState<{ type: WorkflowNodeType } | null>(null);

  useEffect(() => {
    if (def) {
      setNodes(def.nodes);
      setEdges(def.edges);
      setName(def.name);
      setDescription(def.description);
    }
  }, [def]);

  const handleSave = async () => {
    if (!def) return;
    await updateDefinition(definitionId, { name, description, nodes, edges });
  };

  const handleAddNode = (type: WorkflowNodeType) => {
    const meta = getNodeTypeMeta(type);
    const newNode: WorkflowNode = {
      id: generateId(),
      type,
      label: meta.label,
      position: { x: 100 + nodes.length * 20, y: 100 + nodes.length * 30 },
    };
    setNodes([...nodes, newNode]);
  };

  const handleDeleteNode = (nodeId: string) => {
    setNodes(nodes.filter((n) => n.id !== nodeId));
    setEdges(edges.filter((e) => e.source !== nodeId && e.target !== nodeId));
    if (selectedNodeId === nodeId) setSelectedNodeId(null);
  };

  const handleUpdateNode = (nodeId: string, updates: Partial<WorkflowNode>) => {
    setNodes(nodes.map((n) => (n.id === nodeId ? { ...n, ...updates } : n)));
  };

  const handleConnect = (sourceId: string, targetId: string) => {
    if (edges.some((e) => e.source === sourceId && e.target === targetId)) return;
    if (sourceId === targetId) return;

    const newEdge: WorkflowEdge = {
      id: generateId(),
      source: sourceId,
      target: targetId,
    };
    setEdges([...edges, newEdge]);
  };

  const handleDeleteEdge = (edgeId: string) => {
    setEdges(edges.filter((e) => e.id !== edgeId));
  };

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);

  const allTypes = useNodeTypes();
  const builtinTypes = allTypes.filter(t => !t.pluginId);
  const pluginTypes = allTypes.filter(t => t.pluginId);

  return (
    <div className="workflow-editor">
      {/* 顶部栏 */}
      <div className="editor-header">
        <div className="editor-info">
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="editor-name-input"
            placeholder="工作流名称"
          />
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="editor-desc-input"
            placeholder="描述（可选）"
          />
        </div>
        <div className="editor-actions">
          <button onClick={handleSave} className="btn-primary">保存</button>
          <button onClick={onClose}>关闭</button>
        </div>
      </div>

      <div className="editor-body">
        {/* 左侧节点面板 */}
        <div className="editor-node-palette">
          <h4>节点类型</h4>
          {builtinTypes.map((nt) => {
            const meta = getNodeTypeMeta(nt.type);
            return (
              <div
                key={nt.type}
                className="palette-item"
                onClick={() => handleAddNode(nt.type)}
                style={{ borderLeftColor: meta.color }}
              >
                <span className="palette-icon">{meta.icon}</span>
                <span className="palette-label">{nt.label}</span>
              </div>
            );
          })}
          {pluginTypes.length > 0 && (
            <div className="palette-divider">
              <span className="palette-divider-label">插件节点</span>
            </div>
          )}
          {pluginTypes.map((nt) => {
            const meta = getNodeTypeMeta(nt.type);
            return (
              <div
                key={nt.pluginId || nt.type}
                className="palette-item palette-item--plugin"
                onClick={() => handleAddNode(nt.type)}
                style={{ borderLeftColor: meta.color }}
              >
                <span className="palette-icon">{meta.icon}</span>
                <span className="palette-label">{nt.label}</span>
              </div>
            );
          })}
        </div>

        {/* 中间画布 */}
        <div className="editor-canvas">
          {nodes.length === 0 ? (
            <div className="canvas-empty">
              <p>点击左侧节点类型添加节点</p>
            </div>
          ) : (
            <div className="canvas-nodes">
              {nodes.map((node) => {
                const meta = getNodeTypeMeta(node.type);
                const isSelected = node.id === selectedNodeId;
                return (
                  <div
                    key={node.id}
                    className={`canvas-node ${isSelected ? 'selected' : ''}`}
                    style={{
                      left: node.position?.x || 0,
                      top: node.position?.y || 0,
                      borderColor: meta.color,
                    }}
                    onClick={() => setSelectedNodeId(node.id)}
                  >
                    <div className="node-header" style={{ backgroundColor: meta.color }}>
                      <span>{meta.icon}</span>
                      <span className="node-label">{node.label}</span>
                    </div>
                    <div className="node-body">
                      {node.type === 'plugin:command' && node.pluginId && (
                        <span className="node-detail">{node.pluginId}.{node.commandId}</span>
                      )}
                      {node.type === 'trigger:cron' && node.cron && (
                        <span className="node-detail">{node.cron}</span>
                      )}
                      {node.type === 'delay' && node.delayMs && (
                        <span className="node-detail">{node.delayMs}ms</span>
                      )}
                    </div>
                    <div className="node-ports">
                      {meta.canHaveInputs && <div className="port port-input" />}
                      {meta.canHaveOutputs && <div className="port port-output" />}
                    </div>
                    <button
                      className="node-delete-btn"
                      onClick={(e) => { e.stopPropagation(); handleDeleteNode(node.id); }}
                    >
                      ×
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          {/* 连接线列表 */}
          <div className="canvas-edges">
            {edges.map((edge) => {
              const sourceNode = nodes.find((n) => n.id === edge.source);
              const targetNode = nodes.find((n) => n.id === edge.target);
              return (
                <div key={edge.id} className="canvas-edge" title={`${sourceNode?.label} → ${targetNode?.label}`}>
                  <span className="edge-label">{edge.label || '→'}</span>
                  <button className="edge-delete-btn" onClick={() => handleDeleteEdge(edge.id)}>×</button>
                </div>
              );
            })}
          </div>

          {/* 连接创建器 */}
          <div className="canvas-connector">
            <select
              value=""
              onChange={(e) => {
                const val = e.target.value;
                if (!val) return;
                const [source, target] = val.split(':');
                handleConnect(source, target);
                e.target.value = '';
              }}
            >
              <option value="">创建连接...</option>
              {nodes.map((src) =>
                nodes
                  .filter((tgt) => tgt.id !== src.id)
                  .map((tgt) => (
                    <option key={`${src.id}:${tgt.id}`} value={`${src.id}:${tgt.id}`}>
                      {src.label} → {tgt.label}
                    </option>
                  ))
              )}
            </select>
          </div>
        </div>

        {/* 右侧配置面板 */}
        <div className="editor-config">
          {selectedNode ? (
            <WorkflowNodeConfig
              node={selectedNode}
              onUpdate={(updates) => handleUpdateNode(selectedNode.id, updates)}
              onDelete={() => handleDeleteNode(selectedNode.id)}
            />
          ) : (
            <div className="config-empty">
              <p>选择一个节点查看配置</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
