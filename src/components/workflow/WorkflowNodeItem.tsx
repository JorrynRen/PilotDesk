import React from 'react';
import type { WorkflowNode } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';

const NODE_W = 160;
const NODE_H = 60;

interface WorkflowNodeItemProps {
  node: WorkflowNode;
  stageId: string;
  scale: number;
  selectedNodeId: string | null;
  selectedNodeIds: Set<string>;
  draggingNode: { nodeId: string; started: boolean } | null;
  hoveredNodeId: string | null;
  connectTargetId: string | null;
  cycleTargetId: string | null;
  connecting: unknown;
  stepStates: Record<string, string>;
  nodeResults: Record<string, any>;
  selectedNodeId: string | null;
  isRestoredResult?: boolean;
  isConfigChanged?: boolean;
  onSelectNode: (nodeId: string, stageId: string, ctrlKey: boolean) => void;
  onNodeMouseDown: (e: React.MouseEvent, nodeId: string, stageId: string) => void;
  onDeleteNode: (nodeId: string) => void;
  onEndConnect: (nodeId: string, stageId: string) => void;
  onStartConnect: (e: React.MouseEvent, nodeId: string, stageId: string) => void;
  onHoverNode: (nodeId: string | null) => void;
}

const WorkflowNodeItem: React.FC<WorkflowNodeItemProps> = React.memo(({
  node, stageId, scale,
  selectedNodeId, selectedNodeIds,
  draggingNode, hoveredNodeId, connectTargetId, cycleTargetId,
  connecting, stepStates, nodeResults,
  isRestoredResult, isConfigChanged,
  onSelectNode, onNodeMouseDown, onDeleteNode,
  onEndConnect, onStartConnect, onHoverNode,
}) => {
  const meta = getNodeTypeMeta(node.type);
  const pos = node.position as { x: number; y: number } | undefined;
  const isSelected = selectedNodeId === node.id;
  const isDraggingThis = draggingNode?.nodeId === node.id && draggingNode?.started;
  const isHovered = hoveredNodeId === node.id;
  const isConnectTarget = connectTargetId === node.id;
  const isCycleTarget = cycleTargetId === node.id;
  const runState = stepStates[`node_${node.id}`];
  const nodeResult = nodeResults[`node_${node.id}`];
  const showResult = (selectedNodeId === node.id || selectedNodeIds.has(node.id)) && nodeResult !== undefined;

  return (
    <div
      data-node
      data-node-id={node.id}
      onClick={(e) => {
        e.stopPropagation();
        onSelectNode(node.id, stageId, e.ctrlKey || e.metaKey);
      }}
      onMouseDown={(e) => onNodeMouseDown(e, node.id, stageId)}
      onMouseEnter={() => onHoverNode(node.id)}
      onMouseLeave={() => onHoverNode(null)}
      className="cursor-grab active:cursor-grabbing"
      style={{
        position: 'absolute',
        left: pos?.x ?? 20,
        top: pos?.y ?? 20,
        width: NODE_W,
        height: NODE_H,
        boxSizing: 'border-box',
        padding: '12px 10px',
        borderRadius: 8,
        border: selectedNodeIds.has(node.id)
          ? '2px solid var(--accent)'
          : isSelected
            ? '1.5px solid var(--accent)'
            : isHovered
              ? '1px solid var(--accent)'
              : '1.5px solid var(--border)',
        background: isDraggingThis
          ? 'var(--bg-tertiary)'
          : runState === 'running'
            ? '#f59e0b15'
            : runState === 'success'
              ? '#3fb95010'
              : runState === 'failed'
                ? '#f8514910'
                : 'var(--bg-tertiary)',
        boxShadow: isDraggingThis
          ? '0 8px 24px rgba(0,0,0,0.4), 0 0 0 1px var(--accent)'
          : selectedNodeIds.has(node.id) && !isSelected
            ? '0 0 0 2px var(--accent-light), var(--shadow-md)'
            : isSelected
              ? '0 0 0 1px var(--accent-light), var(--shadow-sm)'
            : isHovered
              ? 'var(--shadow-md)'
              : runState === 'running'
                ? 'inset 0 0 12px #f59e0b66, inset 0 0 24px #f59e0b33'
                : 'var(--shadow-sm)',
        animation: runState === 'running' ? 'pulseGlow 1.5s ease-in-out infinite' : 'none',
        zIndex: isDraggingThis ? 20 : 2,
        userSelect: 'none',
        transformOrigin: '80px 30px',
        transform: `scale(${isDraggingThis ? 1.03 / scale : 1 / scale})`,
        opacity: isDraggingThis ? 0.92 : 1,
        transition: isDraggingThis ? 'none' : 'box-shadow 0.15s ease, border-color 0.15s ease, transform 0.15s ease',
      }}
    >
      {/* 节点内容行：图标 + 标签 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <div
          className="shrink-0"
          style={{
            width: 36, height: 36, borderRadius: 6,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            fontSize: 20,
            background: `${meta.color}22`,
            color: meta.color,
          }}
        >
          {meta.icon}
        </div>
        <span className="text-xs font-semibold truncate" style={{ color: 'var(--text-primary)' }}>
          {node.label}
        </span>
        {runState === 'success' && (
          <span className="text-[10px] ml-auto shrink-0" style={{ color: '#3fb950' }}>✓</span>
        )}
        {runState === 'failed' && (
          <span className="text-[10px] ml-auto shrink-0" style={{ color: '#f85149' }}>✗</span>
        )}
      </div>

      {/* 执行结果面板（选中节点时显示） */}
      {showResult && (
        <div
          className="absolute"
          style={{
            left: 0,
            top: NODE_H + 4,
            width: NODE_W,
            maxHeight: 120,
            overflow: 'hidden',
            borderRadius: 6,
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            padding: '6px 8px',
            fontSize: 10,
            lineHeight: 1.5,
            color: 'var(--text-secondary)',
            zIndex: 10,
            boxShadow: 'var(--shadow-md)',
          }}
        >
          <div style={{ fontWeight: 600, marginBottom: 2, color: 'var(--text-primary)', fontSize: 9, display: 'flex', alignItems: 'center', gap: 4 }}>
            {isRestoredResult ? (
              <>
                <span style={{ color: '#d29922' }}>&#9650;</span>
                <span>历史执行结果</span>
              </>
            ) : runState === 'success' ? '执行结果' : runState === 'failed' ? '错误信息' : '执行输出'}
          </div>
          {isRestoredResult && isConfigChanged && (
            <div style={{ marginBottom: 2, padding: '2px 4px', borderRadius: 3, fontSize: 9, background: '#d2992222', color: '#d29922', border: '1px solid #d2992244' }}>
              配置已变更
            </div>
          )}
          <div style={{
            maxHeight: 90,
            overflowY: 'auto',
            wordBreak: 'break-all',
            whiteSpace: 'pre-wrap',
            fontFamily: 'var(--font-mono)',
          }}>
            {typeof nodeResult === 'object'
              ? JSON.stringify(nodeResult, null, 2)
              : String(nodeResult)}
          </div>
        </div>
      )}

      {/* 输入锚点 */}
      {meta.canHaveInputs && (
        <div
          data-anchor="input"
          onMouseDown={(e) => {
            e.stopPropagation();
            if (connecting) onEndConnect(node.id, stageId);
          }}
          className="absolute rounded-full transition-all duration-150"
          style={{
            left: -5, top: '50%', marginTop: -5,
            width: (connecting && (isConnectTarget || isCycleTarget)) ? 12 : 10,
            height: (connecting && (isConnectTarget || isCycleTarget)) ? 12 : 10,
            background: (connecting && isCycleTarget) ? 'var(--status-danger)'
              : (connecting && isConnectTarget) ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-secondary)',
            cursor: (connecting && isConnectTarget) ? 'cell' : (connecting && isCycleTarget) ? 'not-allowed' : 'crosshair',
            boxShadow: (connecting && isCycleTarget) ? '0 0 8px var(--status-danger)'
              : (connecting && isConnectTarget) ? '0 0 8px var(--accent-light)' : 'none',
          }}
        />
      )}

      {/* 输出锚点 */}
      {meta.canHaveOutputs && (
        <div
          data-anchor="output"
          onMouseDown={(e) => {
            e.stopPropagation();
            onStartConnect(e, node.id, stageId);
          }}
          className="absolute rounded-full transition-all duration-150"
          style={{
            right: -5, top: '50%', marginTop: -5,
            width: isHovered ? 12 : 10,
            height: isHovered ? 12 : 10,
            background: isHovered ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-secondary)',
            cursor: 'crosshair',
            boxShadow: isHovered ? '0 0 8px var(--accent-light)' : 'none',
          }}
        />
      )}

      {/* 删除按钮 */}
      {!meta.isBoundary && (
        <div
          onClick={(e) => { e.stopPropagation(); onDeleteNode(node.id); }}
          className="absolute flex items-center justify-center rounded-full cursor-pointer"
          style={{
            top: -6, right: -6, width: 16, height: 16,
            background: 'var(--status-danger)', color: '#fff', fontSize: 10,
            opacity: isHovered ? 0.9 : 0,
            transition: 'opacity 0.15s ease',
          }}
        >x</div>
      )}
    </div>
  );
}, (prev, next) => {
  if (prev.node !== next.node) return false;
  if (prev.stageId !== next.stageId) return false;
  if (prev.scale !== next.scale) return false;
  if (prev.selectedNodeId !== next.selectedNodeId) return false;
  if (prev.hoveredNodeId !== next.hoveredNodeId) return false;
  if (prev.connectTargetId !== next.connectTargetId) return false;
  if (prev.draggingNode?.nodeId !== next.draggingNode?.nodeId) return false;
  if (prev.draggingNode?.started !== next.draggingNode?.started) return false;
  if (prev.stepStates[`node_${prev.node.id}`] !== next.stepStates[`node_${next.node.id}`]) return false;
  if (prev.selectedNodeIds.has(prev.node.id) !== next.selectedNodeIds.has(next.node.id)) return false;
  if (!!prev.connecting !== !!next.connecting) return false;
  return true;
});

export default WorkflowNodeItem;
