/**
 * WorkflowEditor — 工作流可视化编辑器（Structured Canvas）
 *
 * 自由画布 + 阶段列布局，支持：
 * - 画布拖拽平移 + 鼠标滚轮缩放
 * - 节点拖拽移动
 * - 节点连线（从输出锚点到输入锚点）
 * - 阶段折叠/展开
 * - CSS变量主题支持
 */

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { getNodeTypeMeta, generateId, generateEdgeId, generateStageId, autoAssignStage } from '../../workflow/WorkflowDefinition';
import { WorkflowNodeConfig } from './WorkflowNodeConfig';
import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType, Stage, GateConfig } from '../../types/workflow';

interface Props {
  definitionId: string;
  onClose: () => void;
  onNameChange?: (name: string) => void;
}

const BUILTIN_NODE_TYPES: { type: WorkflowNodeType; label: string; icon: string; color: string }[] = [
  { type: 'agent', label: 'Agent 任务', icon: '🤖', color: '#58a6ff' },
  { type: 'api', label: 'API 调用', icon: '🔗', color: '#a371f7' },
  { type: 'transform', label: '代码转换', icon: '\u{26A1}', color: '#d29922' },
  { type: 'interact', label: '人工交互', icon: '\u{1F464}', color: '#f85149' },
  { type: 'plugin', label: '插件命令', icon: '\u{1F9E9}', color: '#3fb950' },
  { type: 'subflow', label: '子工作流', icon: '\u{1F4E6}', color: '#79c0ff' },
];

export const WorkflowEditor: React.FC<Props> = ({ definitionId, onClose, onNameChange }) => {
  const { definitions, updateDefinition, loadDefinitions } = useWorkflowStore();
  const def = definitions.find((d) => d.id === definitionId);

  const [stages, setStages] = useState<Stage[]>(def?.stages || []);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedStageId, setSelectedStageId] = useState<string | null>(null);
  const [name, setName] = useState(def?.name || '');
  const [description, setDescription] = useState(def?.description || '');
  const [connecting, setConnecting] = useState<{ source: string; stageId: string } | null>(null);
  const [conditionInput, setConditionInput] = useState<{ source: string; target: string; stageId: string } | null>(null);
  const [collapsedStages, setCollapsedStages] = useState<Set<string>>(new Set());

  // 画布平移和缩放状态
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [scale, setScale] = useState(1);
  const [isPanning, setIsPanning] = useState(false);
  const panStartRef = useRef<{ x: number; y: number; panX: number; panY: number } | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);

  // 节点拖拽状态
  const [draggingNode, setDraggingNode] = useState<{ nodeId: string; stageId: string; offsetX: number; offsetY: number } | null>(null);

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

  const handleNameChangeLocal = useCallback((newName: string) => {
    setName(newName);
    onNameChange?.(newName);
  }, [onNameChange]);

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

    if (connecting.stageId !== targetStageId) {
      setConditionInput({ source: connecting.source, target: targetId, stageId: targetStageId });
      setConnecting(null);
      return;
    }

    setStages(stages.map((s) => {
      if (s.id === connecting.stageId) {
        return { ...s, edges: [...s.edges, newEdge] };
      }
      return s;
    }));
    setConnecting(null);
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

  const toggleCollapseStage = (stageId: string) => {
    setCollapsedStages((prev) => {
      const next = new Set(prev);
      if (next.has(stageId)) next.delete(stageId);
      else next.add(stageId);
      return next;
    });
  };

  // === 画布拖拽平移 ===
  const handleCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    // Only pan on left click on empty canvas area (not on nodes, buttons, inputs)
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest('button, input, [data-node], [data-anchor]')) return;
    setIsPanning(true);
    panStartRef.current = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
    e.preventDefault();
  }, [pan.x, pan.y]);

  useEffect(() => {
    if (!isPanning) return;
    const handleMouseMove = (e: MouseEvent) => {
      if (!panStartRef.current) return;
      const dx = e.clientX - panStartRef.current.x;
      const dy = e.clientY - panStartRef.current.y;
      setPan({
        x: panStartRef.current.panX + dx,
        y: panStartRef.current.panY + dy,
      });
    };
    const handleMouseUp = () => {
      setIsPanning(false);
      panStartRef.current = null;
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isPanning]);

  // === 鼠标滚轮缩放 ===
  const handleWheel = useCallback((e: React.WheelEvent) => {
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      setScale((prev) => {
        const next = prev - e.deltaY * 0.001;
        return Math.min(Math.max(next, 0.25), 3);
      });
    } else {
      // 非Ctrl时滚动 = 画布平移
      setPan((prev) => ({
        x: prev.x - e.deltaX,
        y: prev.y - e.deltaY,
      }));
    }
  }, []);

  // === 节点拖拽 ===
  const handleNodeMouseDown = useCallback((e: React.MouseEvent, nodeId: string, stageId: string) => {
    e.stopPropagation();
    e.preventDefault();
    const node = stages.find((s) => s.id === stageId)?.nodes.find((n) => n.id === nodeId);
    if (!node) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const pos = node.position ?? { x: 0, y: 0 };
    const nodeX = (pos.x + pan.x) * scale + rect.left;
    const nodeY = (pos.y + pan.y) * scale + rect.top;
    setDraggingNode({
      nodeId,
      stageId,
      offsetX: e.clientX - nodeX,
      offsetY: e.clientY - nodeY,
    });
  }, [stages, pan.x, pan.y, scale]);

  useEffect(() => {
    if (!draggingNode) return;
    const handleMouseMove = (e: MouseEvent) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const newAbsX = e.clientX - draggingNode.offsetX;
      const newAbsY = e.clientY - draggingNode.offsetY;
      const canvasX = newAbsX - rect.left;
      const canvasY = newAbsY - rect.top;
      const nodeX = canvasX / scale - pan.x;
      const nodeY = canvasY / scale - pan.y;
      setStages((prev) =>
        prev.map((s) =>
          s.id === draggingNode.stageId
            ? {
                ...s,
                nodes: s.nodes.map((n) =>
                  n.id === draggingNode.nodeId
                    ? { ...n, position: { x: Math.max(0, nodeX), y: Math.max(0, nodeY) } }
                    : n
                ),
              }
            : s
        )
      );
    };
    const handleMouseUp = () => {
      setDraggingNode(null);
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [draggingNode, scale, pan.x, pan.y]);

  // 渲染节点
  const renderNode = (node: WorkflowNode, stageId: string) => {
    const meta = getNodeTypeMeta(node.type);
    const pos = node.position as { x: number; y: number } | undefined;
    const isSelected = selectedNodeId === node.id;
    const isDraggingThis = draggingNode?.nodeId === node.id;

    return (
      <div
        key={node.id}
        data-node
        onClick={(e) => { e.stopPropagation(); setSelectedNodeId(node.id); setSelectedStageId(stageId); }}
        onMouseDown={(e) => handleNodeMouseDown(e, node.id, stageId)}
        className="cursor-grab active:cursor-grabbing"
        style={{
          position: 'absolute',
          left: pos?.x ?? 20,
          top: pos?.y ?? 20,
          width: 160,
          padding: '8px 10px',
          borderRadius: 8,
          border: isSelected ? '1px solid var(--accent)' : '1px solid var(--border)',
          background: 'var(--bg-tertiary)',
          boxShadow: isSelected ? '0 0 0 2px var(--accent-light)' : 'var(--shadow-sm)',
          zIndex: isDraggingThis ? 20 : 10,
          userSelect: 'none',
        }}
      >
        <div className="flex items-center gap-1.5 mb-1">
          <div style={{ width: 24, height: 24, borderRadius: 5, display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 12, background: `${meta.color}22`, color: meta.color }}>{meta.icon}</div>
          <span className="text-xs font-semibold truncate" style={{ color: 'var(--text-primary)' }}>{node.label}</span>
        </div>
        <div className="flex gap-1 flex-wrap ml-[30px]">
          {node.delayMs && <span className="text-[9px] px-1 py-0.5 rounded" style={{ background: 'var(--bg-secondary)', color: '#d29922', border: '1px solid #d2992244' }}>延迟 {node.delayMs}ms</span>}
          {node.timeoutMs && <span className="text-[9px] px-1 py-0.5 rounded" style={{ background: 'var(--bg-secondary)', color: '#a371f7', border: '1px solid #8957e544' }}>超时 {node.timeoutMs/1000}s</span>}
        </div>
        {/* 输入锚点 */}
        <div
          data-anchor="input"
          onClick={(e) => { e.stopPropagation(); handleStartConnect(node.id, stageId); }}
          className="absolute rounded-full"
          style={{ left: -5, top: '50%', marginTop: -5, width: 10, height: 10, background: 'var(--border)', border: '2px solid var(--bg-primary)', cursor: 'crosshair' }}
        />
        {/* 输出锚点 */}
        <div
          data-anchor="output"
          onClick={(e) => { e.stopPropagation(); handleEndConnect(node.id, stageId); }}
          className="absolute rounded-full"
          style={{ right: -5, top: '50%', marginTop: -5, width: 10, height: 10, background: 'var(--border)', border: '2px solid var(--bg-primary)', cursor: 'crosshair' }}
        />
        {/* 删除按钮 */}
        <div
          onClick={(e) => { e.stopPropagation(); handleDeleteNode(node.id); }}
          className="absolute flex items-center justify-center rounded-full opacity-80 cursor-pointer"
          style={{ top: -6, right: -6, width: 16, height: 16, background: 'var(--status-danger)', color: '#fff', fontSize: 10 }}
        >x</div>
      </div>
    );
  };

  // 渲染连线
  const renderEdge = (edge: WorkflowEdge, stageId: string) => {
    const stage = stages.find((s) => s.id === stageId);
    if (!stage) return null;
    const sourceNode = stage.nodes.find((n) => n.id === edge.source);
    const targetNode = stage.nodes.find((n) => n.id === edge.target);
    if (!sourceNode || !targetNode) return null;
    const sp = sourceNode.position as { x: number; y: number } | undefined;
    const tp = targetNode.position as { x: number; y: number } | undefined;
    if (!sp || !tp) return null;

    return (
      <g key={edge.id}>
        <path
          d={`M ${sp.x + 160} ${sp.y + 20} C ${sp.x + 190} ${sp.y + 20}, ${tp.x - 10} ${tp.y + 20}, ${tp.x} ${tp.y + 20}`}
          stroke={edge.condition ? '#d29922' : 'var(--accent)'}
          strokeWidth={2}
          fill="none"
          markerEnd="url(#arrowhead)"
          style={{ pointerEvents: 'stroke', cursor: 'pointer' }}
          onClick={() => handleDeleteEdge(edge.id)}
        />
        {edge.label && (
          <text
            x={(sp.x + 160 + tp.x) / 2}
            y={(sp.y + tp.y) / 2 - 8}
            fill="var(--text-secondary)"
            fontSize={10}
            textAnchor="middle"
            style={{ pointerEvents: 'auto', cursor: 'pointer' }}
            onClick={() => handleDeleteEdge(edge.id)}
          >
            {edge.label}
          </text>
        )}
      </g>
    );
  };

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* 工具栏（替代原硬编码标题栏） */}
      <div className="flex items-center gap-2 px-4 py-2 shrink-0" style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
        <input
          value={name}
          onChange={(e) => handleNameChangeLocal(e.target.value)}
          className="text-sm font-semibold outline-none"
          style={{ background: 'transparent', color: 'var(--text-primary)', border: 'none' }}
          placeholder="工作流名称"
        />
        <span className="text-[10px] px-2 py-0.5 rounded-full" style={{ color: 'var(--accent)', background: 'var(--accent-light)' }}>
          {def?.version || 'v1.0.0'}
        </span>

        <div className="flex-1" />

        {/* 节点类型拖拽区 */}
        <div className="flex items-center gap-1 flex-wrap">
          <span className="text-[10px] mr-1" style={{ color: 'var(--text-tertiary)' }}>节点:</span>
          {BUILTIN_NODE_TYPES.map((nt) => (
            <div
              key={nt.type}
              draggable
              onDragStart={(e) => e.dataTransfer.setData('text/plain', nt.type)}
              className="flex items-center gap-1 px-2 py-1 rounded text-[11px] cursor-grab select-none"
              style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
            >
              <span>{nt.icon}</span>
              <span>{nt.label}</span>
            </div>
          ))}
        </div>

        <div className="w-px h-5 mx-1" style={{ background: 'var(--border)' }} />

        <div className="flex items-center gap-1.5">
          <button
            onClick={handleAddStage}
            className="pd-btn px-2.5 py-1 text-[11px] rounded"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            + 阶段
          </button>
          <button
            onClick={onClose}
            className="pd-btn px-3 py-1 text-[11px] rounded"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            返回列表
          </button>
          <button
            onClick={handleSave}
            className="pd-btn px-3 py-1 text-[11px] rounded"
            style={{ background: 'var(--accent)', color: '#fff', border: 'none' }}
          >
            保存
          </button>
        </div>
      </div>

      {/* 画布区域 */}
      <div
        ref={canvasRef}
        className="flex-1 overflow-hidden relative"
        style={{ cursor: isPanning ? 'grabbing' : 'grab' }}
        onMouseDown={handleCanvasMouseDown}
        onWheel={handleWheel}
      >
        {/* 缩放指示器 */}
        <div className="absolute bottom-3 right-3 z-30 flex items-center gap-1.5">
          <button
            onClick={() => setScale((s) => Math.min(s + 0.1, 3))}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded text-xs"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
          >+</button>
          <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ background: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}>
            {Math.round(scale * 100)}%
          </span>
          <button
            onClick={() => setScale((s) => Math.max(s - 0.1, 0.25))}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded text-xs"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
          >-</button>
          <button
            onClick={() => { setPan({ x: 0, y: 0 }); setScale(1); }}
            className="pd-btn px-1.5 py-0.5 rounded text-[10px]"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
          >
            重置
          </button>
        </div>

        <div
          style={{
            transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale})`,
            transformOrigin: '0 0',
            width: 4000,
            height: 3000,
            position: 'relative',
          }}
        >
          {/* SVG 箭头标记 */}
          <svg style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: '100%', pointerEvents: 'none' }}>
            <defs>
              <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
                <polygon points="0 0, 8 3, 0 6" fill="var(--accent)" />
              </marker>
            </defs>
            {/* 渲染各阶段内连线 */}
            {stages.map((stage) => (
              <g key={`edges-${stage.id}`}>{stage.edges.map((edge) => renderEdge(edge, stage.id))}</g>
            ))}
          </svg>

          {/* 空状态 */}
          {stages.length === 0 && (
            <div className="flex items-center justify-center absolute inset-0" style={{ color: 'var(--text-tertiary)', fontSize: 14 }}>
              点击 "+ 阶段" 开始创建工作流
            </div>
          )}

          {/* 阶段列 */}
          {stages.map((stage, stageIndex) => {
            const isCollapsed = collapsedStages.has(stage.id);
            return (
              <React.Fragment key={stage.id}>
                <div
                  style={{
                    position: 'absolute',
                    top: 20,
                    left: stageIndex * 500 + 20,
                    width: isCollapsed ? 56 : 460,
                    minHeight: 100,
                    borderRadius: 8,
                    background: 'var(--bg-secondary)',
                    border: 'none',
                    transition: isCollapsed ? 'width 0.2s ease' : 'none',
                    overflow: 'visible',
                  }}
                >
                  {/* 阶段标题栏 — 折叠按钮集成在内 */}
                  <div
                    className="flex items-center justify-between shrink-0 rounded-t-lg"
                    style={{
                      padding: '6px 10px',
                      background: 'var(--bg-tertiary)',
                      borderBottom: '1px solid var(--border)',
                    }}
                  >
                    <div className="flex items-center gap-2">
                      {/* 序号块 — 始终正向显示 */}
                      <span className="inline-flex items-center justify-center rounded text-[10px] font-bold" style={{ width: 22, height: 22, background: 'var(--accent-light)', color: 'var(--accent)' }}>
                        {stage.order + 1}
                      </span>
                      {!isCollapsed && (
                        <input
                          value={stage.name}
                          onChange={(e) => handleRenameStage(stage.id, e.target.value)}
                          className="text-xs font-semibold outline-none"
                          style={{ background: 'transparent', color: 'var(--text-primary)', border: 'none', width: 100 }}
                        />
                      )}
                      {isCollapsed && (
                        <span className="text-[10px] font-semibold" style={{ color: 'var(--text-primary)', writingMode: 'vertical-rl', letterSpacing: 1 }}>
                          {stage.name}
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={(e) => { e.stopPropagation(); toggleCollapseStage(stage.id); }}
                        className="pd-btn p-1 rounded text-[10px]"
                        style={{ color: 'var(--text-tertiary)', background: 'transparent' }}
                        title={isCollapsed ? '展开阶段' : '折叠阶段'}
                      >
                        {isCollapsed ? '\u25B6' : '\u25C0'}
                      </button>
                      {!isCollapsed && (
                        <>
                          <button
                            onClick={() => handleAddNode('agent', stage.id)}
                            className="pd-btn px-1.5 py-0.5 rounded text-[10px]"
                            style={{ border: '1px solid var(--border)', background: 'var(--bg-secondary)', color: 'var(--text-tertiary)' }}
                          >+ 节点</button>
                          <button
                            onClick={() => handleDeleteStage(stage.id)}
                            className="pd-btn p-0.5 rounded text-[10px]"
                            style={{ color: 'var(--status-danger)', background: 'transparent' }}
                          >x</button>
                        </>
                      )}
                    </div>
                  </div>

                  {/* 阶段画布内容区 — 折叠时隐藏 */}
                  {!isCollapsed && (
                    <>
                      <div
                        className="relative"
                        style={{ flex: 1, minHeight: 250, padding: 12 }}
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
                              position: {
                                x: (e.clientX - rect.left - pan.x) / scale - 60,
                                y: (e.clientY - rect.top - pan.y) / scale - 20,
                              },
                            };
                            setStages(stages.map((s) =>
                              s.id === stage.id ? { ...s, nodes: [...s.nodes, newNode] } : s
                            ));
                          }
                        }}
                      >
                        {/* 渲染节点 */}
                        {stage.nodes.map((node) => renderNode(node, stage.id))}
                      </div>

                      {/* Gate 区域 */}
                      <div
                        className="mx-2 mb-2 p-2 rounded-lg cursor-pointer"
                        style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)' }}
                        onClick={() => handleUpdateGate(stage.id, {})}
                      >
                        <div className="flex items-center justify-between mb-1">
                          <span className="text-[10px] font-semibold flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
                            门控 Gate
                          </span>
                          <span className="text-[10px] flex items-center gap-1" style={{ color: 'var(--status-success)' }}>
                            <span className="w-1.5 h-1.5 rounded-full inline-block" style={{ background: 'var(--status-success)' }} />
                            {stage.nodes.length}/{stage.nodes.length} 就绪
                          </span>
                        </div>
                        <div className="flex items-center gap-3">
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>策略: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{stage.gate.strategy}</span></span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>合并: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{stage.gate.mergeStrategy}</span></span>
                        </div>
                      </div>
                    </>
                  )}

                  {/* 折叠时显示节点计数 */}
                  {isCollapsed && (
                    <div className="flex flex-col items-center justify-center gap-1 py-3" style={{ color: 'var(--text-tertiary)', fontSize: 10 }}>
                      <span>{stage.nodes.length} 节点</span>
                      <span>{stage.edges.length} 连线</span>
                    </div>
                  )}
                </div>

                {/* 阶段间虚线分隔线 */}
                {stageIndex < stages.length - 1 && (
                  <div
                    style={{
                      position: 'absolute',
                      top: 40,
                      left: (stageIndex + 1) * 500 - 10,
                      width: 1,
                      height: 'calc(100% - 60px)',
                      minHeight: 200,
                      borderLeft: '1px dashed var(--border)',
                    }}
                  />
                )}
              </React.Fragment>
            );
          })}
        </div>
      </div>

      {/* 条件编辑弹窗 */}
      {conditionInput && (
        <div className="fixed inset-0 flex items-center justify-center z-[1000]" style={{ background: 'var(--bg-overlay)' }}>
          <div className="rounded-xl p-6 w-[90%] max-w-[400px]" style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}>
            <h3 className="text-sm font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>编辑条件</h3>
            <div className="mb-3">
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>条件表达式</label>
              <input
                id="condition-expr"
                placeholder="例如: == approve 或 contains 紧急"
                className="w-full px-3 py-2 rounded-lg text-xs outline-none"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
              />
            </div>
            <div className="mb-4">
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>条件标签（显示在边上）</label>
              <input
                id="condition-label"
                placeholder="例如: 审批通过 / 紧急"
                className="w-full px-3 py-2 rounded-lg text-xs outline-none"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
              />
            </div>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => setConditionInput(null)}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
              >取消</button>
              <button
                onClick={() => {
                  const expr = (document.getElementById('condition-expr') as HTMLInputElement)?.value || '';
                  const label = (document.getElementById('condition-label') as HTMLInputElement)?.value || '';
                  handleConfirmCondition(expr, label);
                }}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ background: 'var(--accent)', color: '#fff', border: 'none' }}
              >确认</button>
            </div>
          </div>
        </div>
      )}

      {/* 节点配置面板 */}
      {selectedNodeId && selectedStageId && (
        <div className="fixed right-0 top-0 bottom-0 w-[360px] z-[100] overflow-auto p-5" style={{ background: 'var(--bg-secondary)', borderLeft: '1px solid var(--border)' }}>
          <WorkflowNodeConfig
            node={stages.find((s) => s.id === selectedStageId)?.nodes.find((n) => n.id === selectedNodeId)!}
            onUpdate={(updates) => handleUpdateNode(selectedNodeId, updates)}
            onClose={() => { setSelectedNodeId(null); setSelectedStageId(null); }}
            onOpenSubflow={(definitionId) => {
              const def = definitions.find(d => d.id === definitionId);
              if (def) {
                setSelectedNodeId(null);
                setSelectedStageId(null);
                if (def.stages && def.stages.length > 0) {
                  setStages(def.stages);
                }
              }
            }}
          />
        </div>
      )}
    </div>
  );
};
