/**
 * WorkflowEditor — 工作流可视化编辑器（Structured Canvas）
 *
 * 自由画布 + 阶段列布局，支持：
 * - 画布拖拽平移（左键空白区/中键拖拽）+ 鼠标滚轮缩放（以光标为中心）
 * - 节点拖拽移动（带视觉反馈：阴影+缩放+吸附辅助线）
 * - 节点连线（从输出锚点到输入锚点，拖拽时实时预览线条）
 * - 阶段折叠/展开
 * - 阶段间自动连线（Gate → 下一阶段标题）
 * - 删除二次确认（阶段/节点/连线）
 * - CSS变量主题支持
 */

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { flushSync } from 'react-dom';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { showToast } from '../../utils/toast';
import { useWorkflowStore } from '../../stores/workflowStore';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { getNodeTypeMeta, generateId, generateEdgeId, generateStageId, autoAssignStage, createWorkflowNode, clampNodePosition, sanitizeMappingReferences, getReachableNodes, validateWorkflowForExecution } from '../../workflow/WorkflowDefinition';
import WorkflowNodeItem from './WorkflowNodeItem';
import { WorkflowNodeConfig } from './WorkflowNodeConfig';
import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType, Stage, GateConfig } from '../../types/workflow';

interface Props {
  definitionId: string;
  onClose: () => void;
  onNameChange?: (name: string) => void;
  onSaveResult?: (success: boolean) => void;
}

// ── 布局常量（画布 & 阶段 & 节点尺寸） ──
const STAGE_W = 480;
const STAGE_COLLAPSED_W = 72;
const STAGE_GAP = 36;
const STAGE_TOP = 20;
const TITLE_H = 36;
const GATE_H = 64;
const CONTENT_H = 500;
const NODE_W = 160;
const NODE_H = 60;
const SNAP_SIZE = 20;
const PAN_THRESHOLD = 3;
const NODE_DRAG_THRESHOLD = 3;
const CANVAS_W = 4000;
const CANVAS_H = 3000;

/** 工具栏可拖拽的节点类型（从 NODE_TYPE_META 派生，排除边界节点） */
const BUILTIN_NODE_TYPES = (['agent', 'api', 'transform', 'interact', 'plugin', 'subflow'] as WorkflowNodeType[])
  .map(type => ({ type, ...getNodeTypeMeta(type) }));

/** 连线拖拽时的实时预览状态 */
interface ConnectingPreview {
  source: string;
  stageId: string;
  mouseCanvasX: number;
  mouseCanvasY: number;
  /** 是否为阶段级连线（source/target 为 stage.id） */
  isStageEdge?: boolean;
}

/** 阶段连线拖拽状态 */
interface StageConnectingPreview {
  sourceStageId: string;
  mouseCanvasX: number;
  mouseCanvasY: number;
}

/** 删除确认弹窗状态 */
interface ConfirmAction {
  type: 'deleteStage' | 'deleteNode' | 'deleteEdge' | 'deleteNodes' | 'deleteStageEdge';
  targetId: string;
  label: string;
}

/** 执行状态枚举 */
type StepRunState = 'idle' | 'running' | 'success' | 'failed';

export const WorkflowEditor: React.FC<Props> = ({ definitionId, onClose, onNameChange, onSaveResult }) => {
  const { definitions, instances, selectedInstanceId, updateDefinition, loadDefinitions } = useWorkflowStore();
  const def = definitions.find((d) => d.id === definitionId);

  const [stages, setStages] = useState<Stage[]>(def?.stages || []);
  const [stageEdges, setStageEdges] = useState<WorkflowEdge[]>(def?.stageEdges || []);
  const [stageConnecting, setStageConnecting] = useState<StageConnectingPreview | null>(null);
  const [hoveredStageEdge, setHoveredStageEdge] = useState<string | null>(null);
  const [hoveredAnchor, setHoveredAnchor] = useState<string | null>(null);
  const [stageOffsets, setStageOffsets] = useState<Record<string, number>>({});
  const [draggingStageId, setDraggingStageId] = useState<string | null>(null);
  const dragStartXRef = useRef<number>(0);
  const dragStartOffsetRef = useRef<number>(0);

  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedStageId, setSelectedStageId] = useState<string | null>(null);

  // ── 多选状态 ──
  const [selectedNodeIds, setSelectedNodeIds] = useState<Set<string>>(new Set());
  const [isBoxSelecting, setIsBoxSelecting] = useState(false);

  const [boxSelectRect, setBoxSelectRect] = useState<{ x1: number; y1: number; x2: number; y2: number; stageId: string } | null>(null);
  const boxSelectStartRef = useRef<{ x: number; y: number; stageId: string } | null>(null);
  const [name, setName] = useState(def?.name || '');
  const [description, setDescription] = useState(def?.description || '');
  const [connecting, setConnecting] = useState<ConnectingPreview | null>(null);
  const [conditionInput, setConditionInput] = useState<{ source: string; target: string; stageId: string } | null>(null);
  const [gateInput, setGateInput] = useState<{ stageId: string } | null>(null);
  const [gateError, setGateError] = useState<string | null>(null);
  const [gateStrategy, setGateStrategy] = useState<string>('all');
  const [showCustomMode, setShowCustomMode] = useState(false);
  const [collapsedStages, setCollapsedStages] = useState<Set<string>>(new Set());
  const [customMode, setCustomMode] = useState<string>('selector');
  const [hoveredNodeId, setHoveredNodeId] = useState<string | null>(null);
  const [hoveredEdgeId, setHoveredEdgeId] = useState<string | null>(null);
  const [dragOverStageId, setDragOverStageId] = useState<string | null>(null);
  const [showGrid, setShowGrid] = useState(false);
  const [edgeContextMenu, setEdgeContextMenu] = useState<{ edgeId: string; x: number; y: number } | null>(null);

  // 删除二次确认
  const [confirmAction, setConfirmAction] = useState<ConfirmAction | null>(null);

  // 执行状态（模拟/预览用，key: nodeId | stageId:stepIndex）
  const [stepStates, setStepStates] = useState<Record<string, StepRunState>>({});
  const [nodeResults, setNodeResults] = useState<Record<string, any>>({});

  // 画布平移和缩放状态
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [scale, setScale] = useState(1);
  const [isPanning, setIsPanning] = useState(false);
  const panStartRef = useRef<{ x: number; y: number; panX: number; panY: number } | null>(null);
  const panThresholdRef = useRef<{ startX: number; startY: number; triggered: boolean } | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);

  
  const dragOverCanvasRef = useRef<boolean>(false);

  // 工具栏节点模拟拖拽状态（替代HTML5 DnD，解决Tauri WebView2不触发dragstart问题）
  const [toolbarDrag, setToolbarDrag] = useState<{
    type: WorkflowNodeType;
    icon: string;
    label: string;
    color: string;
    ghostX: number;
    ghostY: number;
  } | null>(null);
  const toolbarDragRef = useRef<WorkflowNodeType | null>(null);

// Toolbar drag ghost element ref
  const ghostRef = useRef<HTMLDivElement | null>(null);

  // Toolbar drag mouse tracking
  useEffect(() => {
    if (!toolbarDrag) return;
    const handleMove = (e: MouseEvent) => {
      setToolbarDrag(prev => prev ? { ...prev, ghostX: e.clientX, ghostY: e.clientY } : null);
      if (ghostRef.current) {
        ghostRef.current.style.left = e.clientX + 'px';
        ghostRef.current.style.top = e.clientY + 'px';
      }
    };
    const handleUp = (e: MouseEvent) => {
      if (toolbarDrag) {
        // Check if dropped over canvas
        const canvasEl = canvasRef.current;
        if (canvasEl) {
          const rect = canvasEl.getBoundingClientRect();
          if (e.clientX >= rect.left && e.clientX <= rect.right &&
              e.clientY >= rect.top && e.clientY <= rect.bottom) {
            const cx = (e.clientX - rect.left - pan.x) / scale;
            const cy = (e.clientY - rect.top - pan.y) / scale;
            const targetStageId = getStageAtCanvasPos(cx, cy);
            if (targetStageId) {
              // 纯算术计算放置位置
                const rel = canvasToContentPos(cx, cy, targetStageId);
                // 统一创建方法（传入鼠标位置，自动 clamp + snap）
                const newNode = createWorkflowNode(toolbarDrag.type, { x: rel.x, y: rel.y }, undefined, scale);
              setStages(prev => prev.map(s => {
                if (s.id === targetStageId) {
                  return { ...s, nodes: [...s.nodes, newNode] };
                }
                return s;
              }));
              setSelectedNodeId(newNode.id);
              setSelectedStageId(targetStageId);
            }
          }
        }
      }
      // 处理无效放置（画布空白区/非canvas区域）
      if (toolbarDrag) {
        const canvasEl = canvasRef.current;
        let validDrop = false;
        if (canvasEl) {
          const rect = canvasEl.getBoundingClientRect();
          if (e.clientX >= rect.left && e.clientX <= rect.right &&
              e.clientY >= rect.top && e.clientY <= rect.bottom) {
            const cx = (e.clientX - rect.left - pan.x) / scale;
            const cy = (e.clientY - rect.top - pan.y) / scale;
            validDrop = !!getStageAtCanvasPos(cx, cy);
          }
        }
        // ghost shake动画表示无效
        if (!validDrop && ghostRef.current) {
          ghostRef.current.style.transition = 'transform 0.1s ease';
          ghostRef.current.style.transform = 'translate(-50%,-50%) rotate(-5deg)';
          setTimeout(() => { if (ghostRef.current) ghostRef.current.style.transform = 'translate(-50%,-50%) rotate(5deg)'; }, 100);
          setTimeout(() => {
            if (ghostRef.current) {
              ghostRef.current.style.transform = 'translate(-50%,-50%)';
              ghostRef.current.remove();
              ghostRef.current = null;
            }
          }, 250);
          setToolbarDrag(null);
          setDragOverStageId(null);
          dragOverCanvasRef.current = false;
          toolbarDragRef.current = null;
          return;
        }
      }
      setToolbarDrag(null);
      setDragOverStageId(null);
      dragOverCanvasRef.current = false;
      toolbarDragRef.current = null;
      if (ghostRef.current) {
        ghostRef.current.remove();
        ghostRef.current = null;
      }
    };
    const handleMoveHighlight = (e: MouseEvent) => {
      if (!toolbarDrag) return;
      const canvasEl = canvasRef.current;
      if (canvasEl) {
        const rect = canvasEl.getBoundingClientRect();
        if (e.clientX >= rect.left && e.clientX <= rect.right &&
            e.clientY >= rect.top && e.clientY <= rect.bottom) {
          dragOverCanvasRef.current = true;
          const cx = (e.clientX - rect.left - pan.x) / scale;
          const cy = (e.clientY - rect.top - pan.y) / scale;
          const hitStage = getStageAtCanvasPos(cx, cy);
          setDragOverStageId(hitStage);
          // 更新ghost样式：可放置=accent色，不可放置=红色
          if (ghostRef.current) {
            if (hitStage) {
              ghostRef.current.style.borderColor = 'var(--accent)';
              ghostRef.current.style.background = 'var(--accent-light)';
              ghostRef.current.style.color = 'var(--accent)';
            } else {
              ghostRef.current.style.borderColor = '#f85149';
              ghostRef.current.style.background = '#f8514922';
              ghostRef.current.style.color = '#f85149';
            }
          }
        } else {
          dragOverCanvasRef.current = false;
          setDragOverStageId(null);
          if (ghostRef.current) {
            ghostRef.current.style.borderColor = 'var(--border)';
            ghostRef.current.style.background = 'var(--bg-tertiary)';
            ghostRef.current.style.color = 'var(--text-tertiary)';
          }
        }
      }
    };
    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mousemove', handleMoveHighlight);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mousemove', handleMoveHighlight);
      window.removeEventListener('mouseup', handleUp);
    };
  }, [toolbarDrag, pan, scale, stages, collapsedStages]);

  // 节点拖拽状态
  const [draggingNode, setDraggingNode] = useState<{
    nodeId: string;
    stageId: string;
    /** 鼠标按下时节点在内容区的初始位置（用于计算delta） */
    startContentX: number;
    startContentY: number;
    started: boolean;
    startX: number;
    startY: number;
    origPositions?: Record<string, { x: number; y: number }>;
  } | null>(null);

  // ── 状态初始化 ──


  // 连线拖拽时的实际目标节点（鼠标悬停的input anchor）
  const [connectTargetId, setConnectTargetId] = useState<string | null>(null);
  const [cycleTargetId, setCycleTargetId] = useState<string | null>(null);

  useEffect(() => {
    if (def) {
      setStages(sanitizeMappingReferences(def.stages));
      setName(def.name);
      // Auto-create stage edges only when none exist (new workflow / migration)
      if (def.stageEdges && def.stageEdges.length > 0) {
        setStageEdges(def.stageEdges);
      } else {
        const sorted = [...def.stages].sort((a, b) => a.order - b.order);
        const edges: WorkflowEdge[] = [];
        for (let idx = 0; idx < sorted.length - 1; idx++) {
          edges.push({
            id: generateEdgeId(sorted[idx].id, sorted[idx + 1].id),
            source: sorted[idx].id,
            target: sorted[idx + 1].id,
          });
        }
        setStageEdges(edges);
      }
    }
  }, [def]);

  // 组件挂载时：加载实例数据并取消旧的 running 实例
  useEffect(() => {
    if (!definitionId) return;
    // 先刷新实例数据，确保拿到最新状态
    useWorkflowStore.getState().loadInstances().then(() => {
      const currentInstances = useWorkflowStore.getState().instances;
      const runningInstances = currentInstances.filter(
        inst => inst.definitionId === definitionId && inst.status === 'running'
      );
      for (const inst of runningInstances) {
        console.log('[WorkflowEditor] 取消旧的 running 实例:', inst.id);
        useWorkflowStore.getState().cancelWorkflow(inst.id);
      }
    });
  }, [definitionId]);

  const handleExportWorkflow = useCallback(async () => {
    if (!definitionId) return;
    try {
      const filePath = await saveDialog({
        defaultPath: `${name || '工作流'}.json`,
        filters: [{ name: '工作流文件', extensions: ['json'] }],
      });
      if (filePath) {
        await invoke('export_workflow_to_file', { id: definitionId, filePath });
        onSaveResult?.(true);
      }
    } catch (err: any) {
      console.error('导出工作流失败:', err);
      onSaveResult?.(false);
    }
  }, [definitionId, name, onSaveResult]);

  const handleSave = async () => {
    if (!def) return;
    try {
      await updateDefinition(definitionId, { name, description, stages, stageEdges });
      onSaveResult?.(true);
    } catch {
      onSaveResult?.(false);
    }
  };

  const [isRunning, setIsRunning] = useState(false);
  const executionIdRef = useRef<string | null>(null);
  const definitionIdRef = useRef(definitionId);
  definitionIdRef.current = definitionId;
  const [restoredExecutionId, setRestoredExecutionId] = useState<string | null>(null);
  const [selectedHistoryId, setSelectedHistoryId] = useState<string | null>(null);
  const restoredSnapshotRef = useRef<any>(null);
  const restoredModCountRef = useRef<number>(0);
  const modCountRef = useRef<number>(0);

  /** 恢复指定历史执行的节点状态 */
  const handleRestoreExecution = async (executionId: string) => {
    try {
      const nodeExecs = await invoke<any[]>('get_node_executions', { executionId });
      const results: Record<string, any> = {};
      const states: Record<string, StepRunState> = {};
      for (const ne of nodeExecs) {
        if (ne.status === 'completed' && ne.output !== undefined) {
          results['node_' + ne.nodeId] = ne.output;
          states['node_' + ne.nodeId] = 'success';
        } else if (ne.status === 'failed') {
          results['node_' + ne.nodeId] = { error: ne.error || '执行失败' };
          states['node_' + ne.nodeId] = 'failed';
        } else if (ne.status === 'running') {
          states['node_' + ne.nodeId] = 'running';
        } else if (ne.status === 'skipped') {
          states['node_' + ne.nodeId] = 'skipped';
        }
      }
      // 记录恢复时的工作流快照（用于后续检测配置变更）
      restoredSnapshotRef.current = JSON.parse(JSON.stringify(stages));
      restoredModCountRef.current = modCountRef.current;
      setRestoredExecutionId(executionId);
      setNodeResults(results);
      setStepStates(states);
    } catch (err) {
      console.error('恢复执行状态失败:', err);
    }
  };

  const handleRunWorkflow = async () => {
    console.log('[WorkflowEditor] handleRunWorkflow called, isRunning:', isRunning, ', executionIdRef:', executionIdRef.current);
    if (!definitionId) return;

    // 执行前验证
    const execErrors = validateWorkflowForExecution(stages, stageEdges);
    const criticalErrors = execErrors.filter(e => e.severity === 'error');
    if (criticalErrors.length > 0) {
      showToast(`工作流验证失败：${criticalErrors.map(e => e.message).join('; ')}`, 'error');
      setIsRunning(false);
      return;
    }
    // 有 warning 级别的未就绪节点，提示但允许继续
    const warnings = execErrors.filter(e => e.severity === 'warning');
    if (warnings.length > 0) {
      showToast(`存在 ${warnings.length} 个未就绪节点，但存在完整执行路径，继续执行`, 'warning');
    }
    // 如果已经在执行中，先重置状态再重新开始
    if (isRunning) {
      console.warn('[WorkflowEditor] 检测到残留执行状态，强制重置');
      setIsRunning(false);
      // 给 React 一点时间处理状态更新
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    // 重置 executionIdRef
    console.log('[WorkflowEditor] handleRunWorkflow: resetting executionIdRef');
    executionIdRef.current = null;
    // 强制立即渲染 isRunning=true，避免 React 18 自动批处理合并状态更新
    flushSync(() => {
      setIsRunning(true);
      setNodeResults({});
      setStepStates({});
      // 执行时取消所有节点选中状态
      setSelectedNodeId(null);
      setSelectedNodeIds(new Set());
      setSelectedStageId(null);
    });
    try {
      // 清除历史恢复标记（开始真实执行）
      setRestoredExecutionId(null);
      restoredSnapshotRef.current = null;
      // 先保存当前编辑状态（轻量：仅保存 stages，不触发全量 reload）
      await invoke('save_workflow_dag', { id: definitionId, stages, stageEdges });
      // 预生成实例 ID 并在 invoke 前设置 ref（消除 IPC 竞态：快速工作流可能在响应返回前就完成）
      const preGeneratedId = crypto.randomUUID();
      executionIdRef.current = preGeneratedId;
      await useWorkflowStore.getState().safeStartWorkflow(definitionId, undefined, preGeneratedId);
      // 加载实例数据（初始状态）
      await useWorkflowStore.getState().loadInstances();
    } catch (err) {
      console.error('执行工作流失败:', err);
      setIsRunning(false);
      executionIdRef.current = null;
    }
  };

  /** 单点执行：只执行选中的单个节点 */
  const handleExecuteSingleNode = async () => {
    if (!selectedNodeId || !selectedInstanceId) {
      showToast('请先选择一个节点，且存在执行记录', 'warning');
      return;
    }
    if (isRunning) {
      showToast('当前有工作流正在执行，请等待完成', 'warning');
      return;
    }
    // 清除节点状态
    setStepStates({});
    setNodeResults({});
    setIsRunning(true);
    try {
      await useWorkflowStore.getState().executeSingleNode(selectedInstanceId, selectedNodeId);
      executionIdRef.current = selectedInstanceId;
      await useWorkflowStore.getState().loadInstances();
    } catch (err) {
      console.error('单点执行失败:', err);
      showToast(`单点执行失败: ${err}`, 'error');
    } finally {
      setIsRunning(false);
    }
  };

  /** 断点执行：从选中节点开始执行后续所有拓扑节点 */
  const handleExecuteFromNode = async () => {
    if (!selectedNodeId || !selectedInstanceId) {
      showToast('请先选择一个节点，且存在执行记录', 'warning');
      return;
    }
    if (isRunning) {
      showToast('当前有工作流正在执行，请等待完成', 'warning');
      return;
    }
    // 清除节点状态
    setStepStates({});
    setNodeResults({});
    setIsRunning(true);
    try {
      await useWorkflowStore.getState().executeFromNode(selectedInstanceId, selectedNodeId);
      executionIdRef.current = selectedInstanceId;
      await useWorkflowStore.getState().loadInstances();
    } catch (err) {
      console.error('断点执行失败:', err);
      showToast(`断点执行失败: ${err}`, 'error');
    } finally {
      setIsRunning(false);
    }
  };

  /** 手动终止当前执行 */
  const handleCancelExecution = async () => {
    const execId = executionIdRef.current;
    if (!execId) {
      setIsRunning(false);
      return;
    }
    console.log('[WorkflowEditor] 手动终止执行:', execId);
    try {
      // 1. 调用后端取消
      await invoke('cancel_workflow', { executionId: execId });
      // 3. 重置前端状态
      setIsRunning(false);
      executionIdRef.current = null;
      // 4. 刷新实例数据
      await useWorkflowStore.getState().loadInstances();
    } catch (err) {
      console.error('终止执行失败:', err);
      // 即使后端调用失败，也重置前端状态
      setIsRunning(false);
      executionIdRef.current = null;
    }
  };

  // 组件挂载时注册事件监听器（一次性，deps=[]）
  // 监听器通过 executionIdRef（ref）动态匹配当前执行，无需每次执行重新注册
  useEffect(() => {
    let unlistenNode: UnlistenFn | null = null;
    let unlistenExec: UnlistenFn | null = null;
    let unlistenStage: UnlistenFn | null = null;

    const setupListeners = async () => {
      console.log('[WorkflowEditor] setupListeners: starting listener registration...');
      unlistenNode = await listen<{
        execution_id: string;
        node_id: string;
        status: string;
        output?: any;
        error?: string;
      }>('workflow:node-status', (event) => {
        if (event.payload.execution_id !== executionIdRef.current) return;
        const nodeId = event.payload.node_id;
        const status = event.payload.status;
        if (status === 'completed' && event.payload.output !== undefined) {
          setNodeResults(prev => ({ ...prev, ['node_' + nodeId]: event.payload.output }));
        } else if (status === 'failed') {
          setNodeResults(prev => ({ ...prev, ['node_' + nodeId]: { error: event.payload.error || '执行失败' } }));
        }
        // 更新步骤状态（用于连线动画）
        setStepStates(prev => ({ ...prev, ['node_' + nodeId]: status === 'completed' ? 'success' : status === 'failed' ? 'failed' : status === 'running' ? 'running' : prev['node_' + nodeId] }));
      });

      unlistenExec = await listen<{ execution_id: string; definition_id: string; status: string; error?: string }>('workflow:execution-status', (event) => {
        // 通过 definitionId 匹配，彻底消除 IPC 竞态（不依赖 executionIdRef 时序）
        if (event.payload.definition_id !== definitionIdRef.current) return;
        if (event.payload.status === 'completed' || event.payload.status === 'failed' || event.payload.status === 'cancelled') {
          console.log('[WorkflowEditor] execution', event.payload.status, ', setting isRunning=false');
          setIsRunning(false);
          useWorkflowStore.getState().loadInstances();
        }
      });

      // 监听阶段门控失败事件
      unlistenStage = await listen<{ execution_id: string; stage_id: string; stage_name: string; status: string; reason?: string; error?: string }>('workflow:stage-status', (event) => {
        if (event.payload.status === 'gate_failed') {
          const detail = event.payload.reason || event.payload.error || '';
          showToast(`${event.payload.stage_name} 门控策略未通过${detail ? ': ' + detail : ''}`, 'error');
        }
      });

      console.log('[WorkflowEditor] setupListeners: both listeners registered successfully');
    };

    setupListeners();

    return () => {
      unlistenNode?.();
      unlistenExec?.();
      unlistenStage?.();
    };
  }, []);

  // 组件卸载时取消当前执行
  useEffect(() => {
    return () => {
      // 取消当前正在执行的实例（fire-and-forget，不等待结果）
      if (executionIdRef.current) {
        const execId = executionIdRef.current;
        console.log('[WorkflowEditor] 组件卸载，取消执行:', execId);
        invoke('cancel_workflow', { executionId: execId }).catch(() => {});
        executionIdRef.current = null;
      }
    };
  }, []);

  const handleNameChangeLocal = useCallback((newName: string) => {
    setName(newName);
    onNameChange?.(newName);
  }, [onNameChange]);

  // ── 统计信息 ──
  const stats = useMemo(() => {
    const result = {
      totalNodes: stages.reduce((sum, s) => sum + s.nodes.length, 0),
      totalEdges: stages.reduce((sum, s) => sum + s.edges.length, 0),
      totalStages: stages.length,
      nodeTypeCounts: {} as Record<string, number>,
    };
    for (const s of stages) {
      for (const n of s.nodes) {
        result.nodeTypeCounts[n.type] = (result.nodeTypeCounts[n.type] || 0) + 1;
      }
    }
    return result;
  }, [stages]);

  const handleAddStage = () => {
    modCountRef.current++;
    const newStage: Stage = {
      id: generateStageId(),
      name: `阶段 ${stages.length + 1}`,
      order: stages.length,
      nodes: [],
      edges: [],
      gate: { strategy: 'all', mergeStrategy: 'merge' },
    };
    // 将end节点从原最后阶段迁移到新阶段
    let updated = [...stages];
    const endNode = updated.flatMap(s => s.nodes).find(n => n.type === 'end');
    if (endNode) {
      updated = updated.map(s => ({
        ...s,
        nodes: s.nodes.filter(n => n.type !== 'end'),
        edges: s.edges.filter(e => e.source !== endNode.id && e.target !== endNode.id),
      }));
      newStage.nodes = [endNode];
    }
    updated.push(newStage);
    // Auto-connect: create stage edge from previous stage to new stage
    if (updated.length > 1) {
      const prevStage = updated[updated.length - 2];
      const newEdge: WorkflowEdge = {
        id: generateEdgeId(prevStage.id, newStage.id),
        source: prevStage.id,
        target: newStage.id,
      };
      setStageEdges(prev => [...prev, newEdge]);
    }
    setStages(updated.map((s, i) => ({ ...s, order: i })));
  };

  const handleDeleteStage = (stageId: string) => {
    modCountRef.current++;
    setConfirmAction({ type: 'deleteStage', targetId: stageId, label: stages.find(s => s.id === stageId)?.name || '此阶段' });
  };

  const confirmDelete = () => {
    if (!confirmAction) return;
    if (confirmAction.type === 'deleteStage') {
      const deleted = stages.find(s => s.id === confirmAction.targetId);
      const remaining = stages.filter(s => s.id !== confirmAction.targetId);
      // 如果被删阶段含end节点，将其迁移到新的最后阶段
      const endNode = deleted?.nodes.find(n => n.type === 'end');
      if (endNode && remaining.length > 0) {
        const lastIdx = remaining.length - 1;
        const lastStage = { ...remaining[lastIdx] };
        lastStage.nodes = [...lastStage.nodes, endNode];
        remaining[lastIdx] = lastStage;
      }
      // 如果删的不是最后阶段，确保end节点在最后阶段
      if (endNode === undefined) {
        const end = remaining.flatMap(s => s.nodes).find(n => n.type === 'end');
        if (end && remaining.length > 0) {
          const lastIdx = remaining.length - 1;
          const lastStage = remaining[lastIdx];
          if (lastStage.nodes.find(n => n.id === end.id)) {
            // end已在最后阶段，无需移动
          } else {
            remaining[lastIdx] = {
              ...lastStage,
              nodes: [...lastStage.nodes, end],
            };
            remaining.forEach(s => {
              if (s.id !== remaining[lastIdx].id) {
                s.nodes = s.nodes.filter(n => n.type !== 'end');
                s.edges = s.edges.filter(e => e.source !== end.id && e.target !== end.id);
              }
            });
          }
        }
      }
      const cleaned = sanitizeMappingReferences(remaining.map((s, i) => ({ ...s, order: i })));
      setStages(cleaned);
    } else if (confirmAction.type === 'deleteNode') {
      const afterDeleteNode = stages.map((s) => ({
        ...s,
        nodes: s.nodes.filter((n) => n.id !== confirmAction.targetId),
        edges: s.edges.filter((e) => e.source !== confirmAction.targetId && e.target !== confirmAction.targetId),
      }));
      setStages(sanitizeMappingReferences(afterDeleteNode));
      if (selectedNodeId === confirmAction.targetId) { setSelectedNodeId(null); setSelectedStageId(null); }
    } else if (confirmAction.type === 'deleteEdge') {
      const afterDeleteEdge = stages.map((s) => ({
        ...s,
        edges: s.edges.filter((e) => e.id !== confirmAction.targetId),
      }));
      setStages(sanitizeMappingReferences(afterDeleteEdge));
    } else if (confirmAction.type === 'deleteStageEdge') {
      const edge = stageEdges.find(e => e.id === confirmAction.targetId);
      setStageEdges(stageEdges.filter(e => e.id !== confirmAction.targetId));
      const srcStage = edge ? stages.find(s => s.id === edge.source) : null;
      const tgtStage = edge ? stages.find(s => s.id === edge.target) : null;
      const srcLabel = srcStage ? `step${srcStage.order + 1} ${srcStage.name}` : edge?.source || '';
      const tgtLabel = tgtStage ? `step${tgtStage.order + 1} ${tgtStage.name}` : edge?.target || '';
      showToast(`已删除阶段连线「${srcLabel} → ${tgtLabel}」`, 'success');
      setConfirmAction(null);
    } else if (confirmAction.type === 'deleteNodes') {
      // 批量删除选中节点（排除边界节点）
      const boundaryIds = new Set<string>();
      for (const s of stages) {
        for (const n of s.nodes) {
          if (n.isBoundary && selectedNodeIds.has(n.id)) boundaryIds.add(n.id);
        }
      }
      // 如果选中的全部是边界节点，拒绝删除
      if (boundaryIds.size === selectedNodeIds.size) {
        showToast('起始节点和结束节点不可删除', 'warning');
        setConfirmAction(null);
        return;
      }
      const toDelete = new Set([...selectedNodeIds].filter(id => !boundaryIds.has(id)));
      const afterDeleteNodes = stages.map(s => ({
        ...s,
        nodes: s.nodes.filter(n => !toDelete.has(n.id)),
        edges: s.edges.filter(e => !toDelete.has(e.source) && !toDelete.has(e.target)),
      }));
      setStages(sanitizeMappingReferences(afterDeleteNodes));
      setSelectedNodeIds(new Set());
      setSelectedNodeId(null);
      setSelectedStageId(null);
    }
    setConfirmAction(null);
  };

  const handleAddNode = (type: WorkflowNodeType, stageId: string) => {
    modCountRef.current++;
    // 收集阶段内已有节点位置，用于 findFreePosition
    const stage = stages.find(s => s.id === stageId);
    const existing = stage?.nodes.map(n => n.position ?? { x: 20, y: 20 }) ?? [];
    // 统一创建方法（不传 position → 自动计算空闲位置）
    const newNode = createWorkflowNode(type, undefined, existing, scale);
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, nodes: [...s.nodes, newNode] } : s
    ));
  };

  const handleDeleteNode = (nodeId: string) => {
    modCountRef.current++;
    // 边界节点（Start/End）不允许删除，即使误触也拒绝
    for (const s of stages) {
      const node = s.nodes.find(n => n.id === nodeId);
      if (node?.isBoundary) {
        showToast('起始节点和结束节点不可删除', 'warning');
        return;
      }
    }
    setConfirmAction({ type: 'deleteNode', targetId: nodeId, label: stages.flatMap(s => s.nodes).find(n => n.id === nodeId)?.label || '此节点' });
  };

  const handleDeleteEdge = (edgeId: string) => {
    modCountRef.current++;
    setConfirmAction({ type: 'deleteEdge', targetId: edgeId, label: '此连线' });
  };

  const handleDeleteStageEdge = (edgeId: string) => {
    modCountRef.current++;
    setConfirmAction({ type: 'deleteStageEdge', targetId: edgeId, label: getStageEdgeLabel(edgeId) });
  };
  // Helper: get readable stage edge label for delete confirmation
  const getStageEdgeLabel = (edgeId: string) => {
    const edge = stageEdges.find(e => e.id === edgeId);
    if (!edge) return '此阶段连线';
    const srcStage = stages.find(s => s.id === edge.source);
    const tgtStage = stages.find(s => s.id === edge.target);
    const srcLabel = srcStage ? `step${srcStage.order + 1} ${srcStage.name}` : edge.source;
    const tgtLabel = tgtStage ? `step${tgtStage.order + 1} ${tgtStage.name}` : edge.target;
    return `${srcLabel} → ${tgtLabel}`;
  };


  const handleUpdateNode = (nodeId: string, updates: Partial<WorkflowNode>) => {
    modCountRef.current++;
    setStages(stages.map((s) => ({
      ...s,
      nodes: s.nodes.map((n) => n.id === nodeId ? { ...n, ...updates } : n),
    })));
  };

  // ── 多选操作 ──
  const handleSelectNode = useCallback((nodeId: string, stageId: string, ctrlKey: boolean) => {
    if (ctrlKey) {
      // Ctrl+click: toggle in multi-select (always operates on selectedNodeIds)
      setSelectedNodeIds(prev => {
        const next = new Set(prev);
        if (next.has(nodeId)) {
          next.delete(nodeId);
          // If removing the active node, update selectedNodeId to another or null
          if (selectedNodeId === nodeId) {
            if (next.size > 0) { setSelectedNodeId([...next][0]); setSelectedStageId(stageId); }
            else { setSelectedNodeId(null); setSelectedStageId(null); }
          }
        } else {
          next.add(nodeId);
          setSelectedNodeId(nodeId);
          setSelectedStageId(stageId);
        }
        return next;
      });
    } else {
      // Normal click: single select (also add to selectedNodeIds for consistency)
      setSelectedNodeIds(new Set([nodeId]));
      setSelectedNodeId(nodeId);
      setSelectedStageId(stageId);
    }
  }, [selectedNodeId, selectedNodeIds]);

  const handleClearSelection = useCallback(() => {
    setSelectedNodeIds(new Set());
    setSelectedNodeId(null);
    setSelectedStageId(null);
  }, []);

  /** 获取多选节点的位置信息（stageId -> [{nodeId, x, y}]） */
  const getMultiSelectPositions = useCallback(() => {
    const positions: { stageId: string; nodeId: string; x: number; y: number }[] = [];
    for (const stage of stages) {
      for (const node of stage.nodes) {
        if (selectedNodeIds.has(node.id)) {
          positions.push({ stageId: stage.id, nodeId: node.id, x: node.position?.x ?? 20, y: node.position?.y ?? 20 });
        }
      }
    }
    return positions;
  }, [stages, selectedNodeIds]);

  const handleAlignHorizontal = useCallback(() => {
    const positions = getMultiSelectPositions();
    if (positions.length < 2) return;
    const avgY = positions.reduce((s, p) => s + p.y + NODE_H / 2, 0) / positions.length - NODE_H / 2;
    setStages(prev => prev.map(s => ({
      ...s,
      nodes: s.nodes.map(n => positions.some(p => p.nodeId === n.id && p.stageId === s.id)
        ? { ...n, position: { x: n.position?.x ?? 20, y: Math.round(avgY) } } : n),
    })));
  }, [getMultiSelectPositions]);

  const handleAlignVertical = useCallback(() => {
    const positions = getMultiSelectPositions();
    if (positions.length < 2) return;
    const avgX = positions.reduce((s, p) => s + p.x + NODE_W / 2, 0) / positions.length - NODE_W / 2;
    setStages(prev => prev.map(s => ({
      ...s,
      nodes: s.nodes.map(n => positions.some(p => p.nodeId === n.id && p.stageId === s.id)
        ? { ...n, position: { x: Math.round(avgX), y: n.position?.y ?? 20 } } : n),
    })));
  }, [getMultiSelectPositions]);

  const handleDistributeHorizontal = useCallback(() => {
    // 水平平均分布 = X轴等间距排列
    const positions = getMultiSelectPositions();
    if (positions.length < 3) return;
    const sorted = [...positions].sort((a, b) => a.x - b.x);
    const minX = sorted[0].x;
    const maxX = sorted[sorted.length - 1].x;
    const step = (maxX - minX) / (sorted.length - 1);
    const targets = new Map(sorted.map((p, i) => [p.nodeId + '|' + p.stageId, Math.round(minX + step * i)]));
    setStages(prev => prev.map(s => ({
      ...s,
      nodes: s.nodes.map(n => {
        const key = n.id + '|' + s.id;
        return targets.has(key) ? { ...n, position: { x: targets.get(key)!, y: n.position?.y ?? 20 } } : n;
      }),
    })));
  }, [getMultiSelectPositions]);

  const handleDistributeVertical = useCallback(() => {
    // 垂直平均分布 = Y轴等间距排列
    const positions = getMultiSelectPositions();
    if (positions.length < 3) return;
    const sorted = [...positions].sort((a, b) => a.y - b.y);
    const minY = sorted[0].y;
    const maxY = sorted[sorted.length - 1].y;
    const step = (maxY - minY) / (sorted.length - 1);
    const targets = new Map(sorted.map((p, i) => [p.nodeId + '|' + p.stageId, Math.round(minY + step * i)]));
    setStages(prev => prev.map(s => ({
      ...s,
      nodes: s.nodes.map(n => {
        const key = n.id + '|' + s.id;
        return targets.has(key) ? { ...n, position: { x: n.position?.x ?? 20, y: targets.get(key)! } } : n;
      }),
    })));
  }, [getMultiSelectPositions]);

  const handleBatchDelete = useCallback(() => {
    if (selectedNodeIds.size === 0) return;
    setConfirmAction({ type: 'deleteNodes', targetId: 'batch', label: `${selectedNodeIds.size} 个选中节点` });
  }, [selectedNodeIds]);

  // ── 连线操作（带实时预览） ──
  const handleStartConnect = (e: React.MouseEvent, nodeId: string, stageId: string) => {
    e.stopPropagation();
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const mouseCanvasX = (e.clientX - rect.left - pan.x) / scale;
    const mouseCanvasY = (e.clientY - rect.top - pan.y) / scale;
    setConnecting({ source: nodeId, stageId, mouseCanvasX, mouseCanvasY });
  };


  // ══════════════════════════════════════════
  // 统一坐标工具函数（纯算术方案）
  // ══════════════════════════════════════════════════════
    // ══════════════════════════════════════════
  /**
   * 通过DOM查询精确获取节点在画布坐标系中的位置和尺寸。
   * 原理：nodeEl.getBoundingClientRect()得到屏幕坐标，
   * canvasRef.getBoundingClientRect()得到画布屏幕坐标，
   * 两者差值除以scale并减去pan，得到画布坐标。
   * 比手动计算stage偏移更精确（不受标题栏高度变化影响）。
   */
  /**
   * 通过DOM查询获取节点锚点的精确画布坐标。
   * 利用锚点元素（data-anchor）的实际getBoundingClientRect，
   * 换算为画布坐标系的x,y。比纯算术方案更精确，不受缩放/标题栏高度影响。
   */
  

  /** 纯算术获取节点画布坐标（零DOM查询） */
  const wouldCreateCycle = useCallback((targetStageId: string): boolean => {
    const visited = new Set<string>();
    const queue = [targetStageId];
    while (queue.length > 0) {
      const cur = queue.shift()!;
      if (visited.has(cur)) continue;
      visited.add(cur);
      for (const se of stageEdges) {
        if (se.source === cur) queue.push(se.target);
      }
    }
    return visited.has(stageConnecting?.sourceStageId ?? '');
  }, [stageEdges, stageConnecting?.sourceStageId]);


  const getStagePositions = useCallback(() => {
    let leftOffset = 20;
    const map: Record<string, number> = {};
    for (const stage of stages) {
      map[stage.id] = leftOffset + (stageOffsets[stage.id] ?? 0);
      leftOffset += collapsedStages.has(stage.id) ? STAGE_COLLAPSED_W + STAGE_GAP : STAGE_W + STAGE_GAP;
    }
    return map;
  }, [stages, collapsedStages, stageOffsets]);

  const stagePositionsMap = getStagePositions();

  const canvasToContentPos = useCallback((canvasX: number, canvasY: number, stageId: string) => {
    const stageLeft = stagePositionsMap[stageId];
    return {
      x: canvasX - stageLeft,
      y: canvasY - STAGE_TOP - TITLE_H,
    };
  }, [stagePositionsMap]);


  /**
   * 检测添加边 source->target 后是否会产生闭环
   * 使用 DFS 从 target 出发，若能到达 source 则存在闭环
   */
  const hasCycle = useCallback((source: string, target: string, stage: Stage): boolean => {
    const adj = new Map<string, string[]>();
    for (const node of stage.nodes) {
      adj.set(node.id, []);
    }
    for (const edge of stage.edges) {
      const list = adj.get(edge.source);
      if (list) list.push(edge.target);
    }
    // 添加待检测边
    const newList = adj.get(source);
    if (newList) newList.push(target);

    // DFS 从 target 出发
    const visited = new Set<string>();
    const stack = [target];
    while (stack.length > 0) {
      const current = stack.pop()!;
      if (current === source) return true;
      if (visited.has(current)) continue;
      visited.add(current);
      const neighbors = adj.get(current);
      if (neighbors) {
        for (const n of neighbors) {
          if (!visited.has(n)) stack.push(n);
        }
      }
    }
    return false;
  }, []);

  const handleUpdateConnectingPos = useCallback((e: MouseEvent) => {
    if (!connecting) return;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const mx = (e.clientX - rect.left - pan.x) / scale;
    const my = (e.clientY - rect.top - pan.y) / scale;

    setConnecting(prev => prev ? {
      ...prev,
      mouseCanvasX: mx,
      mouseCanvasY: my,
    } : null);

    // 命中检测：鼠标是否在节点输入锚点附近（画布坐标）
    const stage = stages.find(s => s.id === connecting.stageId);
    if (!stage) return;
    let targetId: string | null = null;
    const stageLeft = stagePositionsMap[connecting.stageId];
    for (const node of stage.nodes) {
      if (node.id === connecting.source) continue;
      if (node.type === 'start') continue;
      if (stage.edges.some(e => e.source === connecting.source && e.target === node.id)) continue;
      const nPos = node.position ?? { x: 20, y: 20 };
      const nW = 160;
      const nH = 60;
      // 节点画布坐标（节点 position:absolute，定位基准 = 内容区 padding-box 左上角）
      const nodeCX = stageLeft + nPos.x + nW / 2;
      const nodeCY = STAGE_TOP + TITLE_H + nPos.y + nH / 2;
      const halfW = nW / (2 * scale); // 节点视觉半宽（画布坐标）
      const halfH = nH / (2 * scale); // 节点视觉半高（画布坐标）
      // 检测鼠标是否在节点视觉范围内（锚点在左边缘中心）
      if (mx >= nodeCX - halfW - 15/scale && mx <= nodeCX - halfW + nW / scale * 0.35 &&
          my >= nodeCY - halfH - 15/scale && my <= nodeCY + halfH + 15/scale) {
        targetId = node.id;
        break;
      }
    }
    // 检测目标节点是否会产生闭环
    if (targetId && hasCycle(connecting.source, targetId, stage)) {
      setCycleTargetId(targetId);
      setConnectTargetId(null);
    } else {
      setCycleTargetId(null);
      setConnectTargetId(targetId);
    }
  }, [connecting, pan, scale, stages, canvasToContentPos]);

  const handleEndConnect = useCallback((nodeId: string, stageId: string) => {
    modCountRef.current++;
    if (!connecting || connecting.source === nodeId) {
      setConnecting(null);
      setCycleTargetId(null);
      return;
    }
    // Start节点只有输出锚点，不可被连线
    const targetNode = stages.find(s => s.id === stageId)?.nodes.find(n => n.id === nodeId);
    if (targetNode?.type === 'start') {
      setConnecting(null);
      setConnectTargetId(null);
      setCycleTargetId(null);
      return;
    }
    // 不允许重复连线
    const stage = stages.find(s => s.id === stageId);
    if (stage && stage.edges.some(e => e.source === connecting.source && e.target === nodeId)) {
      setConnecting(null);
      setConnectTargetId(null);
      setCycleTargetId(null);
      return;
    }
    // 闭环检测：若创建此边会产生闭环，阻止创建
    if (stage && hasCycle(connecting.source, nodeId, stage)) {
      setConnecting(null);
      setConnectTargetId(null);
      setCycleTargetId(null);
      return;
    }

    const newEdge: WorkflowEdge = {
      id: generateEdgeId(connecting.source, nodeId),
      source: connecting.source,
      target: nodeId,
    };

    // 阶段间节点隔离：不允许跨阶段连线，只能通过门控传递数据
    if (connecting.stageId !== stageId) {
      setConnecting(null);
      setConnectTargetId(null);
      setCycleTargetId(null);
      return;
    }

    setStages(stages.map((s) => {
      if (s.id === connecting.stageId) {
        return { ...s, edges: [...s.edges, newEdge] };
      }
      return s;
    }));
    setConnecting(null);
    setConnectTargetId(null);
    setCycleTargetId(null);
    setStages((prev) => autoAssignStage(prev));
  }, [connecting, stages]);

  const handleCancelConnect = useCallback(() => {
    setConnecting(null);
    setConnectTargetId(null);
    setCycleTargetId(null);
  }, []);

  // 监听连线拖拽期间的鼠标移动和释放
  useEffect(() => {
    if (!connecting) return;
    const onMouseMove = (e: MouseEvent) => {
      handleUpdateConnectingPos(e);
    };
    const onMouseUp = (e: MouseEvent) => {
      // 释放鼠标时，检测是否在目标节点上（节点任意位置均可完成连线）
      const target = e.target as HTMLElement;
      const nodeEl = target.closest('[data-node]') as HTMLElement | null;
      const stageEl = target.closest('[data-stage]') as HTMLElement | null;
      if (nodeEl && stageEl) {
        const nodeId = nodeEl.getAttribute('data-node-id');
        const stageId = stageEl.getAttribute('data-stage-id');
        if (nodeId && stageId && nodeId !== connecting.source) {
          handleEndConnect(nodeId, stageId);
          return;
        }
      }
      setConnecting(null);
      setConnectTargetId(null);
    };
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
    };
  }, [connecting, handleUpdateConnectingPos]);

  // ── Delete 键：批量删除选中节点 / 删除确认 ──
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Delete' || e.key === 'Backspace') {
        if ((e.target as HTMLElement).closest('input, textarea, [contenteditable]')) return;
        if (selectedNodeIds.size > 0) {
          e.preventDefault();
          handleBatchDelete();
        }
      }
      // Escape: clear multi-select
      if (e.key === 'Escape' && !connecting) {
        if (selectedNodeIds.size > 0) {
          e.preventDefault();
          handleClearSelection();
        }
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [selectedNodeIds, connecting, handleBatchDelete, handleClearSelection]);

  const handleConfirmCondition = (condition: string, label: string) => {
    if (!conditionInput) return;
    // 检查是否已有相同source→target的edge，有则更新，无则新建
    const edgeId = generateEdgeId(conditionInput.source, conditionInput.target);
    const existingStage = stages.find(s => s.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target));
    const existingEdge = existingStage?.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target);

    setStages(stages.map((s) => {
      if (existingStage && s.id === existingStage.id && existingEdge) {
        // 更新已有edge的条件
        return { ...s, edges: s.edges.map(e => e.id === existingEdge.id ? { ...e, condition, label: label || condition } : e) };
      }
      if (s.id === conditionInput.stageId) {
        // 新建edge
        return { ...s, edges: [...s.edges, { id: edgeId, source: conditionInput.source, target: conditionInput.target, condition, label: label || condition }] };
      }
      return s;
    }));
    setConditionInput(null);
    setStages((prev) => autoAssignStage(prev));
  };

  const handleUpdateGate = (stageId: string, gate: Partial<GateConfig>) => {
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, gate: { ...s.gate, ...gate } } : s
    ));
  };

  const handleRenameStage = (stageId: string, newName: string) => {
    setStages(stages.map((s) =>
      s.id === stageId ? { ...s, name: newName } : s
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

  // ── 从实例中提取节点执行结果（兜底：事件监听未覆盖的场景） ──
  useEffect(() => {
    const instance = instances.find(i => i.definitionId === definitionId && i.status !== 'pending');
    if (!instance || !instance.steps) return;
    const results: Record<string, any> = {};
    for (const [nodeId, step] of Object.entries(instance.steps)) {
      if (step.output !== undefined) {
        results[`node_${nodeId}`] = step.output;
      } else if (step.error) {
        results[`node_${nodeId}`] = { error: step.error };
      }
    }
    if (Object.keys(results).length > 0) {
      setNodeResults(prev => ({ ...prev, ...results }));
    }
  }, [instances, definitionId]);

  // ── 画布拖拽平移（含防误触阈值 + 中键支持） ──

  const handleCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    console.log('[CANVAS-MDOWN] handleCanvasMouseDown called, target:', (e.target as HTMLElement).tagName, e.target.className);
    if ((e.target as HTMLElement).closest('[data-stage-gate-output]')) {
      return;
    }
    // 关闭连线右键菜单
    if (edgeContextMenu) setEdgeContextMenu(null);
    if (document.activeElement && document.activeElement !== e.target) {
      (document.activeElement as HTMLElement).blur();
    }
    if ((e.target as HTMLElement).closest('button, input, [data-anchor], [data-gate], [data-stage-entrance], [data-stage-gate-output], [data-edge], [data-stage-title]')) return;
    // Left button (0) or middle button (1): pan canvas
    if (e.button === 0 || e.button === 1) {
      e.preventDefault();
      // Click on a node -> select first, then pan on drag
      const nodeEl = (e.target as HTMLElement).closest('[data-node]');
      if (nodeEl) {
        const nodeId = nodeEl.getAttribute('data-node-id')!;
        const stageEl = (e.target as HTMLElement).closest('[data-stage]')!;
        const stageId = stageEl?.getAttribute('data-stage-id');
        if (stageId) handleSelectNode(nodeId, stageId, e.ctrlKey || e.metaKey);
      } else {
        handleClearSelection();
      }
      // Start panning (left or middle click anywhere pans)
      setIsPanning(true);
      panStartRef.current = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
      panThresholdRef.current = { startX: e.clientX, startY: e.clientY, triggered: e.button === 1 };
      return;
    }
    // Right button (2): box-select in content area
    if (e.button === 2) {
      const stageContentEl = (e.target as HTMLElement).closest('[data-stage-content]');
      if (stageContentEl) {
        e.preventDefault();
        const stageEl = (e.target as HTMLElement).closest('[data-stage]')!;
        const stageId = stageEl?.getAttribute('data-stage-id');
        if (stageId) {
          const rect = canvasRef.current?.getBoundingClientRect();
          if (!rect) return;
          const cx = (e.clientX - rect.left - pan.x) / scale;
          const cy = (e.clientY - rect.top - pan.y) / scale;
          const stageLeft = stagePositionsMap[stageId];
          const contentX = cx - stageLeft;
          const contentY = cy - STAGE_TOP - TITLE_H;
          setIsBoxSelecting(true);
          boxSelectStartRef.current = { x: cx, y: cy, stageId };
          setBoxSelectRect({ x1: contentX, y1: contentY, x2: contentX, y2: contentY, stageId });
          handleClearSelection();
        }
      }
      return;
    }
  }, [pan.x, pan.y, edgeContextMenu, handleSelectNode, handleClearSelection, scale, stagePositionsMap]);

  useEffect(() => {
    if (!isPanning) return;
    const handleMouseMove = (e: MouseEvent) => {
      if (!panStartRef.current || !panThresholdRef.current) return;
      const th = panThresholdRef.current;
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

  // ── 框选鼠标追踪 ──
  useEffect(() => {
    if (!isBoxSelecting) return;
    const handleMove = (e: MouseEvent) => {
      if (!boxSelectStartRef.current) return;
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const cx = (e.clientX - rect.left - pan.x) / scale;
      const cy = (e.clientY - rect.top - pan.y) / scale;
      const stageLeft = stagePositionsMap[boxSelectStartRef.current.stageId];
      const contentX = cx - stageLeft;
      const contentY = cy - STAGE_TOP - TITLE_H;
      setBoxSelectRect(prev => prev ? { ...prev, x2: contentX, y2: contentY } : null);
    };
    const handleUp = (e: MouseEvent) => {
      if (!boxSelectStartRef.current || !boxSelectRect) {
        setIsBoxSelecting(false);
        boxSelectStartRef.current = null;
        setBoxSelectRect(null);
        return;
      }
      // Find nodes inside the selection rect
      const stageId = boxSelectStartRef.current.stageId;
      const stage = stages.find(s => s.id === stageId);
      if (stage) {
        const r = boxSelectRect;
        const x1 = Math.min(r.x1, r.x2), x2 = Math.max(r.x1, r.x2);
        const y1 = Math.min(r.y1, r.y2), y2 = Math.max(r.y1, r.y2);
        const hitIds = new Set<string>();
        for (const node of stage.nodes) {
          const nx = node.position?.x ?? 20;
          const ny = node.position?.y ?? 20;
          // Node box in content coords: (nx, ny, nx+NODE_W, ny+NODE_H)
          if (nx + NODE_W > x1 && nx < x2 && ny + NODE_H > y1 && ny < y2) {
            hitIds.add(node.id);
          }
        }
        if (hitIds.size > 0) {
          setSelectedNodeIds(hitIds);
          const first = [...hitIds][0] ?? null;
          setSelectedNodeId(first);
          setSelectedStageId(stageId);
        }
      }
      setIsBoxSelecting(false);
      boxSelectStartRef.current = null;
      setBoxSelectRect(null);
    };
    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  }, [isBoxSelecting, boxSelectRect, pan, scale, stages, stagePositionsMap]);

  // ── 鼠标滚轮缩放（以光标位置为中心） ──
  const handleWheel = useCallback((e: React.WheelEvent) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    e.preventDefault();

    const mouseX = e.clientX - rect.left;
    const mouseY = e.clientY - rect.top;

    setScale((prevScale) => {
      const delta = -e.deltaY * 0.002;
      const newScale = Math.min(Math.max(prevScale + delta, 0.1), 4);
      const ratio = newScale / prevScale;

      setPan((prevPan) => ({
        x: mouseX - ratio * (mouseX - prevPan.x),
        y: mouseY - ratio * (mouseY - prevPan.y),
      }));

      return newScale;
    });
  }, []);

  // ── 节点拖拽（含防误触阈值 + 视觉反馈） ──

  const handleNodeMouseDown = useCallback((e: React.MouseEvent, nodeId: string, stageId: string) => {
    if ((e.target as HTMLElement).closest('[data-anchor]')) return;
    e.stopPropagation();
    e.preventDefault();
    // Ctrl+click: toggle multi-select (handled in handleCanvasMouseDown), don't start drag
    if (e.ctrlKey || e.metaKey) return;
    const node = stages.find((s) => s.id === stageId)?.nodes.find((n) => n.id === nodeId);
    if (!node) return;
    // 计算鼠标在内容区的起始位置（用于后续 delta 计算）
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return;
    const stageLeft = stagePositionsMap[stageId];
    const rawCanvasX = e.clientX - rect.left;
    const rawCanvasY = e.clientY - rect.top;
    const canvasX = (rawCanvasX - pan.x) / scale;
    const canvasY = (rawCanvasY - pan.y) / scale;
    const mouseContentX = canvasX - stageLeft;
    const mouseContentY = canvasY - STAGE_TOP - TITLE_H;
    // Store original positions for multi-drag
    const isMulti = selectedNodeIds.has(nodeId) && selectedNodeIds.size > 1;
    const origPositions: Record<string, { x: number; y: number }> = {};
    if (isMulti) {
      const stage = stages.find(s => s.id === stageId);
      if (stage) {
        for (const n of stage.nodes) {
          if (selectedNodeIds.has(n.id)) {
            origPositions[n.id] = { ...(n.position ?? { x: 20, y: 20 }) };
          }
        }
      }
    }
    // 记录所有选中节点的原始位置（单节点也记录，避免useEffect中读stages过期）
    const allOrigPositions: Record<string, { x: number; y: number }> = {};
    const stage = stages.find(s => s.id === stageId);
    if (stage) {
      const targetIds = isMulti ? selectedNodeIds : new Set([nodeId]);
      for (const n of stage.nodes) {
        if (targetIds.has(n.id)) {
          allOrigPositions[n.id] = { ...(n.position ?? { x: 20, y: 20 }) };
        }
      }
    }
    setDraggingNode({
      nodeId,
      stageId,
      startContentX: mouseContentX,
      startContentY: mouseContentY,
      started: false,
      startX: e.clientX,
      startY: e.clientY,
      origPositions: allOrigPositions,
    });
    setSelectedNodeId(nodeId);
    setSelectedStageId(stageId);
  }, [stages, stagePositionsMap, pan, scale]);

  useEffect(() => {
    if (!draggingNode) return;
    const handleMouseMove = (e: MouseEvent) => {
      const d = draggingNode;
      if (!d.started) {
        const dx = Math.abs(e.clientX - d.startX);
        const dy = Math.abs(e.clientY - d.startY);
        if (dx < NODE_DRAG_THRESHOLD && dy < NODE_DRAG_THRESHOLD) return;
        setDraggingNode(prev => prev ? { ...prev, started: true } : null);
      }
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const rawCanvasX = e.clientX - rect.left;
      const rawCanvasY = e.clientY - rect.top;
      const canvasX = (rawCanvasX - pan.x) / scale;
      const canvasY = (rawCanvasY - pan.y) / scale;
      const rel = canvasToContentPos(canvasX, canvasY, d.stageId);
      // delta = 鼠标当前位置 - 鼠标起始位置（内容区坐标）
      const deltaX = rel.x - d.startContentX;
      const deltaY = rel.y - d.startContentY;
      // All dragged nodes use origPositions (stored at mousedown time)
      setStages((prev) => prev.map((s) => {
        if (s.id !== d.stageId) return s;
        return {
          ...s,
          nodes: s.nodes.map((n) => {
            if (!d.origPositions || !d.origPositions[n.id]) return n;
            const meta = getNodeTypeMeta(n.type);
            const orig = d.origPositions[n.id];
            return { ...n, position: clampNodePosition(orig.x + deltaX, orig.y + deltaY, meta.nodeW, meta.nodeH, scale) };
          }),
        };
      }));
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
  }, [draggingNode, scale, pan.x, pan.y, canvasToContentPos, selectedNodeIds]);

  // ── 阶段连线拖拽：鼠标移动更新预览位置，释放取消 ──
  useEffect(() => {
    if (!stageConnecting) return;
    const onMouseMove = (e: MouseEvent) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const mx = (e.clientX - rect.left - pan.x) / scale;
      const my = (e.clientY - rect.top - pan.y) / scale;
      setStageConnecting(prev => prev ? { ...prev, mouseCanvasX: mx, mouseCanvasY: my } : null);
    };
    const onMouseUp = () => {
      // 延迟取消：让入口锚点的 React onMouseUp 有机会先处理连线
      setTimeout(() => {
        setStageConnecting(prev => {
          return prev !== null ? null : prev;
        });
      }, 0);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setStageConnecting(null);
    };
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [stageConnecting, scale, pan.x, pan.y]);

  // ── 阶段拖拽：拖动标题栏移动阶段位置 ──
  useEffect(() => {
    if (!draggingStageId) return;
    const onMouseMove = (e: MouseEvent) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;
      const currentX = (e.clientX - rect.left - pan.x) / scale;
      const deltaX = currentX - dragStartXRef.current;
      setStageOffsets(prev => ({
        ...prev,
        [draggingStageId]: dragStartOffsetRef.current + deltaX,
      }));
    };
    const onMouseUp = () => {
      setDraggingStageId(null);
    };
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
    };
  }, [draggingStageId, scale, pan.x]);




  const getStageAtCanvasPos = useCallback((cx: number, cy: number) => {
    for (const stage of stages) {
      const left = stagePositionsMap[stage.id];
      const width = collapsedStages.has(stage.id) ? STAGE_COLLAPSED_W : STAGE_W;
      const height = collapsedStages.has(stage.id) ? 123 : TITLE_H + CONTENT_H + GATE_H + 4;
      if (cx >= left && cx <= left + width && cy >= STAGE_TOP && cy <= STAGE_TOP + height) {
        return stage.id;
      }
    }
    return null;
  }, [stages, collapsedStages, stagePositionsMap]);



  // ── 渲染：节点 ──
    // ── 渲染：连线（纯算术坐标） ──
  const renderEdge = (edge: WorkflowEdge, stageId: string) => {
    const stage = stages.find((s) => s.id === stageId);
    if (!stage) return null;
    const sourceNode = stage.nodes.find((n) => n.id === edge.source);
    const targetNode = stage.nodes.find((n) => n.id === edge.target);
    if (!sourceNode || !targetNode) return null;

    const invScale = 1 / scale;

    // 纯算术锚点：节点 border-box 外边缘中点
    // 节点 position 是 CSS left/top，width=160/height=60 是 border-box 含 border
    // 节点 transform: scale(1/scale) 以 (80,30) 为原点
    // 视觉右边缘 = pos.x + 80 + 80/scale，视觉左边缘 = pos.x + 80 - 80/scale
    // 视觉Y中心 = pos.y + 30（反向缩放不改变Y中心位置）
    const halfW = NODE_W / 2;
    const halfH = NODE_H / 2;
    const srcPosX = sourceNode.position?.x ?? 20;
    const srcPosY = sourceNode.position?.y ?? 20;
    const tgtPosX = targetNode.position?.x ?? 20;
    const tgtPosY = targetNode.position?.y ?? 20;
    const SX = srcPosX + halfW + halfW * invScale; // output: 右边缘
    const SY = srcPosY + halfH;                      // Y中心
    const TX = tgtPosX + halfW - halfW * invScale;  // input: 左边缘
    const TY = tgtPosY + halfH;                      // Y中心

    const edgeRunState = (() => {
      const srcState = stepStates[`node_${sourceNode.id}`];
      const tgtState = stepStates[`node_${targetNode.id}`];
      if (tgtState === 'running') return 'running';
      if (tgtState === 'success') return 'success';
      if (tgtState === 'failed') return 'failed';
      if (srcState === 'success' && !tgtState) return 'running';
      return 'idle';
    })();

    const isHovered = hoveredEdgeId === edge.id;

    const H_SEG = 10 * invScale;
    const startHEndX = SX + H_SEG;
    const endHStartX = TX - H_SEG;
    const dx = Math.abs(endHStartX - startHEndX);
    const cpOffset = Math.max(20 * invScale, dx * 0.4);

    const arrowSize = 5 * invScale;
    const arrowLen = arrowSize * 1.5; // 箭头尖端到基线的水平距离
    const pathEndX = TX - arrowLen;    // 连线路径终点（箭头基线），箭头尖端精确在 TX
    const baseD = `M ${SX} ${SY} H ${startHEndX} C ${startHEndX + cpOffset} ${SY}, ${pathEndX - cpOffset} ${TY}, ${pathEndX} ${TY}`;
    const arrowPoints = `${pathEndX},${TY - arrowSize} ${TX},${TY} ${pathEndX},${TY + arrowSize}`;

    const lineColor = isHovered ? '#58a6ff' : edgeRunState === 'running' ? '#58a6ff' : edgeRunState === 'success' ? '#3fb950'
      : edgeRunState === 'failed' ? '#f85149' : 'var(--border)';

    return (
      <g key={edge.id}>
        <path d={baseD} stroke="transparent" strokeWidth={14 * invScale} fill="none" pointerEvents="stroke" style={{ cursor: 'pointer' }}
          onClick={() => handleDeleteEdge(edge.id)}
          onMouseEnter={() => setHoveredEdgeId(edge.id)}
          onMouseLeave={() => setHoveredEdgeId(null)}
          onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); setEdgeContextMenu({ edgeId: edge.id, x: e.clientX, y: e.clientY }); }}
        />
        {isHovered && (<path d={baseD} stroke={lineColor} strokeWidth={8 * invScale} fill="none" opacity={0.15} style={{ pointerEvents: 'none', transition: 'opacity 0.15s ease' }} />)}
        <path d={baseD} stroke={lineColor} strokeWidth={(isHovered ? 2.5 : edgeRunState === 'running' ? 2.5 : 2) * invScale} fill="none"
          strokeDasharray={edgeRunState === 'running' ? '6 3' : 'none'} style={{ pointerEvents: 'none', transition: 'stroke 0.15s ease, stroke-width 0.15s ease' }} />
        <polygon points={arrowPoints} fill={lineColor} style={{ pointerEvents: 'none' }} />
        {edgeRunState === 'running' && (<path d={baseD} stroke="#58a6ff" strokeWidth={4 * invScale} fill="none" strokeDasharray="8 12" opacity={0.3} style={{ pointerEvents: 'none', animation: 'flowDash 0.3s linear infinite' }} />)}
        {edge.label && (
          <g pointerEvents="all" onClick={() => handleDeleteEdge(edge.id)}>
            <rect x={(SX + TX) / 2 - 20 * invScale} y={(SY + TY) / 2 - 18 * invScale} width={40 * invScale} height={16 * invScale} rx={3 * invScale}
              fill="var(--bg-primary)" stroke="var(--border)" strokeWidth={0.5 * invScale} />
            <text x={(SX + TX) / 2} y={(SY + TY) / 2 - 8 * invScale} fill="var(--text-secondary)" fontSize={9 * invScale} textAnchor="middle">{edge.label}</text>
          </g>
        )}
      </g>
    );
  };
  // ── 渲染：连线预览（拖拽中的临时线条） ──
  const renderConnectingPreview = (stageId: string) => {
    if (!connecting || connecting.stageId !== stageId) return null;
    const sourceNode = stages.find(s => s.id === stageId)?.nodes.find(n => n.id === connecting.source);
    if (!sourceNode) return null;

    const invScale = 1 / scale;

    // 纯算术锚点
    const halfW = 80, halfH = 30;
    const srcPosX = sourceNode.position?.x ?? 20;
    const srcPosY = sourceNode.position?.y ?? 20;
    const SX = srcPosX + halfW + halfW * invScale;
    const SY = srcPosY + halfH;

    // 鼠标位置：画布坐标 → 内容区坐标
    const stageLeft = stagePositionsMap[stageId];
    const mx = connecting.mouseCanvasX - stageLeft;
    const my = connecting.mouseCanvasY - 20 - 36;

    const H_SEG = 10 * invScale;
    const hEndX = SX + H_SEG;
    const dx = Math.abs(mx - hEndX);
    const cpOffset = Math.max(20 * invScale, dx * 0.4);
    const d = `M ${SX} ${SY} H ${hEndX} C ${hEndX + cpOffset} ${SY} ${mx - cpOffset} ${my} ${mx} ${my}`;
    return (
      <path d={d} stroke="var(--accent)" strokeWidth={2 * invScale} strokeDasharray="6 3" fill="none" opacity={0.7} style={{ pointerEvents: 'none' }} />
    );
  };
  // ── 渲染：阶段间连线 ──
  const gateStrategyLabel = (s: string) => ({ all: '全部执行成功', count: '指定数量成功', threshold: '合并运算判断' }[s] || s);
  const gateStrategyDesc = (s: string) => ({ all: '所有节点执行成功后继续', count: '指定数量节点执行成功后继续', threshold: '合并运算值满足条件后继续' }[s] || s);
  const mergeStrategyLabel = (s: string) => ({ merge: '合并为对象', concat: '合并为数组', pick_first: '取第一个结果', pick_last: '取最后个结果', custom: '自定义处理' }[s] || s);
  const renderStageLinks = () => {
    // 从 stageEdges 渲染（而非按 order 自动生成）
    const links: React.ReactNode[] = [];
    let leftOffset = 20;
    const sp: number[] = [];
    for (let j = 0; j < stages.length; j++) {
      sp.push(leftOffset + (stageOffsets[stages[j].id] ?? 0));
      leftOffset += collapsedStages.has(stages[j].id) ? STAGE_COLLAPSED_W + STAGE_GAP : STAGE_W + STAGE_GAP;
    }

    const invScale = 1 / scale;

    for (const edge of stageEdges) {
      const srcStage = stages.find(s => s.id === edge.source);
      const tgtStage = stages.find(s => s.id === edge.target);
      if (!srcStage || !tgtStage) continue;
      const srcCollapsed = collapsedStages.has(srcStage.id);
      const tgtCollapsed = collapsedStages.has(tgtStage.id);

      // 纯算术：阶段间连线锚点（画布坐标系）
      // 源阶段右边中点（门控栏中点Y或折叠中心Y）
      // 目标阶段左边中点（标题栏中点Y或折叠中心Y）
      const srcIdx = stages.findIndex(s => s.id === srcStage.id);
      const tgtIdx = stages.findIndex(s => s.id === tgtStage.id);
      if (srcIdx < 0 || tgtIdx < 0) continue;
      const srcX = sp[srcIdx] + (srcCollapsed ? STAGE_COLLAPSED_W + 6 : STAGE_W + 6);
      const srcY = srcCollapsed ? STAGE_TOP + 54 + 50 : STAGE_TOP + TITLE_H + CONTENT_H + GATE_H / 2;
      const tgtX = sp[tgtIdx] - 6;
      const tgtY = tgtCollapsed ? STAGE_TOP + 27 : STAGE_TOP + TITLE_H / 2;

      // 箭头尖端对齐：路径终点前移箭头长度，让箭头尖端恰好到达 tgtX
      const asLen = 5 * invScale * 1.5;
      const slPathEndX = tgtX - asLen;

      // 贝塞尔控制点
      const gapDx = Math.abs(tgtX - srcX);
      const cpOff = Math.max(30 * invScale, gapDx * 0.4);
      // 2px horizontal segments at both ends
      const horiz = 2;
      const d = `M ${srcX} ${srcY} L ${srcX + horiz} ${srcY} C ${srcX + horiz + cpOff} ${srcY} ${slPathEndX - cpOff - horiz} ${tgtY} ${slPathEndX} ${tgtY} L ${slPathEndX - horiz} ${tgtY} L ${slPathEndX} ${tgtY}`;


      const srcState = stepStates[`stage_${srcStage.id}`];
      const tgtState = stepStates[`stage_${tgtStage.id}`];
      const rs = tgtState === 'running' ? 'running' : tgtState === 'success' ? 'success' : tgtState === 'failed' ? 'failed' : srcState === 'success' && !tgtState ? 'running' : 'idle';
      const lc = rs === 'running' ? '#58a6ff' : rs === 'success' ? '#3fb950' : rs === 'failed' ? '#f85149' : 'var(--border)';
      const sw = (rs === 'running' ? 2.5 : 2) * invScale;
      const as = 5 * invScale;

      links.push(
        <g key={`sl-${edge.id}`} data-edge={edge.id}>
          {/* Hit area: transparent path along the curve */}
          <path d={d} stroke="transparent" strokeWidth={14 * invScale} fill="none" pointerEvents="stroke" style={{ cursor: 'pointer' }}
            onClick={(e) => { e.stopPropagation(); e.preventDefault(); handleDeleteStageEdge(edge.id); }}
            onMouseEnter={() => setHoveredStageEdge(edge.id)}
            onMouseLeave={() => setHoveredStageEdge(null)}
          />
          {/* Hover glow: semi-transparent thick overlay */}
          {hoveredStageEdge === edge.id && (
            <path d={d} stroke="#58a6ff" strokeWidth={8 * invScale} fill="none" opacity={0.15} style={{ pointerEvents: 'none', transition: 'opacity 0.15s ease' }} />
          )}
          {/* Main visible path */}
          <path d={d} stroke={hoveredStageEdge === edge.id ? '#58a6ff' : lc} strokeWidth={hoveredStageEdge === edge.id ? 2.5 * invScale : sw} fill="none" strokeDasharray={rs === 'running' ? '6 3' : rs === 'idle' ? '6 4' : 'none'} style={{ transition: 'stroke 0.3s ease, stroke-width 0.15s ease' }} pointerEvents="none" />
          {/* Arrow polygon */}
          <polygon points={`${slPathEndX},${tgtY - as} ${tgtX},${tgtY} ${slPathEndX},${tgtY + as}`} fill={hoveredStageEdge === edge.id ? '#58a6ff' : lc} pointerEvents="none" />
          {/* Running animation */}
          {rs === 'running' && <path d={d} stroke="#58a6ff" strokeWidth={4 * invScale} fill="none" strokeDasharray="8 12" opacity={0.3} style={{ animation: 'flowDash 0.3s linear infinite' }} pointerEvents="none" />}
          {/* Label box */}
          <g pointerEvents="none">
            <rect x={(srcX + tgtX) / 2 - 16 * invScale} y={(srcY + tgtY) / 2 - 22 * invScale} width={32 * invScale} height={44 * invScale} rx={4 * invScale} fill="var(--bg-primary)" stroke="var(--border)" strokeWidth={0.5 * invScale} />
            <text x={(srcX + tgtX) / 2} y={(srcY + tgtY) / 2 - 10 * invScale} fill="#8b949e" fontSize={9 * invScale} textAnchor="middle">{`step${srcStage.order + 1}`}</text>
            <text x={(srcX + tgtX) / 2} y={(srcY + tgtY) / 2 + 2 * invScale} fill="#8b949e" fontSize={10 * invScale} textAnchor="middle">→</text>
            <text x={(srcX + tgtX) / 2} y={(srcY + tgtY) / 2 + 14 * invScale} fill="#8b949e" fontSize={9 * invScale} textAnchor="middle">{`step${tgtStage.order + 1}`}</text>
          </g>
        </g>
      );
    }
    // 阶段连线拖拽预览线
    if (stageConnecting) {
      const srcStage = stages.find(s => s.id === stageConnecting.sourceStageId);
      if (srcStage) {
        const srcCollapsed = collapsedStages.has(srcStage.id);
        const srcIdx = stages.findIndex(s => s.id === srcStage.id);
        if (srcIdx >= 0) {
          const srcX = sp[srcIdx] + (srcCollapsed ? STAGE_COLLAPSED_W + 6 : STAGE_W + 6);
          const srcY = srcCollapsed ? STAGE_TOP + 54 + 50 : STAGE_TOP + TITLE_H + CONTENT_H + GATE_H / 2;
          const tgtX = stageConnecting.mouseCanvasX;
          const tgtY = stageConnecting.mouseCanvasY;
          const invScale = 1 / scale;
          const gapDx = Math.abs(tgtX - srcX);
          const cpOff = Math.max(30 * invScale, gapDx * 0.4);
          const horiz = 2;
          const d = `M ${srcX} ${srcY} L ${srcX + horiz} ${srcY} C ${srcX + horiz + cpOff} ${srcY} ${tgtX - cpOff - horiz} ${tgtY} ${tgtX} ${tgtY} L ${tgtX - horiz} ${tgtY} L ${tgtX} ${tgtY}`;
          links.push(
            <g key="stage-connecting-preview">
              <path d={d} stroke="#58a6ff" strokeWidth={2 * invScale} fill="none" strokeDasharray="6 4" opacity={0.6} pointerEvents="none" />
            </g>
          );
        }
      }
    }
    return <>{links}</>;
  };


  // ── 计算不可达节点（用于标记红色虚线边框） ──
  const reachableNodeIds = useMemo(() => getReachableNodes(stages, stageEdges), [stages, stageEdges]);
  const unreachableNodeIds = useMemo(
    () => new Set(stages.flatMap(s => s.nodes).filter(n => n.type !== "start" ).map(n => n.id).filter(nid => !reachableNodeIds.has(nid))),
    [stages, reachableNodeIds]
  );
  // ── JSX ──
  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* CSS动画注入 */}
      <style>{`
        @keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
        @keyframes flowDash { from { stroke-dashoffset: 0; } to { stroke-dashoffset: -20; } }
        @keyframes pulseGlow { 0%, 100% { box-shadow: inset 0 0 6px #3b82f666, inset 0 0 12px #60a5fa33; } 50% { box-shadow: inset 0 0 14px #2563ebcc, inset 0 0 28px #3b82f666, 0 0 8px #60a5fa44; } }
      `}</style>

      {/* ── 工具栏 ── */}
      <div className="flex items-center gap-2 px-4 py-2 shrink-0" style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
        <input
          value={name}
          onChange={(e) => handleNameChangeLocal(e.target.value)}
          className="text-xs outline-none px-2.5 py-1.5 rounded-lg"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)', minWidth: 120, maxWidth: 280 }}
          placeholder="工作流名称"
        />
        <span className="text-[10px] px-2 py-0.5 rounded-full" style={{ color: 'var(--accent)', background: 'var(--accent-light)' }}>
          {def?.version || 'v1.0.0'}
        </span>

        <div className="flex-1" />

        {/* ── 历史执行恢复 ── */}
        <div className="flex items-center gap-1.5">
          
          <div className="relative">
            <select
              value={selectedHistoryId || ''}
              onChange={(e) => {
                const execId = e.target.value;
                if (!execId) {
                  setSelectedHistoryId(null);
                  setRestoredExecutionId(null);
                  setNodeResults({});
                  setStepStates({});
                  restoredSnapshotRef.current = null;
                  return;
                }
                handleRestoreExecution(execId);
                setSelectedHistoryId(execId);
              }}
              className="text-[11px] px-2 py-1 rounded outline-none appearance-none cursor-pointer"
              style={{
                border: '1px solid var(--border)',
                background: 'var(--bg-tertiary)',
                color: 'var(--text-secondary)',
                minWidth: 120,
                maxWidth: 200,
                paddingRight: 20,
              }}
              title="选择历史执行记录恢复节点执行状态"
            >
              <option value="">查看历史执行记录</option>
              {instances
                .filter(i => i.definitionId === definitionId && i.status !== 'pending')
                .sort((a, b) => (b.createdAt || 0) - (a.createdAt || 0))
                .slice(0, 20)
                .map((inst) => {
                  const dateStr = inst.createdAt ? new Date(inst.createdAt * 1000).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '';
                  const statusMap: Record<string, string> = { success: '✓', failed: '✗', running: '▶', cancelled: '○', timeout: '△' };
                  const icon = statusMap[inst.status] || '?';
                  return (
                    <option key={inst.id} value={inst.id}>
                      {icon} {dateStr} — {inst.status}
                    </option>
                  );
                })}
            </select>
            {/* 自定义下拉箭头 */}
            <svg
              width="10" height="10" viewBox="0 0 10 10"
              style={{ position: 'absolute', right: 6, top: '50%', transform: 'translateY(-50%)', pointerEvents: 'none', color: 'var(--text-tertiary)' }}
              fill="none" stroke="currentColor" strokeWidth="1.3"
            >
              <path d="M2.5 3.5L5 6.5 7.5 3.5" />
            </svg>
          </div>
        </div>

        <div className="w-px h-5" style={{ background: 'var(--border)' }} />

        {/* 节点类型拖拽区 */}
        <div className="flex items-center gap-1 flex-wrap">
          {BUILTIN_NODE_TYPES.map((nt) => (
            <div
              key={nt.type}
              className="flex items-center gap-1 px-2 py-1 rounded text-[11px] cursor-grab select-none transition-all duration-150"
              style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
              onMouseDown={(e) => {
                e.preventDefault();
                toolbarDragRef.current = nt.type;
                setToolbarDrag({ type: nt.type, icon: nt.icon, label: nt.label, color: nt.color, ghostX: e.clientX, ghostY: e.clientY });
                // 清理可能残留的旧 ghost（防止双击时残留透明节点）
                if (ghostRef.current) {
                  ghostRef.current.remove();
                  ghostRef.current = null;
                }
                // Create ghost element
                const ghost = document.createElement('div');
                ghost.textContent = nt.icon + ' ' + nt.label;
                ghost.style.cssText = `position:fixed;pointer-events:none;z-index:99999;padding:4px 12px;border-radius:6px;font-size:12px;opacity:0.85;white-space:nowrap;border:1px solid ${nt.color};background:${nt.color}22;color:${nt.color};transform:translate(-50%,-50%)`;
                ghost.style.left = e.clientX + 'px';
                ghost.style.top = e.clientY + 'px';
                document.body.appendChild(ghost);
                ghostRef.current = ghost;
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.borderColor = 'var(--accent)';
                e.currentTarget.style.background = 'var(--accent)';
                e.currentTarget.style.color = '#fff';
                e.currentTarget.style.transform = 'translateY(-1px)';
                e.currentTarget.style.boxShadow = '0 2px 8px rgba(0,0,0,0.15)';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.borderColor = 'var(--border)';
                e.currentTarget.style.background = 'var(--bg-tertiary)';
                e.currentTarget.style.color = 'var(--text-primary)';
                e.currentTarget.style.transform = 'translateY(0)';
                e.currentTarget.style.boxShadow = 'none';
              }}
            >
              <span style={{ display: 'inline-flex', alignItems: 'center', justifyContent: 'center', width: 18, height: 18, borderRadius: 4, fontSize: 11, background: `${nt.color}22`, color: nt.color }}>{nt.icon}</span>
              <span>{nt.label}</span>
            </div>
          ))}
        </div>

        <div className="w-px h-5" style={{ background: 'var(--border)' }} />

        <button
          onClick={() => setShowGrid((v) => !v)}
          className="pd-btn px-2 py-1 text-[11px] rounded flex items-center justify-center"
          style={{
            width: 28, height: 28,
            border: showGrid ? '1px solid var(--accent)' : '1px solid var(--border)',
            background: showGrid ? 'var(--accent-light)' : 'var(--bg-tertiary)',
            color: showGrid ? 'var(--accent)' : 'var(--text-secondary)',
          }}
          title={showGrid ? '隐藏辅助网格' : '显示辅助网格'}
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.2">
            <line x1="3" y1="0" x2="3" y2="14" /><line x1="7" y1="0" x2="7" y2="14" /><line x1="11" y1="0" x2="11" y2="14" />
            <line x1="0" y1="3" x2="14" y2="3" /><line x1="0" y1="7" x2="14" y2="7" /><line x1="0" y1="11" x2="14" y2="11" />
          </svg>
        </button>

        <div className="w-px h-5" style={{ background: 'var(--border)' }} />

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
            className="pd-btn px-1.5 py-1 text-[11px] rounded"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="返回列表"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M9 3L5 7l4 4" />
            </svg>
          </button>
          <button
            onClick={handleExportWorkflow}
            className="flex items-center justify-center px-1.5 py-1 rounded text-[11px] transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="导出工作流为 JSON 文件"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M7 2v7M4.5 6L7 8.5 9.5 6M2 11h10" />
            </svg>
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

      {/* 画布 + 配置面板 flex-row */}
      <div className="flex flex-row flex-1 min-h-0 relative">
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
                : isBoxSelecting ? 'crosshair'
                : dragOverStageId === null ? (dragOverCanvasRef.current ? 'not-allowed' : 'grab') : 'grab',
          userSelect: stageConnecting ? 'none' : undefined,
        }}
        onMouseDown={handleCanvasMouseDown}
        onWheel={handleWheel}
        onContextMenu={(e) => {
          // 右键在阶段内容区内：阻止默认菜单（用于框选）
          if ((e.target as HTMLElement).closest('[data-stage-content]')) {
            e.preventDefault();
          }
        }}
      >
        {/* 网格辅助线 — 画布背景，随平移缩放无限延伸 */}
        {showGrid && (
          <div
            className="absolute inset-0 pointer-events-none"
            style={{
              zIndex: 0,
              backgroundImage: `radial-gradient(circle, rgba(139,148,158,0.4) 1px, transparent 1px)`,
              backgroundSize: `${20 * scale}px ${20 * scale}px`,
              backgroundPosition: `${pan.x}px ${pan.y}px`,
              opacity: 0.6,
            }}
          />
        )}
        {/* 缩放控制 + 统计 + 帮助 浮动Bar */}
        <div
          className="absolute bottom-3 right-3 z-30 flex items-center gap-1 rounded-lg px-2 py-1.5"
          style={{ background: 'var(--bg-secondary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)' }}
        >
          {/* 执行工作流按钮 */}
          <button
            onClick={handleRunWorkflow}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded"
            style={{
              background: isRunning ? '#3fb95033' : '#3fb95022',
              color: '#3fb950',
              border: 'none',
            }}
            disabled={isRunning}
            title={isRunning ? '执行中...' : '执行工作流'}
          >
            {isRunning ? (
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" className="pd-animate-spin">
                <circle cx="5" cy="5" r="4" stroke="currentColor" strokeWidth="1.5" strokeDasharray="6 6" />
              </svg>
            ) : (
              <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
                <polygon points="3,1 10,6 3,11" />
              </svg>
            )}
          </button>
          {/* 终止执行按钮（仅在执行中显示） */}
          {isRunning && (
            <button
              onClick={handleCancelExecution}
              className="pd-btn w-6 h-6 flex items-center justify-center rounded"
              style={{
                background: '#f8514922',
                color: '#f85149',
                border: 'none',
              }}
              title="终止执行"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor">
                <rect x="1" y="1" width="8" height="8" rx="1" />
              </svg>
            </button>
          )}
          <div className="w-px h-4" style={{ background: 'var(--border)' }} />

          {/* 操作帮助按钮 */}
          <div className="relative group">
            <button
              className="pd-btn w-6 h-6 flex items-center justify-center rounded"
              style={{ background: '#8b949e22', color: '#8b949e', border: 'none' }}
              title="操作帮助"
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3"><circle cx="6" cy="6" r="4.5" /><path d="M4.5 5a1.5 1.5 0 0 1 3 0c0 1-1.5 1-1.5 2.5" /><circle cx="6" cy="9" r="0.5" fill="currentColor" /></svg>
            </button>
            <div className="absolute bottom-full right-0 mb-2 w-[310px] rounded-lg p-3 opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-opacity duration-200" style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)', zIndex: 100 }}>
              <div className="text-[10px] font-semibold mb-2 pb-1.5" style={{ color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}><span className="inline-flex items-center gap-1"><svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2"><circle cx="6" cy="6" r="4.5" /><path d="M4.5 5a1.5 1.5 0 0 1 3 0c0 1-1.5 1-1.5 2.5" /><circle cx="6" cy="9" r="0.5" fill="currentColor" /></svg>操作帮助</span></div>
              <div className="space-y-0 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>平移画布</span><span style={{ color: 'var(--text-tertiary)' }}>空白区左键拖拽 / 中键</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>缩放画布</span><span style={{ color: 'var(--text-tertiary)' }}>鼠标滚轮</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>添加节点</span><span style={{ color: 'var(--text-tertiary)' }}>拖拽工具栏节点到阶段</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>连接节点</span><span style={{ color: 'var(--text-tertiary)' }}>拖拽输出锚点→输入锚点</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>删除连线</span><span style={{ color: 'var(--text-tertiary)' }}>点击连线</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>删除节点/阶段</span><span style={{ color: 'var(--text-tertiary)' }}>悬停 x 按钮</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>选中节点</span><span style={{ color: 'var(--text-tertiary)' }}>单击</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>多选节点</span><span style={{ color: 'var(--text-tertiary)' }}>内容区右键框选 / Ctrl+点击</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>批量移动</span><span style={{ color: 'var(--text-tertiary)' }}>拖拽任一选中节点</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>对齐/分布</span><span style={{ color: 'var(--text-tertiary)' }}>多选后使用浮动栏按钮</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>批量删除</span><span style={{ color: 'var(--text-tertiary)' }}>Delete / Backspace</span></div>
                <div className="flex justify-between py-1.5"><span>取消连线/多选</span><span style={{ color: 'var(--text-tertiary)' }}>Esc</span></div>
              </div>
            </div>
          </div>
          <div className="w-px h-4" style={{ background: 'var(--border)' }} />

          {/* 统计信息按钮 */}
          <div className="relative group">
            <button
              className="pd-btn w-6 h-6 flex items-center justify-center rounded"
              style={{ background: '#3fb95022', color: '#3fb950', border: 'none' }}
              title="工作流统计"
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3">
                <rect x="1" y="6" width="2.5" height="5" rx="0.5" /><rect x="4.75" y="3" width="2.5" height="8" rx="0.5" /><rect x="8.5" y="1" width="2.5" height="10" rx="0.5" />
              </svg>
            </button>
            <div className="absolute bottom-full right-0 mb-2 w-[220px] rounded-lg p-3 opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-opacity duration-200" style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)', zIndex: 100 }}>
              <div className="text-[10px] font-semibold mb-2 pb-1.5" style={{ color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}><span className="inline-flex items-center gap-1"><svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2"><rect x="1" y="6" width="2.5" height="5" rx="0.5" /><rect x="4.75" y="3" width="2.5" height="8" rx="0.5" /><rect x="8.5" y="1" width="2.5" height="10" rx="0.5" /></svg>工作流统计</span></div>
              <div className="space-y-0 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>阶段</span><span style={{ color: 'var(--text-primary)' }}>{stats.totalStages}</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}><span>节点</span><span style={{ color: 'var(--text-primary)' }}>{stats.totalNodes}</span></div>
                <div className="flex justify-between py-1.5" style={{ borderBottom: '1px solid var(--border)' }}><span>连线</span><span style={{ color: 'var(--text-primary)' }}>{stats.totalEdges}</span></div>
                {Object.entries(stats.nodeTypeCounts).map(([type, count]) => {
                  const m = getNodeTypeMeta(type as WorkflowNodeType);
                  return (
                    <div key={type} className="flex justify-between items-center py-1.5" style={{ borderBottom: '1px dashed var(--border)' }}>
                      <span className="flex items-center gap-1">
                        <span style={{ color: m.color }}>{m.icon}</span>
                        <span>{m.label}</span>
                      </span>
                      <span style={{ color: 'var(--text-primary)' }}>{count}</span>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>

          <div className="w-px h-4" style={{ background: 'var(--border)' }} />

          {/* 缩放控制（彩色图标） */}
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
            className="pd-btn w-6 h-6 flex items-center justify-center rounded"
            style={{ background: '#58a6ff22', color: '#58a6ff', border: 'none' }}
            title="放大"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5"><circle cx="5" cy="5" r="3.5" /><line x1="7.5" y1="7.5" x2="11" y2="11" /><line x1="3" y1="5" x2="7" y2="5" /><line x1="5" y1="3" x2="5" y2="7" /></svg>
          </button>
          <span className="text-[10px] w-[36px] text-center font-mono" style={{ color: 'var(--text-secondary)' }}>
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
            className="pd-btn w-6 h-6 flex items-center justify-center rounded"
            style={{ background: '#58a6ff22', color: '#58a6ff', border: 'none' }}
            title="缩小"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5"><circle cx="5" cy="5" r="3.5" /><line x1="7.5" y1="7.5" x2="11" y2="11" /><line x1="3" y1="5" x2="7" y2="5" /></svg>
          </button>
          <button
            onClick={() => { setPan({ x: 0, y: 0 }); setScale(1); }}
            className="pd-btn w-6 h-6 flex items-center justify-center rounded"
            style={{ background: '#8b949e22', color: '#8b949e', border: 'none' }}
            title="重置视图"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M2 1L1 3l3 1M10 11l1-2-3-1M1 3a5 5 0 0 1 9 0M11 9a5 5 0 0 1-9 0" /></svg>
          </button>
          {/* 多选对齐工具栏 */}
          {selectedNodeIds.size > 1 && (
            <>
              <div className="w-px h-4" style={{ background: 'var(--border)' }} />
              <span className="text-[9px]" style={{ color: 'var(--accent)' }}>{selectedNodeIds.size}</span>
              <button onClick={handleAlignVertical} className="pd-btn w-6 h-6 flex items-center justify-center rounded" style={{ background: '#3fb95022', color: '#3fb950', border: 'none' }} title="垂直对齐">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3"><line x1="6" y1="1" x2="6" y2="11" /><line x1="3" y1="3" x2="9" y2="3" /><line x1="3" y1="9" x2="9" y2="9" /><line x1="6" y1="3" x2="3" y2="3" strokeDasharray="1 1" /><line x1="6" y1="3" x2="9" y2="3" strokeDasharray="1 1" /><line x1="6" y1="9" x2="3" y2="9" strokeDasharray="1 1" /><line x1="6" y1="9" x2="9" y2="9" strokeDasharray="1 1" /></svg>
              </button>
              <button onClick={handleAlignHorizontal} className="pd-btn w-6 h-6 flex items-center justify-center rounded" style={{ background: '#3fb95022', color: '#3fb950', border: 'none' }} title="水平对齐">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3"><line x1="1" y1="6" x2="11" y2="6" /><line x1="3" y1="3" x2="3" y2="9" /><line x1="9" y1="3" x2="9" y2="9" /><line x1="3" y1="6" x2="3" y2="3" strokeDasharray="1 1" /><line x1="3" y1="6" x2="3" y2="9" strokeDasharray="1 1" /><line x1="9" y1="6" x2="9" y2="3" strokeDasharray="1 1" /><line x1="9" y1="6" x2="9" y2="9" strokeDasharray="1 1" /></svg>
              </button>
              <button onClick={handleDistributeHorizontal} className="pd-btn w-6 h-6 flex items-center justify-center rounded" style={{ background: '#d2992222', color: '#d29922', border: 'none' }} title="水平平均分布">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2"><rect x="1.5" y="3.5" width="1.2" height="5" rx="0.3" fill="currentColor" /><rect x="5.4" y="3.5" width="1.2" height="5" rx="0.3" fill="currentColor" /><rect x="9.3" y="3.5" width="1.2" height="5" rx="0.3" fill="currentColor" /><path d="M3.3 6h1.5" strokeWidth="1" /><path d="M7.2 6h1.5" strokeWidth="1" /></svg>
              </button>
              <button onClick={handleDistributeVertical} className="pd-btn w-6 h-6 flex items-center justify-center rounded" style={{ background: '#d2992222', color: '#d29922', border: 'none' }} title="垂直平均分布">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2"><rect x="3.5" y="1.5" width="5" height="1.2" rx="0.3" fill="currentColor" /><rect x="3.5" y="5.4" width="5" height="1.2" rx="0.3" fill="currentColor" /><rect x="3.5" y="9.3" width="5" height="1.2" rx="0.3" fill="currentColor" /><path d="M6 3.3v1.5" strokeWidth="1" /><path d="M6 7.2v1.5" strokeWidth="1" /></svg>
              </button>
              <button onClick={handleBatchDelete} className="pd-btn w-6 h-6 flex items-center justify-center rounded" style={{ background: '#f8514922', color: '#f85149', border: 'none' }} title="批量删除 (Delete)">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3"><path d="M3 3l6 6M9 3l-6 6" /></svg>
              </button>
            </>
          )}
        </div>

        {/* 多选提示 */}
        {selectedNodeIds.size > 1 && !connecting && (
          <div
            className="absolute top-3 left-1/2 -translate-x-1/2 z-30 px-3 py-1.5 rounded-lg text-[10px] flex items-center gap-2"
            style={{ background: 'var(--bg-secondary)', border: '1px solid var(--accent)', color: 'var(--accent)', boxShadow: 'var(--shadow-md)' }}
          >
            <span className="w-1.5 h-1.5 rounded-full inline-block" style={{ background: 'var(--accent)' }} />
            已选中 {selectedNodeIds.size} 个节点 — 可拖拽批量移动 / Delete 删除 / Esc 取消
          </div>
        )}

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
            width: CANVAS_W,
            height: CANVAS_H,
            position: 'relative',
          }}
        >



          {/* SVG 层：阶段间连线（画布坐标，pointer-events:none） */}
          <svg
            style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: '100%', zIndex: 1 }}
          >

            {renderStageLinks()}
          </svg>

          {/* 空状态 */}
          {stages.length === 0 && (
            <div className="flex items-center justify-center absolute inset-0" style={{ color: 'var(--text-tertiary)', fontSize: 14 }}>
              点击 "+ 阶段" 开始创建工作流
            </div>
          )}

          {/* ── 阶段列 ── */}
          {(() => {
            let leftOffset = 20;
            const stagePositions: number[] = [];
            for (let i = 0; i < stages.length; i++) {
              stagePositions.push(leftOffset + (stageOffsets[stages[i].id] ?? 0));
              leftOffset += collapsedStages.has(stages[i].id) ? STAGE_COLLAPSED_W + STAGE_GAP : STAGE_W + STAGE_GAP;
            }
            return (
              <>
          {stages.map((stage, stageIndex) => {
            const isCollapsed = collapsedStages.has(stage.id);
            const stageRunState = stepStates[`stage_${stage.id}`];
            return (
              <React.Fragment key={stage.id}>
                <div
                  data-stage={stage.id}
                  data-stage-id={stage.id}
                  style={{
                    position: 'absolute',
                    top: 20,
                    left: stagePositions[stageIndex],
                    width: isCollapsed ? 72 : 480,
                    height: isCollapsed ? 154 : TITLE_H + CONTENT_H + GATE_H + 4,
                    borderRadius: 8,
                    background: 'var(--bg-secondary)',
                    border: '1px solid var(--border)',
                    transition: isCollapsed ? 'width 0.25s cubic-bezier(0.4,0,0.2,1)' : 'none',
                    overflow: 'visible',
                    clipPath: 'inset(0 round 8px)',
                    boxShadow: stageRunState === 'running'
                      ? '0 0 16px #58a6ff33'
                      : 'var(--shadow-sm)',
                    display: 'flex',
                    flexDirection: 'column',
                    zIndex: 2,
                  }}
                >
                  {/* 网格辅助线 — 阶段工作区内 */}
                  {showGrid && (
                    <div
                      className="absolute inset-0 pointer-events-none"
                      style={{
                        zIndex: 0,
                        backgroundImage: `radial-gradient(circle, rgba(139,148,158,0.4) 1px, transparent 1px)`,
                        backgroundSize: '20px 20px',
                        backgroundPosition: '0 0',
                        opacity: 0.6,
                      }}
                    />
                  )}

                  {/* 阶段标题栏（正向缩放，不做反向补偿） */}
                  <div data-stage-title={stage.id}
                    data-title
                    className={isCollapsed ? 'flex flex-col shrink-0' : 'flex items-center justify-between shrink-0'}
                    style={{
                      padding: isCollapsed ? '6px 8px' : '6px 10px',
                      background: 'var(--bg-tertiary)',
                      borderBottom: isCollapsed ? '1px solid var(--border)' : '1px solid var(--border)',
                      borderRadius: '8px 8px 0 0',
                      height: isCollapsed ? 54 : 36,
                      position: 'relative',
                      zIndex: 10,
                      overflow: 'visible',
                      cursor: draggingStageId === stage.id ? 'grabbing' : 'grab',
                    }}
                    onMouseDown={(e) => {
                      if (e.button !== 0) return;
                      const target = e.target as HTMLElement;
                      if (target.tagName === 'BUTTON' || target.tagName === 'INPUT') return;
                      e.stopPropagation();
                      const rect = canvasRef.current?.getBoundingClientRect();
                      if (!rect) return;
                      const mx = (e.clientX - rect.left - pan.x) / scale;
                      dragStartXRef.current = mx;
                      dragStartOffsetRef.current = stageOffsets[stage.id] ?? 0;
                      setDraggingStageId(stage.id);
                    }}
                  >
                    {isCollapsed && (
                      <>
                        <div className="flex items-center justify-between w-full mb-1">
                          <span className="inline-flex items-center justify-center rounded text-[10px] font-bold shrink-0" style={{ width: 33, height: 22, background: 'var(--accent-light)', color: 'var(--accent)' }}>
                            {`step${stage.order + 1}`}
                          </span>
                          <button onClick={(e) => { e.stopPropagation(); toggleCollapseStage(stage.id); }} className="pd-btn rounded text-[10px] transition-colors duration-150 flex items-center justify-center" style={{ width: 22, height: 22, color: 'var(--text-tertiary)', background: 'var(--bg-secondary)', border: '1px solid var(--border)' }} title="展开阶段">▶</button>
                        </div>
                        <span className="text-[10px] truncate" style={{ color: 'var(--text-primary)', width: '100%', textAlign: 'center', display: 'block' }}>
                          {stage.name}
                        </span>
                      </>
                    )}
                    {!isCollapsed && (
                      <>
                        <div className="flex items-center gap-2 flex-1 min-w-0">
                          <span className="inline-flex items-center justify-center rounded text-[10px] font-bold shrink-0" style={{ width: 33, height: 22, background: 'var(--accent-light)', color: 'var(--accent)' }}>
                            {`step${stage.order + 1}`}
                          </span>
                          <input
                            value={stage.name}
                            onChange={(e) => handleRenameStage(stage.id, e.target.value)}
                            className="text-xs outline-none px-2 py-1 rounded"
                            style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)', flex: 1, minWidth: 0, maxWidth: 'calc(100% - 100px)' }}
                          />
                        </div>
                        <div className="flex items-center gap-1 shrink-0">
                          <button onClick={(e) => { e.stopPropagation(); toggleCollapseStage(stage.id); }} className="pd-btn rounded text-[10px] transition-colors duration-150 flex items-center justify-center" style={{ width: 22, height: 22, color: 'var(--text-tertiary)', background: 'var(--bg-secondary)', border: '1px solid var(--border)' }} title="折叠阶段">◀</button>
                          <button onClick={(e) => { e.stopPropagation(); handleDeleteStage(stage.id); }} className="pd-btn rounded text-[10px] transition-colors duration-150 flex items-center justify-center" style={{ width: 22, height: 22, color: 'var(--status-danger)', background: 'var(--bg-secondary)', border: '1px solid var(--border)' }} title="删除阶段">x</button>
                        </div>
                      </>
                    )}
                  </div>


                  {/* 阶段内容区 — 反向缩放保持节点固定大小 */}
                  {!isCollapsed && (
                    <>
                      {/* 拖放高亮层：不受反向缩放影响，覆盖整个可视内容区域 */}
                      <div
                        className="absolute"
                        style={{
                          top: 36, // 标题栏高度
                          left: 2, // CONTENT_PAD
                          width: 476, // STAGE_W - CONTENT_PAD*2
                          height: 500 + 56 + 4, // 内容区 + Gate区域 + 间距
                          border: dragOverStageId === stage.id ? '2px dashed var(--accent)' : '2px dashed transparent',
                          borderRadius: 6,
                          background: dragOverStageId === stage.id ? 'var(--accent-light)' : 'transparent',
                          transition: 'border-color 0.15s ease, background 0.15s ease',
                          pointerEvents: 'none',
                          zIndex: 2,
                        }}
                      />
                      <div
                        data-stage-content={stage.id}
                        className="relative"
                        style={{
                          height: 500,
                          padding: 12,
                          overflow: 'hidden',
                        }}
                      >
                        {/* 阶段内连线 SVG — 在内容区内渲染，overflow:hidden 自然裁剪 */}
                        <svg
                          style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: '100%', zIndex: 1, pointerEvents: 'none' }}
                        >
                          {stage.edges.map((e) => renderEdge(e, stage.id))}
                          {connecting && connecting.stageId === stage.id && renderConnectingPreview(stage.id)}
                          {/* 框选矩形 */}
                          {isBoxSelecting && boxSelectRect && boxSelectRect.stageId === stage.id && (() => {
                            const r = boxSelectRect;
                            const x = Math.min(r.x1, r.x2), y = Math.min(r.y1, r.y2);
                            const w = Math.abs(r.x2 - r.x1), h = Math.abs(r.y2 - r.y1);
                            if (w < 3 || h < 3) return null;
                            const inv = 1 / scale;
                            return (
                              <g style={{ pointerEvents: 'none' }}>
                                <rect x={x} y={y} width={w} height={h} rx={2}
                                  fill="var(--accent)" fillOpacity={0.08}
                                  stroke="var(--accent)" strokeWidth={1.5 * inv} />
                                {/* Corner handles */}
                                <circle cx={x} cy={y} r={2.5 * inv} fill="var(--accent)" />
                                <circle cx={x + w} cy={y} r={2.5 * inv} fill="var(--accent)" />
                                <circle cx={x} cy={y + h} r={2.5 * inv} fill="var(--accent)" />
                                <circle cx={x + w} cy={y + h} r={2.5 * inv} fill="var(--accent)" />
                              </g>
                            );
                          })()}
                        </svg>
                        {/* 节点 zIndex:2 确保在连线之上 */}
                        {stage.nodes.map((node) => (
                <WorkflowNodeItem
                  key={node.id}
                  node={node}
                  stageId={stage.id}
                  scale={scale}
                  selectedNodeId={selectedNodeId}
                  selectedNodeIds={selectedNodeIds}
                  draggingNode={draggingNode}
                  hoveredNodeId={hoveredNodeId}
                  connectTargetId={connectTargetId}
                  cycleTargetId={cycleTargetId}
                  connecting={connecting}
                  stepStates={stepStates}
                  nodeResults={nodeResults}
                  selectedNodeId={selectedNodeId}
                  isUnreachable={unreachableNodeIds.has(node.id)}
                  isRestoredResult={restoredExecutionId !== null}
                  isConfigChanged={(() => {
                    const snap = restoredSnapshotRef.current;
                    if (!snap) return false;
                    if (stages.length !== snap.length) return true;
                    for (const s of snap) {
                      const cur = stages.find((st: any) => st.id === s.id);
                      if (!cur) return true;
                      if (s.nodes.length !== cur.nodes.length) return true;
                      if (s.edges.length !== cur.edges.length) return true;
                      for (const n of s.nodes) {
                        const curNode = cur.nodes.find((cn: any) => cn.id === n.id);
                        if (!curNode) return true;
                        if (JSON.stringify(n.params) !== JSON.stringify(curNode.params)) return true;
                        if (n.type !== curNode.type) return true;
                        if (n.label !== curNode.label) return true;
                      }
                      for (const e of s.edges) {
                        const curEdge = cur.edges.find((ce: any) => ce.id === e.id);
                        if (!curEdge) return true;
                        if (e.source !== curEdge.source || e.target !== curEdge.target) return true;
                      }
                    }
                    return false;
                  })()}
                  onSelectNode={handleSelectNode}
                  onNodeMouseDown={handleNodeMouseDown}
                  onDeleteNode={handleDeleteNode}
                  onEndConnect={handleEndConnect}
                  onStartConnect={handleStartConnect}
                  onHoverNode={setHoveredNodeId}
                />
              ))}
                      </div>

                      {/* Gate 区域（正向缩放，不做反向补偿） */}
                      <div
                        data-gate
                        className="mx-2 p-2 rounded-lg cursor-pointer transition-colors duration-150"
                        style={{
                          height: GATE_H,
                          border: `1px solid ${stageRunState === 'running' ? '#58a6ff88' : 'var(--border)'}`,
                          background: 'var(--bg-primary)',
                          position: 'relative',
                          zIndex: 10,
                          overflow: 'visible',
                        }}
                        onClick={() => {
                          setShowCustomMode(stage?.gate?.mergeStrategy === 'custom');
                          setGateStrategy(stage?.gate?.strategy || 'all');
                          setCustomMode(stage?.gate?.customMode || (stage?.gate?.customScript ? 'editor' : 'selector'));
                          setGateInput({ stageId: stage.id });
                        }}>
                        <div className="flex items-center justify-between mb-1">
                          <span className="text-[10px] font-semibold flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
                            门控 Gate
                          </span>
                          <span className="text-[10px] flex items-center gap-1" style={{ color: stageRunState === 'running' ? '#58a6ff' : stageRunState === 'success' ? '#3fb950' : 'var(--status-success)' }}>
                            <span className="w-1.5 h-1.5 rounded-full inline-block" style={{ background: stageRunState === 'running' ? '#58a6ff' : stageRunState === 'success' ? '#3fb950' : 'var(--status-success)', animation: stageRunState === 'running' ? 'spin 1s linear infinite' : 'none' }} />
                            {(() => { const r = stage.nodes.filter(n => n.type !== 'start'); const u = r.filter(n => unreachableNodeIds.has(n.id)).length; return `${r.length - u}/${r.length}`; })()} 就绪
                          </span>
                        </div>
                        <div style={{borderTop: '1px solid var(--border)', marginBottom: '6px'}}></div>
                        <div className="flex items-center gap-3">
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            策略: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{gateStrategyLabel(stage.gate.strategy)}</span>
                            <span className="ml-1" style={{ color: 'var(--text-tertiary)' }}>{gateStrategyDesc(stage.gate.strategy)}</span>
                            {stage.gate.threshold !== undefined && stage.gate.strategy === 'count' && (
                              <span className="ml-1 text-[10px]" style={{ color: 'var(--accent)' }}>(完成节点数: {stage.gate.threshold})</span>
                            )}
                            {stage.gate.threshold !== undefined && stage.gate.strategy === 'threshold' && (
                              <span className="ml-1 text-[10px]" style={{ color: 'var(--accent)' }}>(阈值: {stage.gate.threshold})</span>
                            )}
                          </span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            合并: <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ color: 'var(--text-primary)', background: 'var(--bg-tertiary)' }}>{mergeStrategyLabel(stage.gate.mergeStrategy)}</span>
                            {stage.gate.mergeStrategy === 'custom' && stage.gate.customScript && (
                              <span className="ml-1 text-[10px] truncate max-w-[80px]" style={{ color: 'var(--text-tertiary)' }} title={stage.gate.customScript}>脚本</span>
                            )}
                          </span>
                        </div>                    </div>
                    </>
                  )}
                  {/* 折叠摘要 — 四行显示 */}
                  {isCollapsed && (
                    <div className="flex flex-col items-center justify-center px-1" style={{ color: 'var(--text-tertiary)', fontSize: 10, height: 100, gap: 3, lineHeight: '16px', paddingTop: 10 }}>
                      <span>{stage.nodes.length} 节点</span>
                      <span>{stage.edges.length} 连线</span>
                      <span>{gateStrategyLabel(stage.gate.strategy)}</span>
                      <span>{mergeStrategyLabel(stage.gate.mergeStrategy)}</span>
                    </div>
                  )}
                </div>
                  {/* 阶段入口锚点（接收阶段连线） */}
                  {!stage.nodes.some(n => n.type === 'start') && (
                  <div
                    data-stage-entrance
                    data-stage-id={stage.id}
                    style={{
                      position: 'absolute',
                      left: stagePositions[stageIndex] - 6,
                      top: isCollapsed ? STAGE_TOP + 27 : STAGE_TOP + TITLE_H / 2 - 6,
                      width: 12, height: 12, borderRadius: '50%',
                      background: stageConnecting && stageConnecting.sourceStageId !== stage.id ? (wouldCreateCycle(stage.id) ? '#f85149' : '#3fb950') : '#58a6ff',
                      border: '2px solid var(--bg-primary)',
                      cursor: 'pointer',
                      zIndex: 20,
                      opacity: (stageConnecting && stageConnecting.sourceStageId !== stage.id) ? 1 : (hoveredAnchor === 'entrance-' + stage.id ? 1 : 0.4),
                      transform: (stageConnecting && stageConnecting.sourceStageId !== stage.id && hoveredAnchor === 'entrance-' + stage.id) ? 'scale(1.5)' : 'scale(1)',
                      transition: 'opacity 0.15s, background 0.15s, transform 0.15s',
                    }}
                    onMouseEnter={() => setHoveredAnchor('entrance-' + stage.id)}
                    onMouseLeave={() => setHoveredAnchor(null)}
                    onMouseUp={(e) => {
                      e.stopPropagation();
                      e.preventDefault();
                      if (!stageConnecting) return;
                      const targetStageId = stage.id;
                      if (stageConnecting.sourceStageId === targetStageId) return;
                      const visited = new Set<string>();
                      const checkCycle = (sid: string) => {
                        if (visited.has(sid)) return;
                        visited.add(sid);
                        for (const se of stageEdges) {
                          if (se.source === sid) checkCycle(se.target);
                        }
                      };
                      checkCycle(targetStageId);
                      if (visited.has(stageConnecting.sourceStageId)) {
                        showToast('不允许创建闭环连线', 'error');
                        return;
                      }
                      if (stageEdges.some(se => se.source === stageConnecting.sourceStageId && se.target === targetStageId)) {
                        showToast('该阶段连线已存在', 'warning');
                        setStageConnecting(null);
                        return;
                      }
                      setStageEdges(prev => [...prev, {
                        id: generateEdgeId(stageConnecting.sourceStageId, targetStageId),
                        source: stageConnecting.sourceStageId,
                        target: targetStageId,
                      }]);
                      showToast('阶段连线已创建', 'success');
                      setStageConnecting(null);
                    }}
                  />
                  )}
                  {/* 阶段出口锚点（拖拽创建阶段连线） */}
                  {!stage.nodes.some(n => n.type === 'end') && (
                  <div
                    data-stage-gate-output
                    data-stage-id={stage.id}
                    style={{
                      position: 'absolute',
                      left: isCollapsed
                        ? stagePositions[stageIndex] + STAGE_COLLAPSED_W - 6
                        : stagePositions[stageIndex] + STAGE_W - 6,
                      top: isCollapsed
                        ? STAGE_TOP + 54 + 50 - 6
                        : STAGE_TOP + TITLE_H + CONTENT_H + GATE_H / 2 - 6,
                      width: 12, height: 12, borderRadius: '50%',
                      background: '#58a6ff',
                      border: '2px solid var(--bg-primary)',
                      cursor: stageConnecting || hoveredAnchor === 'exit-' + stage.id ? 'crosshair' : 'pointer', zIndex: 20,
                      opacity: stageConnecting || hoveredAnchor === 'exit-' + stage.id ? 1 : 0.4,
                      transition: 'opacity 0.15s, cursor 0.15s',
                    }}
                    onMouseEnter={() => setHoveredAnchor('exit-' + stage.id)}
                    onMouseLeave={() => setHoveredAnchor(null)}
                    onMouseDown={(e) => {
                      e.stopPropagation();
                      const rect = canvasRef.current?.getBoundingClientRect();
                      if (!rect) return;
                      const mx = (e.clientX - rect.left - pan.x) / scale;
                      const my = (e.clientY - rect.top - pan.y) / scale;
                      setStageConnecting({ sourceStageId: stage.id, mouseCanvasX: mx, mouseCanvasY: my });
                    }}
                  />
                  )}

</React.Fragment>
            );
          })}
              </>
            );
          })()}
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

      {/* ── 连线右键菜单 ── */}
      {edgeContextMenu && (
        <>
          <div
            className="fixed inset-0 z-[999]"
            onClick={() => setEdgeContextMenu(null)}
            onContextMenu={(e) => { e.preventDefault(); setEdgeContextMenu(null); }}
          />
          <div
            className="absolute z-[1000] rounded-lg py-1 min-w-[140px]"
            style={{
              left: edgeContextMenu.x,
              top: edgeContextMenu.y,
              background: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
              boxShadow: 'var(--shadow-lg)',
            }}
          >
            {/* 查找edge所属stage */}
            {(() => {
              const edgeStage = stages.find(s => s.edges.find(e => e.id === edgeContextMenu.edgeId));
              const edge = edgeStage?.edges.find(e => e.id === edgeContextMenu.edgeId);
              const sourceNode = edgeStage?.nodes.find(n => n.id === edge?.source);
              const targetNode = edgeStage?.nodes.find(n => n.id === edge?.target);
              return (
                <>
                  <div className="px-3 py-1.5 text-[10px] border-b" style={{ color: 'var(--text-tertiary)', borderBottomColor: 'var(--border)' }}>
                    {sourceNode?.label || '?'} → {targetNode?.label || '?'}
                  </div>
                  {edge && edgeStage && (
                    <button
                      className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                      style={{ color: 'var(--text-primary)', background: 'transparent', border: 'none' }}
                      onClick={() => {
                        setEdgeContextMenu(null);
                        setConditionInput({
                          source: edge.source,
                          target: edge.target,
                          stageId: edgeStage.id,
                        });
                      }}
                      onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--accent-light)'; e.currentTarget.style.color = 'var(--accent)'; }}
                      onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; e.currentTarget.style.color = 'var(--text-primary)'; }}
                    >
                      <span>{'✎'}</span>
                      <span>{edge.condition ? '编辑条件' : '添加条件'}</span>
                      {edge.condition && <span className="ml-auto text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{edge.label || edge.condition}</span>}
                    </button>
                  )}
                  <button
                    className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors"
                    style={{ color: 'var(--status-danger)', background: 'transparent', border: 'none' }}
                    onClick={() => {
                      setEdgeContextMenu(null);
                      handleDeleteEdge(edgeContextMenu.edgeId);
                    }}
                    onMouseEnter={(e) => { e.currentTarget.style.background = 'rgba(248,81,73,0.1)'; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                  >
                    <span>{'✕'}</span>
                    <span>删除连线</span>
                  </button>
                </>
              );
            })()}
          </div>
        </>
      )}

      {/* ── 删除二次确认弹窗 ── */}
      {confirmAction && (
        <div className="absolute inset-0 flex items-center justify-center z-[1000]" style={{ background: 'var(--bg-overlay)' }}>
          <div
            className="rounded-xl p-5 w-[320px]"
            style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-sm font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
              确认删除
            </h3>
            <p className="text-xs mb-4" style={{ color: 'var(--text-secondary)' }}>
              是否删除 <span style={{ color: 'var(--text-primary)', fontWeight: 600 }}>{confirmAction.label}</span>连线？此操作不可撤销。
            </p>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => setConfirmAction(null)}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
              >取消</button>
              <button
                onClick={confirmDelete}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ background: 'var(--status-danger)', color: '#fff', border: 'none' }}
              >删除</button>
            </div>
          </div>
        </div>
      )}

      {/* ── 条件编辑弹窗 ── */}
      {conditionInput && (
        <div className="absolute inset-0 flex items-center justify-center z-[1000]" style={{ background: 'var(--bg-overlay)' }}>
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
                defaultValue={(() => {
                  const srcStage = stages.find(s => s.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target));
                  const existingEdge = srcStage?.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target);
                  return existingEdge?.condition || '';
                })()}
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
                defaultValue={(() => {
                  const srcStage = stages.find(s => s.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target));
                  const existingEdge = srcStage?.edges.find(e => e.source === conditionInput.source && e.target === conditionInput.target);
                  return existingEdge?.label || '';
                })()}
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

      {/* ── 门控编辑弹窗 ── */}
      {gateInput && (
        <div className="absolute inset-0 flex items-center justify-center z-[1000]" style={{ background: 'var(--bg-overlay)' }}>
          <div
            className="rounded-xl p-6 w-[90%] max-w-[400px]"
            style={{ background: 'var(--bg-tertiary)', border: '1px solid var(--border)', boxShadow: 'var(--shadow-lg)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-sm font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>编辑 [step{(stages.find(s => s.id === gateInput.stageId)?.order ?? 0) + 1} {stages.find(s => s.id === gateInput.stageId)?.name || gateInput.stageId}] 门控配置</h3>
            {gateError && (
              <div className="mb-3 px-3 py-2 rounded-lg text-[10px]" style={{ background: 'rgba(248,81,73,0.12)', color: '#f85149', border: '1px solid rgba(248,81,73,0.3)' }}>
                {gateError}
              </div>
            )}
            <div className="mb-4">
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>聚合策略</label>
              <select
                id="gate-strategy"
                className="w-full px-3 py-2 rounded-lg text-xs outline-none"
                defaultValue={stages.find(s => s.id === gateInput.stageId)?.gate.strategy || 'all'}
                  onChange={(e) => {
                    const val = e.target.value;
                    setGateStrategy(val);
                    if (val === 'threshold') {
                      // 阈值策略强制使用自定义合并，自动切换
                      const mergeSelect = document.getElementById('gate-merge') as HTMLSelectElement;
                      if (mergeSelect && mergeSelect.value !== 'custom') {
                        mergeSelect.value = 'custom';
                        mergeSelect.dispatchEvent(new Event('change'));
                      }
                    }
                  }}
                style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
              >
                <option value="all">全部执行成功</option>
                <option value="count">指定数量成功</option>
                <option value="threshold">合并运算判断</option>
              </select>
            </div>
            <div className="mb-4" style={{ display: gateStrategy === 'count' ? 'block' : 'none' }}>
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>完成节点数（指定数量完成策略）</label>
              <input
                id="gate-count"
                type="number"
                min="1"
                step="1"
                pattern="[0-9]*"
                className="w-full px-3 py-2 rounded-lg text-xs outline-none"
                defaultValue={stages.find(s => s.id === gateInput.stageId)?.gate.threshold ?? ''}
                style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                placeholder="例如: 3"
              />
            </div>
            <div className="mb-4" style={{ display: gateStrategy === 'threshold' ? 'block' : 'none' }}>
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>阈值（合并运算判断策略）</label>
              <div className="flex gap-2 items-center">
                <select
                  id="gate-threshold-op"
                  className="flex-shrink-0 px-3 py-1.5 rounded-lg text-xs outline-none"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                >
                  <option value=">=">大于等于</option>
                  <option value=">">大于</option>
                  <option value="==">等于</option>
                  <option value="<=">小于等于</option>
                  <option value="<">小于</option>
                </select>
                <input
                  id="gate-threshold"
                  type="text"
                  className="flex-1 px-3 py-1.5 rounded-lg text-xs outline-none"
                  defaultValue={''}
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                  placeholder="输入阈值，如 60"
                />
              </div>
            </div>
            <div className="mb-4">
              <label className="text-[10px] block mb-1" style={{ color: 'var(--text-tertiary)' }}>合并策略</label>
              <div className="flex gap-2">
                <select
                  id="gate-merge"
                  className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
                  defaultValue={stages.find(s => s.id === gateInput.stageId)?.gate.mergeStrategy || 'merge'}
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                  onChange={(e) => {
                    const isCustom = e.target.value === 'custom';
                    setShowCustomMode(isCustom);
                    const modeRow = document.getElementById('gate-custom-mode-row');
                    if (modeRow) modeRow.style.display = isCustom ? 'block' : 'none';
                    const modeSelect = document.getElementById('gate-custom-mode');
                    if (modeSelect) modeSelect.style.display = isCustom ? 'block' : 'none';
                  }}
                >
                  {gateStrategy !== 'threshold' && <option value="merge">合并为对象</option>}
                  {gateStrategy !== 'threshold' && <option value="concat">合并为数组</option>}
                  {gateStrategy !== 'threshold' && <option value="pick_first">取第一个结果</option>}
                  {gateStrategy !== 'threshold' && <option value="pick_last">取最后个结果</option>}
                  <option value="custom">自定义处理</option>
                </select>
                <select
                  id="gate-custom-mode"
                  className="w-[100px] px-2 py-2 rounded-lg text-xs outline-none"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', display: showCustomMode ? 'block' : 'none' }}
                  onChange={(e) => {
                    const mode = e.target.value;
                    setCustomMode(mode);
                  }}
                >
                  <option value="selector">选择器</option>
                  {gateStrategy !== 'threshold' && <option value="editor">编辑器</option>}
                </select>
              </div>
            </div>
            <div id="gate-custom-mode-row" className="mb-4" style={{ display: (showCustomMode || gateStrategy === 'threshold') ? 'block' : 'none' }}>
              <div id="gate-selector-area" style={{ display: (customMode === 'selector' || gateStrategy === 'threshold') ? 'block' : 'none' }}>
                <div className="flex gap-2 mb-2">
                  <div className="flex-1">
                    <label className="text-[9px] block mb-0.5" style={{ color: 'var(--text-tertiary)' }}>过滤</label>
                    <select id="gate-script-filter" className="w-full px-2 py-1.5 rounded-lg text-[10px] outline-none" style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}>
                      <option value="all">保留全部</option>
                      <option value="success">只保留成功</option>
                    </select>
                  </div>
                  <div className="flex-1">
                    <label className="text-[9px] block mb-0.5" style={{ color: 'var(--text-tertiary)' }}>合并为</label>
                    <select id="gate-script-merge" className="w-full px-2 py-1.5 rounded-lg text-[10px] outline-none" style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}>
                      <option value="object">合并成一个对象</option>
                      <option value="array">合并成一个数组</option>
                      <option value="flat">展开成一维数组</option>
                    </select>
                  </div>
                  <div className="flex-1">
                    <label className="text-[9px] block mb-0.5" style={{ color: 'var(--text-tertiary)' }}>取值</label>
                    <select id="gate-script-value" className="w-full px-2 py-1.5 rounded-lg text-[10px] outline-none" style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}>
                      <option value="none">保留原始值</option>
                      <option value="max">取最大值</option>
                      <option value="min">取最小值</option>
                      <option value="avg">取平均值</option>
                      <option value="sum">计算总和</option>
                    </select>
                  </div>
                </div>
              </div>
              <div id="gate-editor-area" style={{ display: customMode === 'editor' ? 'block' : 'none' }}>
                <textarea
                  id="gate-script"
                  rows={3}
                  className="w-full px-3 py-2 rounded-lg text-xs outline-none resize-none font-mono"
                  defaultValue={stages.find(s => s.id === gateInput.stageId)?.gate.customScript ?? ''}
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}
                  placeholder="在此编写自定义脚本，例如: (results) =&gt; results.map(r =&gt; r.data)"
                />
                <div className="mt-2 p-2 rounded-lg text-[10px] leading-relaxed" style={{ background: 'var(--bg-secondary)', border: '1px solid var(--border)', color: 'var(--text-tertiary)', maxHeight: '180px', overflowY: 'auto' }}>
                  <div className="font-semibold mb-1" style={{ color: 'var(--text-secondary)' }}>参数说明</div>
                  <div><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results</code> — 上游节点输出数组，需编写完整函数: <code className="text-[10px]" style={{ color: 'var(--accent)' }}>(results) =&gt; {'{ /* 你的代码 */ }'}</code></div>
                  <div className="ml-3">每个元素: <code className="text-[10px]" style={{ color: 'var(--accent)' }}>{'{ data, success, nodeId, nodeName }'}</code></div>
                  <div className="mt-1 font-semibold" style={{ color: 'var(--text-secondary)' }}>常用数组方法</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.map(r =&gt; r.data)</code> — 遍历转换，提取所有数据</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.filter(r =&gt; r.success)</code> — 过滤，只保留符合条件的元素</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.find(r =&gt; r.success)</code> — 查找，返回第一个匹配的元素</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.findIndex(r =&gt; r.success)</code> — 查找索引，返回第一个匹配的位置</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.some(r =&gt; r.success)</code> — 判断是否有任意元素满足条件</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.every(r =&gt; r.success)</code> — 判断是否所有元素满足条件</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.reduce((acc, r) =&gt; Object.assign(acc, r.data), {'{}'})</code> — 归并，累积合并为单个值/对象</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.flatMap(r =&gt; r.data)</code> — 遍历展平，将嵌套数组展开一层</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.sort((a, b) =&gt; b.data - a.data)</code> — 排序，按数值降序排列</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.includes(target)</code> — 判断数组是否包含指定值</div>
                  <div className="ml-3"><code className="text-[10px]" style={{ color: 'var(--accent)' }}>results.slice(0, 3)</code> — 截取，取前 N 个元素</div>
                  <div className="mt-1" style={{ color: 'var(--text-tertiary)' }}>提示: 函数体最后一行即为返回值，无需 return 关键字（箭头函数简写）。</div>
                </div>
              </div>
            </div>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => { setGateError(null); setGateInput(null); }}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
              >取消</button>
              <button
                onClick={() => {
                  const strategy = (document.getElementById('gate-strategy') as HTMLSelectElement)?.value;
                  let mergeStrategy = (document.getElementById('gate-merge') as HTMLSelectElement)?.value as 'merge' | 'concat' | 'pick_first' | 'pick_last' | 'custom';
                  // 阈值策略强制使用自定义合并
                  if (strategy === 'threshold') {
                    mergeStrategy = 'custom';
                  }
                  const countInput = (document.getElementById('gate-count') as HTMLInputElement)?.value;
                  const thresholdOp = (document.getElementById('gate-threshold-op') as HTMLSelectElement)?.value || '>=';
                  const thresholdInput = (document.getElementById('gate-threshold') as HTMLInputElement)?.value;
                  const customMode = (document.getElementById('gate-custom-mode') as HTMLSelectElement)?.value || 'selector';
                  let customScript: string | undefined;
                  if (mergeStrategy === 'custom') {
                    if (customMode === 'editor') {
                      customScript = (document.getElementById('gate-script') as HTMLTextAreaElement)?.value || undefined;
                    } else {
                      const filter = (document.getElementById('gate-script-filter') as HTMLSelectElement)?.value || 'all';
                      const mergeAs = (document.getElementById('gate-script-merge') as HTMLSelectElement)?.value || 'array';
                      const valueOp = (document.getElementById('gate-script-value') as HTMLSelectElement)?.value || 'none';
                      // 自动生成脚本
                      let lines: string[] = [];
                      if (filter === 'success') {
                        lines.push('  const filtered = results.filter(r => r.success);');
                      } else {
                        lines.push('  const filtered = results;');
                      }
                      // 生成 calc: 前缀格式（Rust 后端可直接解析执行）
                      // 格式: calc:<filter>:<merge_as>:<value_op>
                      customScript = `calc:${filter}:${mergeAs}:${valueOp}`;
                    }
                  }
                  // 校验
                  if (strategy === 'count' && (!countInput || !Number.isInteger(Number(countInput)) || parseInt(countInput, 10) < 1)) {
                    setGateError('请填写有效的完成节点数（正整数，至少为 1）');
                    return;
                  }
                  if (strategy === 'threshold' && (!thresholdInput || isNaN(Number(thresholdInput)))) {
                    setGateError('请填写有效的阈值');
                    return;
                  }
                  if (mergeStrategy === 'custom' && !customScript?.trim()) {
                    setGateError('请配置自定义脚本（选择器模式或编辑器模式）');
                    return;
                  }
                  const threshold = strategy === 'count'
                    ? (countInput ? parseInt(countInput, 10) : undefined)
                    : (thresholdInput ? `${thresholdOp} ${thresholdInput}`.trim() : undefined);
                  setGateError(null);
                  handleUpdateGate(gateInput.stageId, { strategy, mergeStrategy, threshold, customScript: customScript || undefined, customMode: mergeStrategy === 'custom' ? customMode : undefined });
                  setGateInput(null);
                }}
                className="pd-btn px-4 py-1.5 text-xs rounded"
                style={{ background: 'var(--accent)', color: '#fff', border: 'none' }}
              >确认</button>
            </div>
          </div>
        </div>
      )}

      {/* ── 节点配置面板 ── */}
      {selectedNodeId && selectedStageId && stages.some(s => s.id === selectedStageId && s.nodes.some(n => n.id === selectedNodeId)) && (
        <div
          className="w-[360px] shrink-0 overflow-auto p-5 flex flex-col"
          style={{ background: 'var(--bg-secondary)', borderLeft: '1px solid var(--border)' }}
        >
      )}
      </div>{/* end flex-row: canvas + panel */}
    </div>
  );
};
