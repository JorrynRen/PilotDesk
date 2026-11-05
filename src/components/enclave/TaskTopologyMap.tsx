/**
 * 飞地任务拓扑地图：按前置依赖（blockedBy）从左到右分层，父子汇总任务用分组框圈住。
 *
 * 交互：空白处拖拽平移、滚轮以光标为锚点缩放、右下角「适应窗口」；
 * 点击节点由外部打开任务详情（地图保持可见，便于连续查看）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Lock, Maximize2, Minus, Plus } from 'lucide-react';
import type { TaskView } from '../../types/enclave';
import { STATUS_COLOR, STATUS_LABEL } from './taskStatusMeta';
import { buildTaskTopology } from './taskTopologyLayout';

const MIN_SCALE = 0.3;
const MAX_SCALE = 1.6;
/** 画布四周留白，避免节点贴边 */
const FIT_PAD = 32;

interface View {
  scale: number;
  x: number;
  y: number;
}

interface Props {
  tasks: TaskView[];
  /** 扁平筛选（单状态 / 负责人 / 逾期）时不做父子分组 */
  flat: boolean;
  activeTaskId: string | null;
  memberLabel: (id: string | null) => string;
  onSelect: (id: string) => void;
  emptyHint: string;
  className?: string;
}

function dueLabel(unixSec: number | null): string {
  if (!unixSec) return '';
  const d = new Date(unixSec * 1000);
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function isOverdue(task: TaskView, nowSec: number): boolean {
  if (!task.dueDate || task.dueDate >= nowSec) return false;
  return task.status === 'pending' || task.status === 'in_progress' || task.status === 'submitted';
}

function edgePath(edge: { x1: number; y1: number; x2: number; y2: number; direction: 'h' | 'v' }): string {
  if (edge.direction === 'h') {
    const dx = Math.max(24, (edge.x2 - edge.x1) / 2);
    return `M ${edge.x1} ${edge.y1} C ${edge.x1 + dx} ${edge.y1}, ${edge.x2 - dx} ${edge.y2}, ${edge.x2} ${edge.y2}`;
  }
  const dy = Math.max(24, (edge.y2 - edge.y1) / 2);
  return `M ${edge.x1} ${edge.y1} C ${edge.x1} ${edge.y1 + dy}, ${edge.x2} ${edge.y2 - dy}, ${edge.x2} ${edge.y2}`;
}

export function TaskTopologyMap({
  tasks,
  flat,
  activeTaskId,
  memberLabel,
  onSelect,
  emptyHint,
  className,
}: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ sx: number; sy: number; ox: number; oy: number } | null>(null);
  const fitKeyRef = useRef('');
  const [host, setHost] = useState({ w: 0, h: 0 });
  const [view, setView] = useState<View>({ scale: 1, x: 0, y: 0 });

  const layout = useMemo(() => buildTaskTopology(tasks, flat), [tasks, flat]);
  const nowSec = Math.floor(Date.now() / 1000);
  /** 空态与地图是两个不同的宿主元素，切换时需要重新挂观察器 */
  const hasTasks = tasks.length > 0;

  useEffect(() => {
    const el = hostRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (!rect) return;
      setHost({ w: rect.width, h: rect.height });
    });
    ro.observe(el);
    setHost({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [hasTasks]);

  const fit = useCallback(() => {
    if (!host.w || !host.h || !layout.width || !layout.height) return;
    const scale = Math.min(
      MAX_SCALE,
      Math.max(
        MIN_SCALE,
        Math.min(1, (host.w - FIT_PAD) / layout.width, (host.h - FIT_PAD) / layout.height),
      ),
    );
    setView({
      scale,
      x: (host.w - layout.width * scale) / 2,
      y: (host.h - layout.height * scale) / 2,
    });
  }, [host.w, host.h, layout.width, layout.height]);

  // 只在画布尺寸变化时自适应；任务状态更新（尺寸不变）不打断用户当前的缩放/平移
  useEffect(() => {
    if (!host.w || !host.h) return;
    const key = `${layout.width}x${layout.height}`;
    if (fitKeyRef.current === key) return;
    fitKeyRef.current = key;
    fit();
  }, [host.w, host.h, layout.width, layout.height, fit]);

  // 滚轮缩放：React 的 onWheel 是 passive 的，必须挂原生监听才能 preventDefault
  useEffect(() => {
    const el = hostRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const py = e.clientY - rect.top;
      setView((v) => {
        const next = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * (e.deltaY < 0 ? 1.12 : 1 / 1.12)));
        const k = next / v.scale;
        return { scale: next, x: px - (px - v.x) * k, y: py - (py - v.y) * k };
      });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [hasTasks]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    // 节点与地图控件自己处理点击，不触发画布拖拽
    if ((e.target as HTMLElement).closest('[data-topo-node], button')) return;
    dragRef.current = { sx: e.clientX, sy: e.clientY, ox: view.x, oy: view.y };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    setView((v) => ({
      ...v,
      x: drag.ox + (e.clientX - drag.sx),
      y: drag.oy + (e.clientY - drag.sy),
    }));
  };
  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    dragRef.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 指针已释放时忽略
    }
  };

  const zoomBy = (factor: number) => {
    if (!host.w || !host.h) return;
    setView((v) => {
      const next = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * factor));
      const k = next / v.scale;
      const cx = host.w / 2;
      const cy = host.h / 2;
      return { scale: next, x: cx - (cx - v.x) * k, y: cy - (cy - v.y) * k };
    });
  };

  if (tasks.length === 0) {
    return (
      <div
        className={`flex items-center justify-center text-xs ${className ?? ''}`}
        style={{ color: 'var(--text-tertiary)' }}
      >
        {emptyHint}
      </div>
    );
  }

  return (
    <div
      ref={hostRef}
      className={`relative overflow-hidden select-none ${className ?? ''}`}
      style={{ cursor: 'grab', touchAction: 'none' }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <div
        className="absolute origin-top-left"
        style={{
          width: layout.width,
          height: layout.height,
          transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
        }}
      >
        {/* 分组框（最底层） */}
        {layout.groups
          .filter((g) => g.framed)
          .map((g) => (
            <div
              key={g.key}
              className="absolute rounded-lg"
              style={{
                left: g.x,
                top: g.y,
                width: g.width,
                height: g.height,
                backgroundColor: 'var(--bg-side)',
                border: '1px dashed var(--border)',
              }}
            >
              {g.rail && (
                <div
                  className="absolute"
                  style={{
                    left: g.rail.x - g.x,
                    top: g.rail.y1 - g.y,
                    width: 1,
                    height: Math.max(0, g.rail.y2 - g.rail.y1),
                    backgroundColor: 'var(--border)',
                  }}
                />
              )}
            </div>
          ))}

        {/* 依赖边（节点之下） */}
        <svg
          className="absolute inset-0 pointer-events-none"
          width={layout.width}
          height={layout.height}
        >
          <defs>
            <marker
              id="pd-topo-arrow"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="6"
              markerHeight="6"
              orient="auto"
            >
              <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--text-tertiary)" />
            </marker>
            <marker
              id="pd-topo-arrow-blocked"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="6"
              markerHeight="6"
              orient="auto"
            >
              <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--status-danger)" />
            </marker>
          </defs>
          {layout.edges.map((edge) => (
            <path
              key={edge.key}
              d={edgePath(edge)}
              fill="none"
              stroke={edge.blocked ? 'var(--status-danger)' : 'var(--text-tertiary)'}
              strokeWidth={1}
              strokeDasharray={edge.blocked ? '4 3' : undefined}
              opacity={edge.blocked ? 0.8 : 0.5}
              markerEnd={`url(#${edge.blocked ? 'pd-topo-arrow-blocked' : 'pd-topo-arrow'})`}
            />
          ))}
        </svg>

        {/* 节点 */}
        {layout.nodes.map((node) => {
          const task = node.task;
          const active = node.id === activeTaskId;
          const overdue = isOverdue(task, nowSec);
          // 只改四边颜色，不使用 border 简写：简写会覆盖 borderLeft 的状态色条
          const lineColor = active ? 'var(--accent)' : 'var(--border)';
          return (
            <button
              key={node.id}
              data-topo-node
              type="button"
              onClick={() => onSelect(node.id)}
              title={`${task.title}\n${STATUS_LABEL[task.status]} · ${memberLabel(task.assigneeId)}`}
              className="absolute text-left rounded-lg px-2 py-1.5 flex flex-col gap-0.5 transition-colors"
              style={{
                left: node.x,
                top: node.y,
                width: node.width,
                height: node.height,
                backgroundColor: active ? 'var(--accent-light)' : 'var(--bg-raised)',
                borderTop: `1px solid ${lineColor}`,
                borderRight: `1px solid ${lineColor}`,
                borderBottom: `1px solid ${lineColor}`,
                borderLeft: `3px solid ${STATUS_COLOR[task.status]}`,
                boxShadow: '0 1px 2px rgba(0,0,0,0.12)',
                cursor: 'pointer',
              }}
            >
              <div className="flex items-center gap-1 min-w-0">
                <span className="shrink-0 w-1.5 h-1.5 rounded-full" style={{ backgroundColor: STATUS_COLOR[task.status] }} />
                <span
                  className="text-[11px] truncate min-w-0"
                  style={{ color: 'var(--text-primary)', fontWeight: node.role === 'parent' ? 600 : 500 }}
                >
                  {task.title}
                </span>
                {task.blocked && (
                  <Lock size={9} className="shrink-0" style={{ color: 'var(--status-danger)' }} />
                )}
              </div>
              <div className="flex items-center gap-1.5 text-[9px] min-w-0" style={{ color: 'var(--text-tertiary)' }}>
                <span className="shrink-0">{STATUS_LABEL[task.status]}</span>
                <span className="truncate min-w-0">{memberLabel(task.assigneeId)}</span>
                {task.dueDate && (
                  <span className="shrink-0" style={{ color: overdue ? 'var(--status-danger)' : 'var(--text-tertiary)' }}>
                    {dueLabel(task.dueDate)}
                  </span>
                )}
                {node.childTotal > 0 && (
                  <span
                    className="shrink-0 ml-auto"
                    title={`汇总任务：${node.childDone}/${node.childTotal} 个子任务已完成`}
                  >
                    {node.childDone}/{node.childTotal}
                  </span>
                )}
              </div>
            </button>
          );
        })}
      </div>

      {/* 缩放控制 */}
      <div className="absolute bottom-2 right-2 flex items-center gap-1 rounded-lg px-1 py-0.5" style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}>
        <button
          type="button"
          onClick={() => zoomBy(1 / 1.2)}
          className="pd-btn w-6 h-6 flex items-center justify-center rounded"
          style={{ color: 'var(--text-secondary)' }}
          title="缩小"
        >
          <Minus size={11} />
        </button>
        <span className="text-[10px] w-9 text-center" style={{ color: 'var(--text-tertiary)' }}>
          {Math.round(view.scale * 100)}%
        </span>
        <button
          type="button"
          onClick={() => zoomBy(1.2)}
          className="pd-btn w-6 h-6 flex items-center justify-center rounded"
          style={{ color: 'var(--text-secondary)' }}
          title="放大"
        >
          <Plus size={11} />
        </button>
        <button
          type="button"
          onClick={fit}
          className="pd-btn w-6 h-6 flex items-center justify-center rounded"
          style={{ color: 'var(--text-secondary)' }}
          title="适应窗口"
        >
          <Maximize2 size={11} />
        </button>
      </div>
    </div>
  );
}
