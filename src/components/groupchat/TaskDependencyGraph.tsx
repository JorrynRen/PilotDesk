import { useId, useMemo, useState } from 'react';
import type { GroupChatTask, TaskStatus } from '../../types/groupchat';
import {
  buildTaskGraph,
  FULL_GRAPH_METRICS,
  MINI_GRAPH_METRICS,
  taskPhase,
  type TaskGraphSelection,
  type TaskPhase,
} from '../../utils/taskGraph';

interface Props {
  tasks: GroupChatTask[];
  variant: 'mini' | 'full';
  selectedId?: string | null;
  onSelect?: (selection: TaskGraphSelection) => void;
  /** 缩略图点击任意节点 → 打开放大图（起止节点也走这里）。 */
  onOpenFull?: (task?: GroupChatTask) => void;
  /** 全屏图的缩放倍数（0.5–2.0）；缩略图不使用。 */
  scale?: number;
  /** 起点/终点节点的一句话摘要（总目标 / 最终结论首行），仅放大图展示。 */
  endpointText?: { start?: string; end?: string };
  /** 最终结论是否已产出：决定终点节点的配色与虚线。 */
  concluded?: boolean;
}

function nodePaint(status: TaskStatus, phase: TaskPhase) {
  switch (status) {
    case 'success':
      return { fill: 'var(--status-success-bg)', stroke: 'var(--status-success)', dash: undefined, opacity: 1 };
    case 'failed':
      return { fill: 'var(--status-danger-bg)', stroke: 'var(--status-danger)', dash: undefined, opacity: 1 };
    case 'skipped':
      return { fill: 'var(--status-warning-bg)', stroke: 'var(--status-warning)', dash: '4 3', opacity: 1 };
    case 'aborted':
      return { fill: 'var(--bg-tertiary)', stroke: 'var(--text-tertiary)', dash: '4 3', opacity: 0.72 };
    case 'running':
      return { fill: 'var(--accent-light)', stroke: 'var(--accent)', dash: undefined, opacity: 1 };
    case 'discussing':
      return { fill: 'var(--bg-tertiary)', stroke: 'var(--accent)', dash: '3 3', opacity: 1 };
    default:
      return {
        fill: 'var(--bg-tertiary)',
        stroke: phase === 'ready' ? 'var(--accent)' : 'var(--border)',
        dash: undefined,
        opacity: 1,
      };
  }
}

export function TaskDependencyGraph({
  tasks,
  variant,
  selectedId,
  onSelect,
  onOpenFull,
  scale = 1,
  endpointText,
  concluded = false,
}: Props) {
  const rawArrowId = useId();
  const arrowId = `pd-task-graph-arrow-${rawArrowId.replace(/[^a-zA-Z0-9_-]/g, '')}`;
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const metrics = variant === 'mini' ? MINI_GRAPH_METRICS : FULL_GRAPH_METRICS;
  const layout = useMemo(() => buildTaskGraph(tasks, metrics), [tasks, metrics]);

  const relatedIds = useMemo(() => {
    if (!hoveredId) return null;
    const ids = new Set<string>([hoveredId]);
    layout.edges.forEach((edge) => {
      if (edge.fromId === hoveredId) ids.add(edge.toId);
      if (edge.toId === hoveredId) ids.add(edge.fromId);
    });
    return ids;
  }, [hoveredId, layout.edges]);

  if (layout.nodes.length === 0) return null;

  const compact = variant === 'mini';
  const fontSize = compact ? 11 : 11;
  const paddingX = compact ? 6 : 10;
  // 节点尺寸有上限：不再让 SVG 拉满容器，否则任务很少时节点会被放大到失真。
  // 缩略图固定 1.35 倍上限；全屏图按 scale 缩放，超过 1 时允许溢出滚动。
  const renderedWidth = compact ? layout.width * 1.35 : layout.width * scale;

  return (
    <svg
      viewBox={`0 0 ${layout.width} ${layout.height}`}
      role="img"
      aria-label={`子任务依赖图，共 ${tasks.length} 个任务`}
      style={{
        display: 'block',
        width: renderedWidth,
        maxWidth: compact || scale <= 1 ? '100%' : 'none',
        height: 'auto',
        margin: '0 auto',
      }}
    >
      <defs>
        <marker
          id={arrowId}
          viewBox="0 0 8 8"
          refX="7"
          refY="4"
          markerWidth="8"
          markerHeight="8"
          markerUnits="userSpaceOnUse"
          orient="auto"
        >
          <path d="M1 1 L7 4 L1 7 Z" fill="var(--text-tertiary)" />
        </marker>
      </defs>
      {layout.edges.map((edge, i) => {
        const active = hoveredId !== null && (edge.fromId === hoveredId || edge.toId === hoveredId);
        return (
          <path
            key={`${edge.fromId}-${edge.toId}-${i}`}
            d={edge.d}
            fill="none"
            stroke={active ? 'var(--accent)' : 'var(--border)'}
            strokeWidth={active ? 2 : 1.2}
            strokeDasharray={edge.dashed ? '4 3' : undefined}
            strokeLinecap="round"
            strokeLinejoin="round"
            markerEnd={`url(#${arrowId})`}
            style={{ opacity: hoveredId && !active ? 0.35 : 1, transition: 'opacity 120ms ease, stroke 120ms ease' }}
          />
        );
      })}
      {layout.nodes.map((node) => {
        const dim = relatedIds !== null && !relatedIds.has(node.id);
        const selected = selectedId === node.id;
        const common = {
          key: node.id,
          transform: `translate(${node.x},${node.y})`,
          role: 'button' as const,
          tabIndex: 0,
          onMouseEnter: () => setHoveredId(node.id),
          onMouseLeave: () => setHoveredId(null),
          onFocus: () => setHoveredId(node.id),
          onBlur: () => setHoveredId(null),
          style: { cursor: 'pointer', opacity: dim ? 0.5 : 1, transition: 'opacity 120ms ease' },
        };

        // 起止节点：缩略图只画图标胶囊，放大图补上「总目标 / 最终结论」与小段摘要。
        if (node.kind !== 'task') {
          const isStart = node.kind === 'start';
          const title = isStart ? '总目标' : '最终结论';
          const raw = (isStart ? endpointText?.start : endpointText?.end) ?? '';
          const single = raw.replace(/\s+/g, ' ').trim();
          const sub = single.length <= 9 ? single : `${single.slice(0, 9)}…`;
          const stroke = selected
            ? 'var(--accent)'
            : isStart
              ? 'var(--accent)'
              : concluded
                ? 'var(--status-success)'
                : 'var(--text-tertiary)';
          const fill = isStart
            ? 'var(--bg-tertiary)'
            : concluded
              ? 'var(--status-success-bg)'
              : 'var(--bg-tertiary)';
          const pick = () => {
            if (compact) onOpenFull?.();
            else onSelect?.(isStart ? { kind: 'start' } : { kind: 'end' });
          };
          return (
            <g
              {...common}
              aria-label={isStart ? '起点：总目标' : `终点：最终结论${concluded ? '' : '（待收敛）'}`}
              onClick={pick}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  pick();
                }
              }}
            >
              <rect
                width={node.width}
                height={node.height}
                rx={compact ? 4 : 7}
                fill={fill}
                stroke={stroke}
                strokeWidth={selected ? 2 : 1}
                strokeDasharray={isStart ? undefined : concluded ? undefined : '4 3'}
              />
              {compact ? (
                <text
                  x={node.width / 2}
                  y={node.height / 2 + 4}
                  textAnchor="middle"
                  fill="var(--text-secondary)"
                  fontSize={10}
                >
                  {isStart ? '起' : '终'}
                </text>
              ) : (
                <>
                  <text x={paddingX} y={20} fill="var(--text-secondary)" fontSize={fontSize} fontWeight={600}>
                    {title}
                  </text>
                  <text x={paddingX} y={38} fill="var(--text-primary)" fontSize={fontSize}>
                    {sub || '—'}
                  </text>
                </>
              )}
            </g>
          );
        }

        const task = node.task!;
        const phase = taskPhase(task, tasks);
        const paint = nodePaint(task.status, phase);
        const showPhaseMark = phase === 'ready' || phase === 'waiting';
        const label = compact || task.description.length <= 9
          ? task.description
          : `${task.description.slice(0, 9)}…`;
        return (
          <g
            {...common}
            aria-label={`T${task.taskNo} ${task.description}，${phase === 'finished' ? '已结束' : phase === 'running' ? '执行中' : phase === 'ready' ? '就绪' : '等待'}`}
            onClick={() => (compact ? onOpenFull?.(task) : onSelect?.({ kind: 'task', task }))}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                if (compact) onOpenFull?.(task);
                else onSelect?.({ kind: 'task', task });
              }
            }}
          >
            <rect
              width={node.width}
              height={node.height}
              rx={compact ? 4 : 7}
              fill={paint.fill}
              stroke={selected ? 'var(--accent)' : paint.stroke}
              strokeWidth={selected ? 2 : 1}
              strokeDasharray={paint.dash}
              style={{ opacity: paint.opacity }}
            />
            <text
              x={paddingX}
              y={compact ? node.height / 2 + 4 : 20}
              fill="var(--text-secondary)"
              fontSize={fontSize}
              style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', fontWeight: 500 }}
            >
              T{task.taskNo}
            </text>
            {!compact && (
              <text x={paddingX} y={38} fill="var(--text-primary)" fontSize={fontSize}>
                {label}
              </text>
            )}
            {showPhaseMark && (
              <circle
                cx={node.width - (compact ? 6 : 9)}
                cy={compact ? 6 : 9}
                r={compact ? 2.5 : 3}
                fill={phase === 'ready' ? 'var(--status-success)' : 'var(--bg-primary)'}
                stroke={phase === 'ready' ? 'var(--status-success)' : 'var(--text-tertiary)'}
                strokeWidth={1}
              />
            )}
          </g>
        );
      })}
    </svg>
  );
}
