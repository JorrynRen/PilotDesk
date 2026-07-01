import React, { useState, useRef, useEffect } from 'react';
import { createPortal } from 'react-dom';
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
  reachableTargets: Set<string>;
  connecting: unknown;
  stepStates: Record<string, string>;
  nodeResults: Record<string, any>;
  selectedNodeId: string | null;
  isUnreachable?: boolean;
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
  draggingNode, hoveredNodeId, connectTargetId, cycleTargetId, reachableTargets,
  connecting, stepStates, nodeResults,
  isUnreachable,
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
  const isReachableTarget = !!connecting && reachableTargets.has(node.id);
  const runState = stepStates[`node_${node.id}`];
  const nodeResult = nodeResults[`node_${node.id}`];
  const [expanded, setExpanded] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);

  // 监听全屏状态变化
  useEffect(() => {
    const handler = () => setIsFullscreen(!!document.fullscreenElement);
    document.addEventListener('fullscreenchange', handler);
    return () => document.removeEventListener('fullscreenchange', handler);
  }, []);
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
        border: isUnreachable
          ? '2px dashed #f85149'
          : selectedNodeIds.has(node.id)
            ? '2px solid var(--accent)'
            : isSelected
              ? '1.5px solid var(--accent)'
              : isHovered
                ? '1px solid var(--accent)'
                : '1.5px solid var(--text-tertiary)',
        background: isDraggingThis
          ? 'var(--bg-tertiary)'
          : runState === 'running'
            ? '#3b82f615'
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
                ? 'inset 0 0 12px #3b82f666, inset 0 0 24px #2563eb44'
                : 'var(--shadow-sm)',
        animation: runState === 'running' ? 'pulseGlow 1.5s ease-in-out infinite' : 'none',
        zIndex: isDraggingThis ? 20 : showResult ? 30 : 2,
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
        <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
          {node.label}
        </span>
        {(runState === 'success' || runState === 'failed') && (
          <span
            className="ml-auto shrink-0"
            style={{
              width: 16,
              height: 16,
              borderRadius: '50%',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 9,
              lineHeight: 1,
              fontWeight: 700,
              color: '#fff',
              background: runState === 'success'
                ? 'linear-gradient(135deg, #3fb950, #2ea043)'
                : 'linear-gradient(135deg, #f85149, #da3633)',
              boxShadow: runState === 'success'
                ? '0 1px 3px rgba(63,185,80,0.4)'
                : '0 1px 3px rgba(248,81,73,0.4)',
            }}
          >
            {runState === 'success' ? '✓' : '✗'}
          </span>
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
            zIndex: 50,
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
              ? JSON.stringify(Object.fromEntries(
                  Object.entries(nodeResult).filter(([k]) => k !== 'agent_session_id')
                ), null, 2)
              : String(nodeResult)}
          </div>
          {/* 放大查看按钮 */}
          <div
            onClick={(e) => { e.stopPropagation(); setExpanded(true); }}
            title="放大查看"
            style={{
              position: 'absolute',
              bottom: 2,
              right: 2,
              width: 14,
              height: 14,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              borderRadius: 3,
              fontSize: 9,
              lineHeight: 1,
              color: 'var(--text-tertiary)',
              cursor: 'pointer',
              opacity: 0.5,
              transition: 'opacity 0.15s',
              userSelect: 'none',
            }}
            onMouseEnter={(e) => { (e.target as HTMLElement).style.opacity = '1'; }}
            onMouseLeave={(e) => { (e.target as HTMLElement).style.opacity = '0.5'; }}
          >⤢</div>
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
            width: 10, height: 10,
            left: -5, top: '50%', marginTop: -5,
            transform: isConnectTarget ? 'scale(1.5)' : 'scale(1)',
            background: isCycleTarget ? 'var(--status-danger)'
              : isConnectTarget ? 'var(--accent)'
              : isReachableTarget ? '#3fb950'
              : 'var(--border)',
            border: '2px solid var(--bg-secondary)',
            cursor: isConnectTarget ? 'cell' : isCycleTarget ? 'not-allowed' : 'crosshair',
            boxShadow: isCycleTarget ? '0 0 8px var(--status-danger)'
              : isConnectTarget ? '0 0 8px var(--accent-light)'
              : isReachableTarget ? '0 0 6px #3fb95088' : 'none',
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
            width: 10, height: 10,
            right: -5, top: '50%', marginTop: -5,
            transform: (!connecting && isHovered) ? 'scale(1.5)' : 'scale(1)',
            background: (!connecting && isHovered) ? 'var(--accent)' : 'var(--border)',
            border: '2px solid var(--bg-secondary)',
            cursor: 'crosshair',
            boxShadow: (!connecting && isHovered) ? '0 0 8px var(--accent-light)' : 'none',
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
      {/* 放大查看弹窗 — 通过 Portal 渲染到 body 层级，避免画布 transform 影响 */}
      {expanded && createPortal(
        <div
          onClick={() => setExpanded(false)}
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 10000,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: 'rgba(0,0,0,0.4)',
          }}
        >
          <div
            ref={modalRef}
            onClick={(e) => e.stopPropagation()}
            style={{
              position: 'relative',
              width: 640,
              height: 480,
              minWidth: 320,
              minHeight: 200,
              maxWidth: '90vw',
              maxHeight: '90vh',
              display: 'flex',
              flexDirection: 'column',
              borderRadius: 8,
              background: 'var(--bg-primary)',
              border: '1px solid var(--border)',
              boxShadow: '0 8px 32px rgba(0,0,0,0.3)',
              resize: 'both',
              overflow: 'hidden',
            }}
          >
            {/* 标题栏 */}
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                padding: '6px 10px',
                borderBottom: '1px solid var(--border)',
                background: 'var(--bg-secondary)',
                borderRadius: '8px 8px 0 0',
                flexShrink: 0,
                userSelect: 'none',
              }}
            >
              <span style={{ fontSize: 11, fontWeight: 600, color: 'var(--text-primary)' }}>
                {node.label} - 执行结果
              </span>
              <div style={{ marginLeft: 'auto', display: 'flex', gap: 2 }}>
                {/* 全屏/还原按钮 */}
                <button
                  onClick={() => {
                    if (isFullscreen) {
                      document.exitFullscreen();
                    } else {
                      modalRef.current?.requestFullscreen?.();
                    }
                  }}
                  title={isFullscreen ? '还原窗口' : '全屏'}
                  style={{
                    padding: '2px 6px',
                    borderRadius: 4,
                    border: 'none',
                    background: 'transparent',
                    color: 'var(--text-tertiary)',
                    cursor: 'pointer',
                    fontSize: 11,
                    lineHeight: 1,
                  }}
                >{isFullscreen ? '[X]' : '[ ]'}</button>
                {/* 关闭按钮 */}
                <button
                  onClick={() => setExpanded(false)}
                  title="关闭"
                  style={{
                    padding: '2px 6px',
                    borderRadius: 4,
                    border: 'none',
                    background: 'transparent',
                    color: 'var(--text-tertiary)',
                    cursor: 'pointer',
                    fontSize: 13,
                    lineHeight: 1,
                  }}
                >x</button>
              </div>
            </div>
            {/* 内容区 — HTML 渲染 */}
            <div
              onWheel={(e) => e.stopPropagation()}
              onMouseDown={(e) => e.stopPropagation()}
              onMouseUp={(e) => e.stopPropagation()}
              style={{
                flex: 1,
                padding: 10,
                overflow: 'auto',
                fontFamily: 'var(--font-mono)',
                fontSize: 13,
                lineHeight: 1.6,
                color: 'var(--text-primary)',
              }}
              dangerouslySetInnerHTML={{
                __html: (typeof nodeResult === 'object'
                  ? JSON.stringify(Object.fromEntries(
                      Object.entries(nodeResult).filter(([k]) => k !== 'agent_session_id')
                    ), null, 2)
                  : String(nodeResult)
                )
                  .replace(/\\n/g, '\n')
                  .replace(/\\r/g, '')
                  .replace(/\\"/g, '"')
                  .replace(/\\'/g, "'")
                  .replace(/\\t/g, '  ')
                  .split('\n').join('<br>')
                  .replace(/  /g, '&nbsp;&nbsp;')
              }}
            />
            {/* 右下角缩放提示 */}
            <div style={{
                position: 'absolute',
                bottom: 2,
                right: 2,
                fontSize: 9,
                color: 'var(--text-tertiary)',
                userSelect: 'none',
                pointerEvents: 'none',
              }}>~</div>
          </div>
        </div>,
        document.body
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
  if (!!prev.isUnreachable !== !!next.isUnreachable) return false;
  if (!!prev.connecting !== !!next.connecting) return false;
  return true;
});

export default WorkflowNodeItem;
