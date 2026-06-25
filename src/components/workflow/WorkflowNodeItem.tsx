import React from 'react';
import type { WorkflowNode } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import { NODE_W, NODE_H } from './WorkflowEditor';

interface WorkflowNodeItemProps {
  node: WorkflowNode;
  stageId: string;
  scale: number;
  selectedNodeId: string | null;
  selectedNodeIds: Set<string>;
  draggingNode: { nodeId: string; started: boolean } | null;
  hoveredNodeId: string | null;
  connectTargetId: string | null;
  connecting: unknown;
  stepStates: Record<string, string>;
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
  draggingNode, hoveredNodeId, connectTargetId,
  connecting, stepStates,
  onSelectNode, onNodeMouseDown, onDeleteNode,
  onEndConnect, onStartConnect, onHoverNode,
}) => {
  const meta = getNodeTypeMeta(node.type);
  const pos = node.position as { x: number; y: number } | undefined;
  const isSelected = selectedNodeId === node.id;
  const isDraggingThis = draggingNode?.nodeId === node.id && draggingNode?.started;
  const isHovered = hoveredNodeId === node.id;
  const isConnectTarget = connectTargetId === node.id;
  const runState = stepStates[`node_${node.id}`];

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
              : '1.5px solid #484f58',
        background: isDraggingThis
          ? 'var(--bg-tertiary)'
          : runState === 'running'
            ? '#58a6ff15'
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
                ? '0 0 12px #58a6ff44'
                : 'var(--shadow-sm)',
        zIndex: isDraggingThis ? 20 : 2,
        userSelect: 'none',
        transformOrigin: '80px 30px',
        transform: `scale(${isDraggingThis ? 1.03 / scale : 1 / scale})`,
        opacity: isDraggingThis ? 0.92 : 1,
        transition: isDraggingThis ? 'none' : 'box-shadow 0.15s ease, border-color 0.15s ease, transform 0.15s ease',
      }}
    >
      {runState === 'running' && (
        <div className="absolute inset-0 rounded-lg overflow-hidden pointer-events-none" style={{ zIndex: -1 }}>
          <div className="absolute inset-0" style={{
            border: '2px solid #58a6ff',
            borderRadius: 8,
            animation: 'spin 1.5s linear infinite',
            clipPath: 'polygon(0 0, 100% 0, 100% 50%, 0 50%)',
            opacity: 0.5,
          }} />
        </div>
      )}

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
            width: (connecting && isConnectTarget) ? 12 : 10,
            height: (connecting && isConnectTarget) ? 12 : 10,
            background: (connecting && isConnectTarget) ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-secondary)',
            cursor: (connecting && isConnectTarget) ? 'cell' : 'crosshair',
            boxShadow: (connecting && isConnectTarget) ? '0 0 8px var(--accent-light)' : 'none',
          }}
        />
      )}

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
