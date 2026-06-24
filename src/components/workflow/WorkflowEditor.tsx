/**
 * WorkflowEditor — 工作流可视化编辑器（Structured Canvas）
 *
 * 自由画布 + 阶段列布局，支持：
 * - 画布拖拽平移（左键空白区/中键拖拽）+ 鼠标滚轮缩放（以光标为中心）
 * - 节点拖拽移动（带视觉反馈：阴影+缩放+吸附辅助线）
 * - 节点连线（从输出锚点到输入锚点，拖拽时实时预览线条）
 * - 阶段折叠/展开
 * - CSS变量主题支持
 */

import React, { useState, useEffect, useCallback, useRef } from 'react';
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
  { type: 'agent', label: 'Agent 任务', icon: '\u{1F916}', color: '#58a6ff' },
  { type: 'api', label: 'API 调用', icon: '\u{1F517}', color: '#a371f7' },
  { type: 'transform', label: '代码转换', icon: '\u{26A1}', color: '#d29922' },
  { type: 'interact', label: '人工交互', icon: '\u{1F464}', color: '#f85149' },
  { type: 'plugin', label: '插件命令', icon: '\u{1F9E9}', color: '#3fb950' },
  { type: 'subflow', label: '子工作流', icon: '\u{1F4E6}', color: '#79c0ff' },
];

/** 连线拖拽时的实时预览状态 */
interface ConnectingPreview {
  source: string;
  stageId: string;
  mouseCanvasX: number;
  mouseCanvasY: number;
}

export const WorkflowEditor: React.FC<Props> = ({ definitionId, onClose, onNameChange }) => {
  const { definitions, updateDefinition, loadDefinitions } = useWorkflowStore();
  const def = definitions.find((d) => d.id === definitionId);

  const [stages, setStages] = useState<Stage[]>(def?.stages || []);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedStageId, setSelectedStageId] = useState<string | null>(null);
  const [name, setName] = useState(def?.name || '');
  const [description, setDescription] = useState(def?.description || '');
  const [connecting, setConnecting] = useState<ConnectingPreview | null>(null);
  const [conditionInput, setConditionInput] = useState<{ source: string; target: string; stageId: string } | null>(null);
  const [collapsedStages, setCollapsedStages] = useState<Set<string>>(new Set());
  const [hoveredNodeId, setHoveredNodeId] = useState<string | null>(null);
  const [dragOverStageId, setDragOverStageId] = useState<string | null>(null);
  const [showGrid, setShowGrid] = useState(false);

  // 画布平移和缩放状态
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [scale, setScale] = useState(1);
  const [isPanning, setIsPanning] = useState(false);
  const panStartRef = useRef<{ x: number; y: number; panX: number; panY: number } | null>(null);
  const panThresholdRef = useRef<{ startX: number; startY: number; triggered: boolean } | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const dragOverCanvasRef = useRef<boolean>(false);

  // 节点拖拽状态
  const [draggingNode, setDraggingNode] = useState<{
    nodeId: string;
    stageId: string;
    offsetX: number;
    offsetY: number;
    started: boolean;
    startX: number;
    startY: number;
  } | null>(null);

  // 用于连线预览的SVG ref
  const connectingLineRef = useRef<SVGPathElement | null>(null);

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

  // ── 连线操作（带实时预览） ──
  const handleStartConnect = (e: React.MouseEvent, nodeId: string, stageId: string) => {
    e.stopPropagation();
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const mouseCanvasX = (e.clientX - rect.left - pan.x) / scale;
    const mouseCanvasY = (e.clientY - rect.top - pan.y) / scale;
    setConnecting({ source: nodeId, stageId, mouseCanvasX, mouseCanvasY });
  };

  const handleUpdateConnectingPos = useCallback((e: MouseEvent) => {
    if (!connecting) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    setConnecting(prev => prev ? {
      ...prev,
      mouseCanvasX: (e.clientX - rect.left - pan.x) / scale,
      mouseCanvasY: (e.clientY - rect.top - pan.y) / scale,
    } : null);
  }, [connecting, pan, scale]);

  const handleEndConnect = useCallback((nodeId: string, stageId: string) => {
    if (!connecting || connecting.source === nodeId) {
      setConnecting(null);
      return;
    }

    const newEdge: WorkflowEdge = {
      id: generateEdgeId(connecting.source, nodeId),
      source: connecting.source,
      target: nodeId,
    };

    if (connecting.stageId !== stageId) {
      setConditionInput({ source: connecting.source, target: nodeId, stageId });
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
  }, [connecting, stages]);

  const handleCancelConnect = useCallback(() => {
    setConnecting(null);
  }, []);

  // 监听连线拖拽期间的鼠标移动和释放
  useEffect(() => {
    if (!connecting) return;
    const onMouseMove = (e: MouseEvent) => {
      handleUpdateConnectingPos(e);
    };
    const onMouseUp = () => {
      setConnecting(null);
    };
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
    };
  }, [connecting, handleUpdateConnectingPos]);

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

  // ══════════════════════════════════════════
  // 画布拖拽平移（含 3px 防误触阈值 + 中键支持）
  // ══════════════════════════════════════════
  const PAN_THRESHOLD = 3; // 最小像素移动量才触发平移

  const handleCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    // 中键直接开始平移，无阈值
    if (e.button === 1) {
      e.preventDefault();
      setIsPanning(true);
      panStartRef.current = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
      panThresholdRef.current = { startX: e.clientX, startY: e.clientY, triggered: true };
      return;
    }
    // 仅左键
    if (e.button !== 0) return;
    // 不在交互元素上时才开始平移
    if ((e.target as HTMLElement).closest('button, input, [data-node], [data-anchor], [data-gate]')) return;
    e.preventDefault();
    setIsPanning(true);
    panStartRef.current = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
    panThresholdRef.current = { startX: e.clientX, startY: e.clientY, triggered: false };
  }, [pan.x, pan.y]);

  useEffect(() => {
    if (!isPanning) return;
    const handleMouseMove = (e: MouseEvent) => {
      if (!panStartRef.current || !panThresholdRef.current) return;
      const th = panThresholdRef.current;
      // 如果尚未触发，检查阈值
      if (!th.triggered) {
        const dx = Math.abs(e.clientX - th.startX);
        const dy = Math.abs(e.clientY - th.startY);
        if (dx < PAN_THRESHOLD && dy < PAN_THRESHOLD) return;
        th.triggered = true;
      }
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
      panThresholdRef.current = null;
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isPanning]);

  // ══════════════════════════════════════════
  // 鼠标滚轮缩放（以光标位置为中心）
  // ══════════════════════════════════════════
  const handleWheel = useCallback((e: React.WheelEvent) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;

    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      // 以鼠标位置为中心缩放
      const mouseX = e.clientX - rect.left;
      const mouseY = e.clientY - rect.top;

      setScale((prevScale) => {
        const delta = -e.deltaY * 0.002;
        const newScale = Math.min(Math.max(prevScale + delta, 0.1), 4);
        const ratio = newScale / prevScale;

        // 调整平移使鼠标位置不变
        setPan((prevPan) => ({
          x: mouseX - ratio * (mouseX - prevPan.x),
          y: mouseY - ratio * (mouseY - prevPan.y),
        }));

        return newScale;
      });
    } else {
      // 非Ctrl时：水平滚轮=水平平移，垂直滚轮=垂直平移
      setPan((prev) => ({
        x: prev.x - e.deltaX,
        y: prev.y - e.deltaY,
      }));
    }
  }, []);

  // ══════════════════════════════════════════
  // 节点拖拽（含 3px 防误触阈值 + 视觉反馈）
  // ══════════════════════════════════════════
  const NODE_DRAG_THRESHOLD = 3;

  const handleNodeMouseDown = useCallback((e: React.MouseEvent, nodeId: string, stageId: string) => {
    // 锚点点击不触发拖拽
    if ((e.target as HTMLElement).closest('[data-anchor]')) return;
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
      started: false,
      startX: e.clientX,
      startY: e.clientY,
    });
    // 点击即选中
    setSelectedNodeId(nodeId);
    setSelectedStageId(stageId);
  }, [stages, pan.x, pan.y, scale]);

  useEffect(() => {
    if (!draggingNode) return;
    const handleMouseMove = (e: MouseEvent) => {
      const d = draggingNode;
      // 检查阈值
      if (!d.started) {
        const dx = Math.abs(e.clientX - d.startX);
        const dy = Math.abs(e.clientY - d.startY);
        if (dx < NODE_DRAG_THRESHOLD && dy < NODE_DRAG_THRESHOLD) return;
        setDraggingNode(prev => prev ? { ...prev, started: true } : null);
      }
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const newAbsX = e.clientX - d.offsetX;
      const newAbsY = e.clientY - d.offsetY;
      const canvasX = newAbsX - rect.left;
      const canvasY = newAbsY - rect.top;
      const nodeX = canvasX / scale - pan.x;
      const nodeY = canvasY / scale - pan.y;
      setStages((prev) =>
        prev.map((s) =>
          s.id === d.stageId
            ? {
                ...s,
                nodes: s.nodes.map((n) =>
                  n.id === d.nodeId
                    ? { ...n, position: { x: Math.round(nodeX), y: Math.round(nodeY) } }
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

  // ── 坐标转换工具 ──
  const clientToCanvas = useCallback((clientX: number, clientY: number) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return { x: 0, y: 0 };
    return {
      x: (clientX - rect.left - pan.x) / scale,
      y: (clientY - rect.top - pan.y) / scale,
    };
  }, [pan, scale]);

  // 获取节点输出锚点的画布坐标（右侧中心）
  const getNodeOutputAnchor = useCallback((nodeId: string, stageId: string): { x: number; y: number } | null => {
    const stage = stages.find(s => s.id === stageId);
    const node = stage?.nodes.find(n => n.id === nodeId);
    if (!node?.position) return null;
    const pos = node.position;
    return { x: pos.x + 160, y: pos.y + 20 };
  }, [stages]);

  // ══════════════════════════════════════════
  // 渲染：节点
  // ══════════════════════════════════════════
  const renderNode = (node: WorkflowNode, stageId: string) => {
    const meta = getNodeTypeMeta(node.type);
    const pos = node.position as { x: number; y: number } | undefined;
    const isSelected = selectedNodeId === node.id;
    const isDraggingThis = draggingNode?.nodeId === node.id && draggingNode?.started;
    const isHovered = hoveredNodeId === node.id;
    const isConnectTarget = connecting?.source !== node.id;

    return (
      <div
        key={node.id}
        data-node
        onClick={(e) => {
          e.stopPropagation();
          setSelectedNodeId(node.id);
          setSelectedStageId(stageId);
        }}
        onMouseDown={(e) => handleNodeMouseDown(e, node.id, stageId)}
        onMouseEnter={() => setHoveredNodeId(node.id)}
        onMouseLeave={() => setHoveredNodeId(null)}
        className="cursor-grab active:cursor-grabbing"
        style={{
          position: 'absolute',
          left: pos?.x ?? 20,
          top: pos?.y ?? 20,
          width: 160,
          padding: '8px 10px',
          borderRadius: 8,
          border: isSelected
            ? '2px solid var(--accent)'
            : isHovered
              ? '1px solid var(--accent)'
              : '1px solid var(--border)',
          background: isDraggingThis
            ? 'var(--bg-tertiary)'
            : 'var(--bg-tertiary)',
          boxShadow: isDraggingThis
            ? '0 8px 24px rgba(0,0,0,0.4), 0 0 0 1px var(--accent)'
            : isSelected
              ? '0 0 0 2px var(--accent-light), var(--shadow-md)'
              : isHovered
                ? 'var(--shadow-md)'
                : 'var(--shadow-sm)',
          zIndex: isDraggingThis ? 20 : 10,
          userSelect: 'none',
          transform: isDraggingThis ? 'scale(1.03)' : 'scale(1)',
          opacity: isDraggingThis ? 0.92 : 1,
          transition: isDraggingThis ? 'none' : 'box-shadow 0.15s ease, border-color 0.15s ease, transform 0.15s ease',
        }}
      >
        <div className="flex items-center gap-1.5 mb-1">
          <div
            className="shrink-0"
            style={{
              width: 24, height: 24, borderRadius: 5,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 12,
              background: `${meta.color}22`,
              color: meta.color,
            }}
          >
            {meta.icon}
          </div>
          <span className="text-xs font-semibold truncate" style={{ color: 'var(--text-primary)' }}>{node.label}</span>
        </div>
        <div className="flex gap-1 flex-wrap ml-[30px]">
          {node.delayMs && (
            <span className="text-[9px] px-1 py-0.5 rounded" style={{ background: 'var(--bg-secondary)', color: '#d29922', border: '1px solid #d2992244' }}>
              延迟 {node.delayMs}ms
            </span>
          )}
          {node.timeoutMs && (
            <span className="text-[9px] px-1 py-0.5 rounded" style={{ background: 'var(--bg-secondary)', color: '#a371f7', border: '1px solid #8957e544' }}>
              超时 {node.timeoutMs / 1000}s
            </span>
          )}
        </div>

        {/* 输入锚点 — hover时发光 */}
        <div
          data-anchor="input"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            if (connecting) {
              handleEndConnect(node.id, stageId);
            }
          }}
          className="absolute rounded-full transition-all duration-150"
          style={{
            left: -5, top: '50%', marginTop: -5,
            width: connecting ? 12 : 10, height: connecting ? 12 : 10,
            background: (connecting && isConnectTarget) ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-primary)',
            cursor: connecting ? 'cell' : 'crosshair',
            boxShadow: (connecting && isConnectTarget) ? '0 0 8px var(--accent-light)' : 'none',
          }}
        />
        {/* 输出锚点 — hover时发光 */}
        <div
          data-anchor="output"
          onMouseDown={(e) => {
            e.stopPropagation();
            handleStartConnect(e, node.id, stageId);
          }}
          className="absolute rounded-full transition-all duration-150"
          style={{
            right: -5, top: '50%', marginTop: -5,
            width: isHovered || connecting ? 12 : 10,
            height: isHovered || connecting ? 12 : 10,
            background: connecting ? 'var(--accent)' : isHovered ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-primary)',
            cursor: 'crosshair',
            boxShadow: (isHovered || connecting) ? '0 0 8px var(--accent-light)' : 'none',
          }}
        />
        {/* 删除按钮 — 仅hover时显示 */}
        <div
          onClick={(e) => { e.stopPropagation(); handleDeleteNode(node.id); }}
          className="absolute flex items-center justify-center rounded-full cursor-pointer"
          style={{
            top: -6, right: -6, width: 16, height: 16,
            background: 'var(--status-danger)', color: '#fff', fontSize: 10,
            opacity: isHovered ? 0.9 : 0,
            transition: 'opacity 0.15s ease',
          }}
        >x</div>
      </div>
    );
  };

  // ══════════════════════════════════════════
  // 渲染：连线
  // ══════════════════════════════════════════
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
        {/* 不可见的粗命中区域，便于点击删除 */}
        <path
          d={`M ${sp.x + 160} ${sp.y + 20} C ${sp.x + 190} ${sp.y + 20}, ${tp.x - 10} ${tp.y + 20}, ${tp.x} ${tp.y + 20}`}
          stroke="transparent"
          strokeWidth={12}
          fill="none"
          style={{ cursor: 'pointer', pointerEvents: 'stroke' }}
          onClick={() => handleDeleteEdge(edge.id)}
        />
        {/* 可见连线 */}
        <path
          d={`M ${sp.x + 160} ${sp.y + 20} C ${sp.x + 190} ${sp.y + 20}, ${tp.x - 10} ${tp.y + 20}, ${tp.x} ${tp.y + 20}`}
          stroke={edge.condition ? '#d29922' : 'var(--accent)'}
          strokeWidth={2}
          fill="none"
          markerEnd="url(#arrowhead)"
          style={{ pointerEvents: 'none' }}
        />
        {edge.label && (
          <g style={{ pointerEvents: 'auto', cursor: 'pointer' }} onClick={() => handleDeleteEdge(edge.id)}>
            <rect
              x={(sp.x + 160 + tp.x) / 2 - 20}
              y={(sp.y + tp.y) / 2 - 18}
              width={40}
              height={16}
              rx={3}
              fill="var(--bg-primary)"
              stroke="var(--border)"
              strokeWidth={0.5}
            />
            <text
              x={(sp.x + 160 + tp.x) / 2}
              y={(sp.y + tp.y) / 2 - 8}
              fill="var(--text-secondary)"
              fontSize={9}
              textAnchor="middle"
            >
              {edge.label}
            </text>
          </g>
        )}
      </g>
    );
  };

  // ══════════════════════════════════════════
  // 渲染：连线预览（拖拽中的临时线条）
  // ══════════════════════════════════════════
  const renderConnectingPreview = () => {
    if (!connecting) return null;
    const anchorPos = getNodeOutputAnchor(connecting.source, connecting.stageId);
    if (!anchorPos) return null;
    const mx = connecting.mouseCanvasX;
    const my = connecting.mouseCanvasY;
    const d = `M ${anchorPos.x} ${anchorPos.y} C ${anchorPos.x + 30} ${anchorPos.y}, ${mx - 30} ${my}, ${mx} ${my}`;
    return (
      <path
        d={d}
        stroke="var(--accent)"
        strokeWidth={2}
        strokeDasharray="6 3"
        fill="none"
        opacity={0.7}
        style={{ pointerEvents: 'none' }}
      />
    );
  };

  // ══════════════════════════════════════════
  // JSX
  // ══════════════════════════════════════════
  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* ── 工具栏 ── */}
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

        <button
          onClick={() => setShowGrid((v) => !v)}
          className="pd-btn px-2 py-1 text-[11px] rounded flex items-center gap-1"
          style={{
            border: showGrid ? '1px solid var(--accent)' : '1px solid var(--border)',
            background: showGrid ? 'var(--accent-light)' : 'var(--bg-tertiary)',
            color: showGrid ? 'var(--accent)' : 'var(--text-secondary)',
          }}
          title={showGrid ? '隐藏辅助网格' : '显示辅助网格'}
        >
          □
        </button>

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

      {/* ── 画布区域 ── */}
      <div
        ref={canvasRef}
        className="flex-1 overflow-hidden relative"
        style={{
          cursor: connecting
            ? 'crosshair'
            : isPanning
              ? (panThresholdRef.current?.triggered ? 'grabbing' : 'default')
              : draggingNode?.started
                ? 'default'
                : dragOverStageId === null ? (dragOverCanvasRef.current ? 'not-allowed' : 'grab') : 'grab',
        }}
        onMouseDown={handleCanvasMouseDown}
        onWheel={handleWheel}
        onContextMenu={(e) => e.preventDefault()}
        onDragOver={(e) => {
          // 标记正在拖入画布（可能在空白区）
          dragOverCanvasRef.current = true;
        }}
        onDragLeave={(e) => {
          // 离开画布根区域时清除标记
          if (e.currentTarget === e.target || !e.currentTarget.contains(e.relatedTarget as Node)) {
            dragOverCanvasRef.current = false;
            setDragOverStageId(null);
          }
        }}
      >
        {/* 背景辅助网格 */}
        {showGrid && (
          <div
            className="absolute inset-0 z-0 pointer-events-none"
            style={{
              backgroundImage: `
                linear-gradient(var(--border) 1px, transparent 1px),
                linear-gradient(90deg, var(--border) 1px, transparent 1px)
              `,
              backgroundSize: `${20 * scale}px ${20 * scale}px`,
              backgroundPosition: `${pan.x % (20 * scale)}px ${pan.y % (20 * scale)}px`,
              opacity: 0.35,
            }}
          />
        )}

        {/* 缩放指示器 */}
        <div
          className="absolute bottom-3 right-3 z-30 flex items-center gap-1 rounded-lg px-1.5 py-1"
          style={{ background: 'var(--bg-secondary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)' }}
        >
          <button
            onClick={() => {
              const rect = canvasRef.current?.getBoundingClientRect();
              if (!rect) return;
              const cx = rect.width / 2;
              const cy = rect.height / 2;
              setScale((prev) => {
                const next = Math.min(prev + 0.15, 4);
                const ratio = next / prev;
                setPan(p => ({ x: cx - ratio * (cx - p.x), y: cy - ratio * (cy - p.y) }));
                return next;
              });
            }}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded text-xs font-bold"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: 'none' }}
          >+</button>
          <span className="text-[10px] w-[36px] text-center" style={{ color: 'var(--text-secondary)' }}>
            {Math.round(scale * 100)}%
          </span>
          <button
            onClick={() => {
              const rect = canvasRef.current?.getBoundingClientRect();
              if (!rect) return;
              const cx = rect.width / 2;
              const cy = rect.height / 2;
              setScale((prev) => {
                const next = Math.max(prev - 0.15, 0.1);
                const ratio = next / prev;
                setPan(p => ({ x: cx - ratio * (cx - p.x), y: cy - ratio * (cy - p.y) }));
                return next;
              });
            }}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded text-xs font-bold"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: 'none' }}
          >-</button>
          <div className="w-px h-4" style={{ background: 'var(--border)' }} />
          <button
            onClick={() => { setPan({ x: 0, y: 0 }); setScale(1); }}
            className="pd-btn px-1.5 h-6 flex items-center justify-center rounded text-[10px]"
            style={{ background: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', border: 'none' }}
            title="重置视图"
          >
            重置
          </button>
        </div>

        {/* 连线提示 */}
        {connecting && (
          <div
            className="absolute top-3 left-1/2 -translate-x-1/2 z-30 px-3 py-1.5 rounded-lg text-[10px] flex items-center gap-2"
            style={{ background: 'var(--bg-secondary)', border: '1px solid var(--accent)', color: 'var(--accent)', boxShadow: 'var(--shadow-md)' }}
          >
            <span className="w-1.5 h-1.5 rounded-full inline-block animate-pulse" style={{ background: 'var(--accent)' }} />
            正在连线 — 点击目标节点的输入锚点完成连接，按 Esc 取消
          </div>
        )}

        {/* 拖拽到画布空白区——不可放置提示 */}
        {dragOverCanvasRef.current && dragOverStageId === null && (
          <div
            className="absolute inset-0 z-20 flex items-center justify-center pointer-events-none"
            style={{ background: 'rgba(248, 81, 73, 0.05)' }}
          >
            <div
              className="px-4 py-2 rounded-lg flex items-center gap-2"
              style={{
                background: 'var(--bg-secondary)',
                border: '1px solid var(--status-danger)',
                color: 'var(--status-danger)',
                boxShadow: 'var(--shadow-md)',
                fontSize: 11,
              }}
            >
              <span style={{ fontSize: 14 }}>✗</span>
              <span>请拖入阶段内容区</span>
            </div>
          </div>
        )}

        {/* 内部可变换画布 */}
        <div
          style={{
            transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale})`,
            transformOrigin: '0 0',
            width: 4000,
            height: 3000,
            position: 'relative',
          }}
        >
          {/* SVG 层：箭头标记 + 连线 + 预览线 */}
          <svg
            style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: '100%', pointerEvents: 'none' }}
          >
            <defs>
              <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
                <polygon points="0 0, 8 3, 0 6" fill="var(--accent)" />
              </marker>
            </defs>
            {stages.map((stage) => (
              <g key={`edges-${stage.id}`}>
                {stage.edges.map((edge) => renderEdge(edge, stage.id))}
              </g>
            ))}
            {/* 连线拖拽预览 */}
            {connecting && renderConnectingPreview()}
          </svg>

          {/* 空状态 */}
          {stages.length === 0 && (
            <div className="flex items-center justify-center absolute inset-0" style={{ color: 'var(--text-tertiary)', fontSize: 14 }}>
              点击 "+ 阶段" 开始创建工作流
            </div>
          )}

          {/* ── 阶段列 ── */}
          {stages.map((stage, stageIndex) => {
            const isCollapsed = collapsedStages.has(stage.id);
            return (
              <React.Fragment key={stage.id}>
                <div
                  data-stage={stage.id}
                  style={{
                    position: 'absolute',
                    top: 20,
                    left: stageIndex * 500 + 20,
                    width: isCollapsed ? 56 : 460,
                    minHeight: 100,
                    borderRadius: 8,
                    background: 'var(--bg-secondary)',
                    border: 'none',
                    transition: isCollapsed ? 'width 0.25s cubic-bezier(0.4,0,0.2,1)' : 'none',
                    overflow: 'visible',
                  }}
                >
                  {/* 阶段标题栏 */}
                  <div
                    className="flex items-center justify-between shrink-0"
                    style={{
                      padding: '6px 10px',
                      background: 'var(--bg-tertiary)',
                      borderBottom: '1px solid var(--border)',
                    }}
                  >
                    <div className="flex items-center gap-2">
                      <span
                        className="inline-flex items-center justify-center rounded text-[10px] font-bold shrink-0"
                        style={{ width: 22, height: 22, background: 'var(--accent-light)', color: 'var(--accent)' }}
                      >
                        {stage.order + 1}
                      </span>
                      {!isCollapsed ? (
                        <input
                          value={stage.name}
                          onChange={(e) => handleRenameStage(stage.id, e.target.value)}
                          className="text-xs font-semibold outline-none"
                          style={{ background: 'transparent', color: 'var(--text-primary)', border: 'none', width: 100 }}
                        />
                      ) : (
                        <span
                          className="text-[10px] font-semibold"
                          style={{ color: 'var(--text-primary)', writingMode: 'vertical-rl', letterSpacing: 1 }}
                        >
                          {stage.name}
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={(e) => { e.stopPropagation(); toggleCollapseStage(stage.id); }}
                        className="pd-btn p-1 rounded text-[10px] transition-colors duration-150"
                        style={{ color: 'var(--text-tertiary)', background: 'transparent' }}
                        title={isCollapsed ? '展开阶段' : '折叠阶段'}
                      >
                        {isCollapsed ? '\u25B6' : '\u25C0'}
                      </button>
                      {!isCollapsed && (
                        <>
                          <button
                            onClick={() => handleAddNode('agent', stage.id)}
                            className="pd-btn px-1.5 py-0.5 rounded text-[10px] transition-colors duration-150"
                            style={{ border: '1px solid var(--border)', background: 'var(--bg-secondary)', color: 'var(--text-tertiary)' }}
                          >+ 节点</button>
                          <button
                            onClick={() => handleDeleteStage(stage.id)}
                            className="pd-btn p-0.5 rounded text-[10px] transition-colors duration-150"
                            style={{ color: 'var(--status-danger)', background: 'transparent' }}
                          >x</button>
                        </>
                      )}
                    </div>
                  </div>

                  {/* 阶段内容区 */}
                  {!isCollapsed && (
                    <>
                      <div
                        className="relative"
                        style={{
                          flex: 1,
                          minHeight: 250,
                          padding: 12,
                          border: dragOverStageId === stage.id ? '2px dashed var(--accent)' : '2px dashed transparent',
                          borderRadius: 6,
                          background: dragOverStageId === stage.id ? 'var(--accent-light)' : 'transparent',
                          transition: 'border-color 0.15s ease, background 0.15s ease',
                        }}
                        onDragOver={(e) => {
                          e.preventDefault();
                          setDragOverStageId(stage.id);
                        }}
                        onDragLeave={() => {
                          if (dragOverStageId === stage.id) setDragOverStageId(null);
                        }}
                        onDrop={(e) => {
                          e.preventDefault();
                          setDragOverStageId(null);
                          const type = e.dataTransfer.getData('text/plain') as WorkflowNodeType;
                          if (type) {
                            const rect = e.currentTarget.getBoundingClientRect();
                            const meta = getNodeTypeMeta(type);
                            const newNode: WorkflowNode = {
                              id: generateId(),
                              type,
                              label: meta.label,
                              position: {
                                x: Math.round((e.clientX - rect.left - pan.x) / scale - 60),
                                y: Math.round((e.clientY - rect.top - pan.y) / scale - 20),
                              },
                            };
                            setStages(stages.map((s) =>
                              s.id === stage.id ? { ...s, nodes: [...s.nodes, newNode] } : s
                            ));
                          }
                        }}
                      >
                        {stage.nodes.map((node) => renderNode(node, stage.id))}
                      </div>

                      {/* Gate 区域 */}
                      <div
                        data-gate
                        className="mx-2 mb-2 p-2 rounded-lg cursor-pointer transition-colors duration-150"
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
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            策略: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{stage.gate.strategy}</span>
                          </span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            合并: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{stage.gate.mergeStrategy}</span>
                          </span>
                        </div>
                      </div>
                    </>
                  )}

                  {/* 折叠摘要 */}
                  {isCollapsed && (
                    <div className="flex flex-col items-center justify-center gap-1 py-3" style={{ color: 'var(--text-tertiary)', fontSize: 10 }}>
                      <span>{stage.nodes.length} 节点</span>
                      <span>{stage.edges.length} 连线</span>
                    </div>
                  )}
                </div>


              </React.Fragment>
            );
          })}
        </div>
      </div>

      {/* ── Esc取消连线 ── */}
      {connecting && (
        <div
          tabIndex={-1}
          ref={(el) => { if (el) el.focus(); }}
          onKeyDown={(e) => { if (e.key === 'Escape') handleCancelConnect(); }}
          style={{ position: 'absolute', opacity: 0, pointerEvents: 'none' }}
        />
      )}

      {/* ── 条件编辑弹窗 ── */}
      {conditionInput && (
        <div className="fixed inset-0 flex items-center justify-center z-[1000]" style={{ background: 'var(--bg-overlay)' }}>
          <div
            className="rounded-xl p-6 w-[90%] max-w-[400px]"
            style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>编辑条件</h3>
            <div className="mb-3">
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>条件表达式</label>
              <input
                id="condition-expr"
                placeholder="例如: == approve 或 contains 紧急"
                className="w-full px-3 py-2 rounded-lg text-xs outline-none"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                autoFocus
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

      {/* ── 节点配置面板 ── */}
      {selectedNodeId && selectedStageId && (
        <div
          className="fixed right-0 top-0 bottom-0 w-[360px] z-[100] overflow-auto p-5"
          style={{ background: 'var(--bg-secondary)', borderLeft: '1px solid var(--border)' }}
        >
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
