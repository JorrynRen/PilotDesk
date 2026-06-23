/**
 * WorkflowEditor — 工作流可视化编辑器（Structured Canvas）
 *
 * 阶段栏 + 自由画布，支持拖拽节点、连线、条件标签。
 * 智能连线自动归入阶段。
 */

import React, { useState, useEffect, useCallback } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { getNodeTypeMeta, generateId, generateEdgeId, generateStageId, autoAssignStage } from '../../workflow/WorkflowDefinition';
import { workflowNodeTypeRegistry } from '../../utils/WorkflowNodeTypeRegistry';
import { WorkflowNodeConfig } from './WorkflowNodeConfig';
import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType, Stage, GateConfig } from '../../types/workflow';

interface Props {
  definitionId: string;
  onClose: () => void;
}

const BUILTIN_NODE_TYPES: { type: WorkflowNodeType; label: string; icon: string; color: string }[] = [
  { type: 'agent', label: 'Agent 任务', icon: '🤖', color: '#58a6ff' },
  { type: 'api', label: 'API 调用', icon: '🔗', color: '#a371f7' },
  { type: 'transform', label: '代码转换', icon: '⚡', color: '#d29922' },
  { type: 'interact', label: '人工交互', icon: '👤', color: '#f85149' },
  { type: 'plugin', label: '插件命令', icon: '🧩', color: '#3fb950' },
  { type: 'subflow', label: '子工作流', icon: '📦', color: '#79c0ff' },
];

export const WorkflowEditor: React.FC<Props> = ({ definitionId, onClose }) => {
  const { definitions, updateDefinition } = useWorkflowStore();
  const def = definitions.find((d) => d.id === definitionId);

  const [stages, setStages] = useState<Stage[]>(def?.stages || []);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedStageId, setSelectedStageId] = useState<string | null>(null);
  const [name, setName] = useState(def?.name || '');
  const [description, setDescription] = useState(def?.description || '');
  const [connecting, setConnecting] = useState<{ source: string; stageId: string } | null>(null);
  const [conditionInput, setConditionInput] = useState<{ source: string; target: string; stageId: string } | null>(null);

  useEffect(() => {
    if (def) {
      setStages(def.stages);
      setName(def.name);
      setDescription(def.description);
    }
  }, [def]);

  const handleSave = async () => {
    if (!def) return;
    await updateDefinition(definitionId, { name, description, stages });
  };

  const handleAddStage = () => {
    const newStage: Stage = {
      id: generateStageId(),
      name: `阶段 ${stages.length + 1}`,
      order: stages.length,
      nodes: [],
      edges: [],
      gate: { strategy: 'all', mergeStrategy: 'merge' },
    };
    setStages([...stages, newStage]);
  };

  const handleDeleteStage = (stageId: string) => {
    setStages(stages.filter((s) => s.id !== stageId).map((s, i) => ({ ...s, order: i })));
  };

  const handleAddNode = (type: WorkflowNodeType, stageId: string) => {
    const meta = getNodeTypeMeta(type);
    const newNode: WorkflowNode = {
      id: generateId(),
      type,
      label: meta.label,
      position: { x: 60 + Math.random() * 100, y: 60 + Math.random() * 100 },
    };
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, nodes: [...s.nodes, newNode] } : s
    ));
  };

  const handleDeleteNode = (nodeId: string) => {
    setStages(stages.map((s) => ({
      ...s,
      nodes: s.nodes.filter((n) => n.id !== nodeId),
      edges: s.edges.filter((e) => e.source !== nodeId && e.target !== nodeId),
    })));
    if (selectedNodeId === nodeId) setSelectedNodeId(null);
  };

  const handleUpdateNode = (nodeId: string, updates: Partial<WorkflowNode>) => {
    setStages(stages.map((s) => ({
      ...s,
      nodes: s.nodes.map((n) => n.id === nodeId ? { ...n, ...updates } : n),
    })));
  };

  // 连线操作
  const handleStartConnect = (nodeId: string, stageId: string) => {
    setConnecting({ source: nodeId, stageId });
  };

  const handleEndConnect = (targetId: string, targetStageId: string) => {
    if (!connecting || connecting.source === targetId) {
      setConnecting(null);
      return;
    }

    const newEdge: WorkflowEdge = {
      id: generateEdgeId(connecting.source, targetId),
      source: connecting.source,
      target: targetId,
    };

    // 如果跨阶段连线，弹出条件编辑框
    if (connecting.stageId !== targetStageId) {
      setConditionInput({ source: connecting.source, target: targetId, stageId: targetStageId });
      setConnecting(null);
      return;
    }

    // 同阶段连线
    setStages(stages.map((s) => {
      if (s.id === connecting.stageId) {
        return { ...s, edges: [...s.edges, newEdge] };
      }
      return s;
    }));
    setConnecting(null);

    // 触发自动归入阶段
    setStages((prev) => autoAssignStage(prev));
  };

  const handleConfirmCondition = (condition: string, label: string) => {
    if (!conditionInput) return;
    const newEdge: WorkflowEdge = {
      id: generateEdgeId(conditionInput.source, conditionInput.target),
      source: conditionInput.source,
      target: conditionInput.target,
      condition,
      label: label || condition,
    };

    setStages(stages.map((s) => {
      if (s.id === conditionInput.stageId) {
        return { ...s, edges: [...s.edges, newEdge] };
      }
      return s;
    }));
    setConditionInput(null);

    // 触发自动归入阶段
    setStages((prev) => autoAssignStage(prev));
  };

  const handleDeleteEdge = (edgeId: string) => {
    setStages(stages.map((s) => ({
      ...s,
      edges: s.edges.filter((e) => e.id !== edgeId),
    })));
  };

  const handleUpdateGate = (stageId: string, gate: Partial<GateConfig>) => {
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, gate: { ...s.gate, ...gate } } : s
    ));
  };

  const handleRenameStage = (stageId: string, name: string) => {
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, name } : s
    ));
  };

  const handleMoveNode = (nodeId: string, stageId: string, position: { x: number; y: number }) => {
    setStages(stages.map((s) => ({
      ...s,
      nodes: s.nodes.map((n) => n.id === nodeId ? { ...n, position } : n),
    })));
  };

  // 获取节点在画布中的位置
  const getNodePosition = (stageId: string, nodeId: string): { x: number; y: number } | undefined => {
    const stage = stages.find((s) => s.id === stageId);
    return stage?.nodes.find((n) => n.id === nodeId)?.position as { x: number; y: number } | undefined;
  };

  return (
    <div className="workflow-editor" style={{ display: 'flex', flexDirection: 'column', height: '100%', background: '#0d1117' }}>
      {/* 标题栏 */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 20px', borderBottom: '1px solid #21262d' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            style={{ background: 'transparent', border: 'none', color: '#f0f6fc', fontSize: 16, fontWeight: 600, outline: 'none' }}
            placeholder="工作流名称"
          />
          <span style={{ fontSize: 11, color: '#8b949e', padding: '2px 8px', borderRadius: 10, background: '#1f6feb22', border: '1px solid #1f6feb44' }}>
            {def?.version || 'v1.0.0'}
          </span>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button onClick={onClose} style={{ padding: '6px 14px', borderRadius: 6, border: '1px solid #30363d', background: '#21262d', color: '#c9d1d9', fontSize: 12, cursor: 'pointer' }}>关闭</button>
          <button onClick={handleSave} style={{ padding: '6px 14px', borderRadius: 6, border: 'none', background: '#238636', color: '#fff', fontSize: 12, cursor: 'pointer' }}>保存</button>
        </div>
      </div>

      {/* 工具栏 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 4, padding: '8px 20px', borderBottom: '1px solid #21262d', background: '#0d1117', flexWrap: 'wrap' }}>
        <span style={{ fontSize: 11, color: '#8b949e', marginRight: 4 }}>节点类型:</span>
        {BUILTIN_NODE_TYPES.map((nt) => (
          <div
            key={nt.type}
            draggable
            onDragStart={(e) => e.dataTransfer.setData('text/plain', nt.type)}
            style={{ display: 'flex', alignItems: 'center', gap: 4, padding: '4px 10px', borderRadius: 6, border: '1px solid #30363d', background: '#161b22', color: '#c9d1d9', fontSize: 12, cursor: 'grab', userSelect: 'none' }}
          >
            <span>{nt.icon}</span>
            <span>{nt.label}</span>
          </div>
        ))}
        <div style={{ width: 1, height: 24, background: '#30363d', margin: '0 8px' }} />
        <button onClick={handleAddStage} style={{ padding: '4px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#161b22', color: '#c9d1d9', fontSize: 12, cursor: 'pointer' }}>+ 添加阶段</button>
      </div>

      {/* 画布 */}
      <div style={{ flex: 1, overflow: 'auto', padding: 16, display: 'flex', gap: 16, minHeight: 400 }}>
        {stages.length === 0 && (
          <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', color: '#8b949e', fontSize: 14 }}>
            点击 "+ 添加阶段" 开始创建工作流
          </div>
        )}

        {stages.map((stage) => (
          <div key={stage.id} style={{ flex: 1, minWidth: 300, border: '1px solid #21262d', borderRadius: 8, display: 'flex', flexDirection: 'column', background: '#161b22' }}>
            {/* 阶段标题 */}
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '8px 12px', borderBottom: '1px solid #21262d', background: '#0d1117', borderRadius: '8px 8px 0 0' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ display: 'inline-flex', width: 20, height: 20, alignItems: 'center', justifyContent: 'center', borderRadius: 4, background: '#1f6feb33', color: '#58a6ff', fontSize: 11, fontWeight: 700 }}>{stage.order + 1}</span>
                <input
                  value={stage.name}
                  onChange={(e) => handleRenameStage(stage.id, e.target.value)}
                  style={{ background: 'transparent', border: 'none', color: '#f0f6fc', fontSize: 13, fontWeight: 600, outline: 'none', width: 100 }}
                />
              </div>
              <div style={{ display: 'flex', gap: 4 }}>
                <button onClick={() => handleAddNode('agent', stage.id)} style={{ padding: '2px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#21262d', color: '#8b949e', fontSize: 10, cursor: 'pointer' }}>+ 节点</button>
                <button onClick={() => handleDeleteStage(stage.id)} style={{ padding: '2px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#21262d', color: '#f85149', fontSize: 10, cursor: 'pointer' }}>×</button>
              </div>
            </div>

            {/* 画布区 */}
            <div
              style={{ flex: 1, minHeight: 250, padding: 12, position: 'relative' }}
              onDragOver={(e) => e.preventDefault()}
              onDrop={(e) => {
                e.preventDefault();
                const type = e.dataTransfer.getData('text/plain') as WorkflowNodeType;
                if (type) {
                  const rect = e.currentTarget.getBoundingClientRect();
                  const meta = getNodeTypeMeta(type);
                  const newNode: WorkflowNode = {
                    id: generateId(),
                    type,
                    label: meta.label,
                    position: { x: e.clientX - rect.left - 60, y: e.clientY - rect.top - 20 },
                  };
                  setStages(stages.map((s) =>
                    s.id === stage.id ? { ...s, nodes: [...s.nodes, newNode] } : s
                  ));
                }
              }}
            >
              {/* 节点 */}
              {stage.nodes.map((node) => {
                const meta = getNodeTypeMeta(node.type);
                const pos = node.position as { x: number; y: number } | undefined;
                return (
                  <div
                    key={node.id}
                    onClick={() => { setSelectedNodeId(node.id); setSelectedStageId(stage.id); }}
                    style={{
                      position: 'absolute',
                      left: pos?.x ?? 20,
                      top: pos?.y ?? 20,
                      width: 160,
                      padding: '8px 10px',
                      borderRadius: 8,
                      border: selectedNodeId === node.id ? '1px solid #58a6ff' : '1px solid #30363d',
                      background: '#1c2128',
                      cursor: 'pointer',
                      boxShadow: selectedNodeId === node.id ? '0 0 0 2px #1f6feb44' : 'none',
                      zIndex: 10,
                    }}
                  >
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 2 }}>
                      <div style={{ width: 24, height: 24, borderRadius: 5, display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 12, background: `${meta.color}22`, color: meta.color }}>{meta.icon}</div>
                      <span style={{ fontSize: 12, fontWeight: 600, color: '#f0f6fc' }}>{node.label}</span>
                    </div>
                    <div style={{ marginLeft: 30, display: 'flex', gap: 3, flexWrap: 'wrap' }}>
                      {node.delayMs && <span style={{ fontSize: 9, padding: '1px 4px', borderRadius: 3, background: '#21262d', color: '#d29922', border: '1px solid #d2992244' }}>延迟 {node.delayMs}ms</span>}
                      {node.timeoutMs && <span style={{ fontSize: 9, padding: '1px 4px', borderRadius: 3, background: '#21262d', color: '#a371f7', border: '1px solid #8957e544' }}>超时 {node.timeoutMs/1000}s</span>}
                    </div>
                    {/* 输入锚点 */}
                    <div style={{ position: 'absolute', left: -5, top: '50%', marginTop: -5, width: 10, height: 10, borderRadius: '50%', background: '#30363d', border: '2px solid #0d1117', cursor: 'crosshair' }}
                      onClick={(e) => { e.stopPropagation(); handleStartConnect(node.id, stage.id); }}
                    />
                    {/* 输出锚点 */}
                    <div style={{ position: 'absolute', right: -5, top: '50%', marginTop: -5, width: 10, height: 10, borderRadius: '50%', background: '#30363d', border: '2px solid #0d1117', cursor: 'crosshair' }}
                      onClick={(e) => { e.stopPropagation(); handleEndConnect(node.id, stage.id); }}
                    />
                    {/* 删除按钮 */}
                    <div
                      onClick={(e) => { e.stopPropagation(); handleDeleteNode(node.id); }}
                      style={{ position: 'absolute', top: -6, right: -6, width: 16, height: 16, borderRadius: '50%', background: '#f85149', color: '#fff', fontSize: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', cursor: 'pointer', opacity: 0.8 }}
                    >×</div>
                  </div>
                );
              })}

              {/* 边（简化显示） */}
              {stage.edges.map((edge) => {
                const sourcePos = stage.nodes.find((n) => n.id === edge.source)?.position as { x: number; y: number } | undefined;
                const targetPos = stage.nodes.find((n) => n.id === edge.target)?.position as { x: number; y: number } | undefined;
                if (!sourcePos || !targetPos) return null;
                return (
                  <div key={edge.id} style={{ position: 'absolute', left: 0, top: 0, width: '100%', height: '100%', pointerEvents: 'none' }}>
                    <svg style={{ position: 'absolute', left: 0, top: 0, width: '100%', height: '100%' }}>
                      <path
                        d={`M ${sourcePos.x + 160} ${sourcePos.y + 20} C ${sourcePos.x + 190} ${sourcePos.y + 20}, ${targetPos.x - 10} ${targetPos.y + 20}, ${targetPos.x} ${targetPos.y + 20}`}
                        stroke={edge.condition ? '#d29922' : '#58a6ff'}
                        strokeWidth={2}
                        fill="none"
                        markerEnd="url(#arrowhead)"
                      />
                      {edge.label && (
                        <text x={(sourcePos.x + 160 + targetPos.x) / 2} y={(sourcePos.y + targetPos.y) / 2 - 8}
                          fill="#8b949e" fontSize={10} textAnchor="middle"
                          style={{ pointerEvents: 'auto', cursor: 'pointer' }}
                          onClick={() => handleDeleteEdge(edge.id)}
                        >
                          {edge.label}
                        </text>
                      )}
                    </svg>
                  </div>
                );
              })}
            </div>

            {/* Gate */}
            <div style={{ margin: '0 8px 8px 8px', padding: '8px 12px', border: '1px solid #30363d', borderRadius: 8, background: '#0d1117', cursor: 'pointer' }}
              onClick={() => handleUpdateGate(stage.id, {})}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 4 }}>
                <span style={{ fontSize: 11, fontWeight: 600, color: '#8b949e', display: 'flex', alignItems: 'center', gap: 4 }}>⊞ 门控 Gate</span>
                <span style={{ fontSize: 11, color: '#3fb950', display: 'flex', alignItems: 'center', gap: 4 }}>
                  <span style={{ width: 6, height: 6, borderRadius: '50%', background: '#3fb950' }} />
                  {stage.nodes.length}/{stage.nodes.length} 就绪
                </span>
              </div>
              <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                <span style={{ fontSize: 11, color: '#8b949e' }}>策略: <span style={{ color: '#c9d1d9', background: '#21262d', padding: '1px 6px', borderRadius: 3, fontSize: 10 }}>{stage.gate.strategy}</span></span>
                <span style={{ fontSize: 11, color: '#8b949e' }}>合并: <span style={{ color: '#c9d1d9', background: '#21262d', padding: '1px 6px', borderRadius: 3, fontSize: 10 }}>{stage.gate.mergeStrategy}</span></span>
              </div>
            </div>
          </div>
        ))}
      </div>

      {/* 条件编辑弹窗 */}
      {conditionInput && (
        <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.6)', display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 1000 }}>
          <div style={{ background: '#1c2128', border: '1px solid #30363d', borderRadius: 12, padding: 24, maxWidth: 400, width: '90%' }}>
            <h3 style={{ fontSize: 14, color: '#f0f6fc', marginBottom: 12 }}>编辑条件</h3>
            <div style={{ marginBottom: 12 }}>
              <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 4 }}>条件表达式</label>
              <input
                id="condition-expr"
                placeholder="例如: == approve 或 contains 紧急"
                style={{ width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
              />
            </div>
            <div style={{ marginBottom: 16 }}>
              <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 4 }}>条件标签（显示在边上）</label>
              <input
                id="condition-label"
                placeholder="例如: 审批通过 / 紧急"
                style={{ width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
              />
            </div>
            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
              <button onClick={() => setConditionInput(null)} style={{ padding: '6px 16px', borderRadius: 6, border: '1px solid #30363d', background: '#21262d', color: '#c9d1d9', fontSize: 12, cursor: 'pointer' }}>取消</button>
              <button onClick={() => {
                const expr = (document.getElementById('condition-expr') as HTMLInputElement)?.value || '';
                const label = (document.getElementById('condition-label') as HTMLInputElement)?.value || '';
                handleConfirmCondition(expr, label);
              }} style={{ padding: '6px 16px', borderRadius: 6, border: 'none', background: '#238636', color: '#fff', fontSize: 12, cursor: 'pointer' }}>确认</button>
            </div>
          </div>
        </div>
      )}

      {/* 节点配置面板 */}
      {selectedNodeId && selectedStageId && (
        <div style={{ position: 'fixed', right: 0, top: 0, bottom: 0, width: 360, background: '#161b22', borderLeft: '1px solid #30363d', zIndex: 100, overflow: 'auto', padding: 20 }}>
          <WorkflowNodeConfig
            node={stages.find((s) => s.id === selectedStageId)?.nodes.find((n) => n.id === selectedNodeId)!}
            onUpdate={(updates) => handleUpdateNode(selectedNodeId, updates)}
            onClose={() => { setSelectedNodeId(null); setSelectedStageId(null); }}
          />
        </div>
      )}
    </div>
  );
};
