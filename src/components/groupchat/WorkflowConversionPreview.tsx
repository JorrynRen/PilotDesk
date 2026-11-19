import { useId, useMemo, useState } from 'react';
import {
  Bot,
  CheckCircle2,
  Circle,
  Code2,
  GitBranch,
  Image as ImageIcon,
  Layers3,
  Play,
  Puzzle,
  RotateCcw,
  UserRound,
  Workflow,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import type { Stage, WorkflowDefinition, WorkflowNode } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';

const GRAPH_PADDING = 20;
const GRAPH_NODE_WIDTH = 176;
const GRAPH_NODE_HEIGHT = 58;
const GRAPH_COLUMN_GAP = 48;
const GRAPH_ROW_GAP = 16;

const TRIGGER_LABEL: Record<string, string> = {
  manual: '手动触发',
  cron: '定时触发',
  event: '事件触发',
};

function NodeTypeIcon({ type, size = 12 }: { type: string; size?: number }) {
  switch (type) {
    case 'start':
      return <Play size={size} />;
    case 'end':
      return <CheckCircle2 size={size} />;
    case 'agent':
      return <Bot size={size} />;
    case 'interact':
      return <UserRound size={size} />;
    case 'media':
      return <ImageIcon size={size} />;
    case 'api':
      return <Code2 size={size} />;
    case 'transform':
      return <GitBranch size={size} />;
    case 'plugin':
      return <Puzzle size={size} />;
    case 'subflow':
      return <Layers3 size={size} />;
    default:
      return <Circle size={size} />;
  }
}

function truncateText(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max)}…` : value;
}

function nodeSort(a: WorkflowNode, b: WorkflowNode): number {
  const ax = a.position?.x ?? 0;
  const bx = b.position?.x ?? 0;
  if (ax !== bx) return ax - bx;
  return (a.position?.y ?? 0) - (b.position?.y ?? 0);
}

interface GraphLayout {
  width: number;
  height: number;
  positions: Map<string, { x: number; y: number }>;
  layers: WorkflowNode[][];
}

function buildGraphLayout(stage: Stage): GraphLayout {
  const nodes = [...stage.nodes].sort(nodeSort);
  const nodeIds = new Set(nodes.map((node) => node.id));
  const depths = new Map(nodes.map((node) => [node.id, 0]));

  // Longest-path layering keeps every edge pointing left-to-right. The backend rejects
  // cyclic task graphs, so the pass limit only protects this read-only preview.
  for (let pass = 0; pass < nodes.length; pass += 1) {
    let changed = false;
    for (const edge of stage.edges) {
      if (!nodeIds.has(edge.source) || !nodeIds.has(edge.target)) continue;
      const next = (depths.get(edge.source) ?? 0) + 1;
      if (next <= nodes.length - 1 && next > (depths.get(edge.target) ?? 0)) {
        depths.set(edge.target, next);
        changed = true;
      }
    }
    if (!changed) break;
  }

  const layerCount = Math.max(1, ...depths.values()) + 1;
  const layers: WorkflowNode[][] = Array.from({ length: layerCount }, () => []);
  for (const node of nodes) layers[depths.get(node.id) ?? 0].push(node);

  const maxRows = Math.max(1, ...layers.map((layer) => layer.length));
  const height = GRAPH_PADDING * 2
    + maxRows * GRAPH_NODE_HEIGHT
    + Math.max(0, maxRows - 1) * GRAPH_ROW_GAP;
  const contentHeight = maxRows * GRAPH_NODE_HEIGHT
    + Math.max(0, maxRows - 1) * GRAPH_ROW_GAP;
  const width = GRAPH_PADDING * 2
    + layerCount * GRAPH_NODE_WIDTH
    + Math.max(0, layerCount - 1) * GRAPH_COLUMN_GAP;
  const positions = new Map<string, { x: number; y: number }>();

  layers.forEach((layer, layerIndex) => {
    const groupHeight = layer.length * GRAPH_NODE_HEIGHT
      + Math.max(0, layer.length - 1) * GRAPH_ROW_GAP;
    const startY = GRAPH_PADDING + (contentHeight - groupHeight) / 2;
    layer.forEach((node, rowIndex) => {
      positions.set(node.id, {
        x: GRAPH_PADDING + layerIndex * (GRAPH_NODE_WIDTH + GRAPH_COLUMN_GAP),
        y: startY + rowIndex * (GRAPH_NODE_HEIGHT + GRAPH_ROW_GAP),
      });
    });
  });

  return { width, height, positions, layers };
}

function StageGraph({ stage }: { stage: Stage }) {
  const rawId = useId();
  const markerId = `pd-workflow-preview-arrow-${rawId.replace(/[^a-zA-Z0-9_-]/g, '')}`;
  const [scale, setScale] = useState(1);
  const layout = useMemo(() => buildGraphLayout(stage), [stage]);
  const nodeMap = useMemo(
    () => new Map(stage.nodes.map((node) => [node.id, node])),
    [stage.nodes],
  );
  const nodeTypes = useMemo(
    () => [...new Set(stage.nodes.map((node) => node.type))],
    [stage.nodes],
  );

  return (
    <section
      className="rounded-lg overflow-hidden"
      style={{ backgroundColor: 'var(--bg-raised)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-center gap-2 px-3 py-2" style={{ borderBottom: '1px solid var(--border-light)' }}>
        <Layers3 size={13} style={{ color: 'var(--accent)' }} />
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>{stage.name}</span>
        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          {stage.nodes.length} 个节点 · {stage.edges.length} 条依赖
        </span>
        <div className="flex-1" />
        <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
          {stage.gate.strategy === 'all' ? '全部完成后汇合' : `门控：${stage.gate.strategy}`}
        </span>
        <div
          className="inline-flex items-center gap-0.5 p-0.5 rounded-md shrink-0"
          style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
        >
          <button
            className="pd-btn inline-flex items-center justify-center rounded"
            style={{ width: 20, height: 20, color: 'var(--text-secondary)' }}
            onClick={() => setScale((value) => Math.max(0.5, Number((value - 0.1).toFixed(1))))}
            disabled={scale <= 0.5}
            title="缩小图谱"
            aria-label="缩小图谱"
          >
            <ZoomOut size={11} />
          </button>
          <button
            className="pd-btn text-[9px] tabular-nums rounded"
            style={{ minWidth: 35, height: 20, color: 'var(--text-secondary)' }}
            onClick={() => setScale(1)}
            title="恢复到 100%"
            aria-label={`当前缩放 ${Math.round(scale * 100)}%，点击恢复 100%`}
          >
            {Math.round(scale * 100)}%
          </button>
          <button
            className="pd-btn inline-flex items-center justify-center rounded"
            style={{ width: 20, height: 20, color: 'var(--text-secondary)' }}
            onClick={() => setScale((value) => Math.min(2, Number((value + 0.1).toFixed(1))))}
            disabled={scale >= 2}
            title="放大图谱"
            aria-label="放大图谱"
          >
            <ZoomIn size={11} />
          </button>
          <button
            className="pd-btn inline-flex items-center justify-center rounded"
            style={{ width: 20, height: 20, color: 'var(--text-tertiary)' }}
            onClick={() => setScale(1)}
            disabled={scale === 1}
            title="重置缩放"
            aria-label="重置缩放"
          >
            <RotateCcw size={10} />
          </button>
        </div>
      </div>

      <div
        className="px-2 py-2"
        style={{
          backgroundColor: 'var(--bg-field)',
          overflowX: 'auto',
          overflowY: 'hidden',
        }}
      >
        <svg
          role="img"
          aria-label={`${stage.name}工作流依赖图，共 ${stage.nodes.length} 个节点`}
          viewBox={`0 0 ${layout.width} ${layout.height}`}
          width={layout.width * scale}
          height={layout.height * scale}
          preserveAspectRatio="xMidYMid meet"
          style={{ display: 'block', maxWidth: 'none' }}
        >
          <defs>
            <marker
              id={markerId}
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="7"
              markerHeight="7"
              markerUnits="userSpaceOnUse"
              orient="auto"
            >
              <path d="M1 1 L7 4 L1 7 Z" fill="var(--text-tertiary)" />
            </marker>
          </defs>

          {stage.edges.map((edge) => {
            const source = layout.positions.get(edge.source);
            const target = layout.positions.get(edge.target);
            if (!source || !target) return null;
            const startX = source.x + GRAPH_NODE_WIDTH;
            const startY = source.y + GRAPH_NODE_HEIGHT / 2;
            const endX = target.x;
            const endY = target.y + GRAPH_NODE_HEIGHT / 2;
            const middleX = (startX + endX) / 2;
            return (
              <path
                key={edge.id}
                d={`M ${startX} ${startY} C ${middleX} ${startY}, ${middleX} ${endY}, ${endX} ${endY}`}
                fill="none"
                stroke="var(--text-tertiary)"
                strokeWidth={1.2}
                strokeLinecap="round"
                markerEnd={`url(#${markerId})`}
                opacity={0.72}
              />
            );
          })}

          {[...layout.positions.entries()].map(([nodeId, position]) => {
            const node = nodeMap.get(nodeId);
            if (!node) return null;
            const meta = getNodeTypeMeta(node.type);
            const inputCount = Object.keys(node.inputMapping ?? {}).length;
            const outputCount = Object.keys(node.outputMapping ?? {}).length;
            const mappingText = [
              inputCount > 0 ? `入 ${inputCount}` : '',
              outputCount > 0 ? `出 ${outputCount}` : '',
            ].filter(Boolean).join(' · ');
            const title = node.label || meta.label;

            return (
              <foreignObject
                key={node.id}
                x={position.x}
                y={position.y}
                width={GRAPH_NODE_WIDTH}
                height={GRAPH_NODE_HEIGHT}
                aria-label={`${meta.label}：${title}${mappingText ? `（${mappingText}）` : ''}`}
              >
                <div
                  title={`${meta.label}：${title}${mappingText ? `（${mappingText}）` : ''}`}
                  style={{
                    width: '100%',
                    height: '100%',
                    boxSizing: 'border-box',
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    padding: '7px 9px',
                    overflow: 'hidden',
                    borderRadius: 7,
                    backgroundColor: 'var(--bg-primary)',
                    border: '1px solid var(--border)',
                  }}
                >
                  <span
                    style={{
                      width: 22,
                      height: 22,
                      flex: '0 0 22px',
                      display: 'inline-flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                      color: meta.color,
                      backgroundColor: `${meta.color}1f`,
                      borderRadius: 5,
                    }}
                  >
                    <NodeTypeIcon type={node.type} size={11} />
                  </span>
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 5, minWidth: 0 }}>
                      <span style={{ color: meta.color, fontSize: 9.5, fontWeight: 600, whiteSpace: 'nowrap' }}>
                        {meta.label}
                      </span>
                      {mappingText && (
                        <span
                          style={{
                            marginLeft: 'auto',
                            color: 'var(--text-tertiary)',
                            fontSize: 8.5,
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {mappingText}
                        </span>
                      )}
                    </div>
                    <div
                      style={{
                        minWidth: 0,
                        marginTop: 2,
                        overflow: 'hidden',
                        color: 'var(--text-primary)',
                        fontSize: 10.5,
                        fontWeight: 500,
                        lineHeight: '14px',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {title}
                    </div>
                  </div>
                </div>
              </foreignObject>
            );
          })}
        </svg>
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
        {nodeTypes.map((type) => {
          const meta = getNodeTypeMeta(type);
          return (
            <span key={type} className="inline-flex items-center gap-1 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
              <NodeTypeIcon type={type} size={10} />
              {meta.label}
            </span>
          );
        })}
      </div>
    </section>
  );
}

export function WorkflowConversionPreview({ definition }: { definition: WorkflowDefinition }) {
  const stageCount = definition.stages.length;
  const nodeCount = definition.stages.reduce((sum, stage) => sum + stage.nodes.length, 0);
  const edgeCount = definition.stages.reduce((sum, stage) => sum + stage.edges.length, 0);
  const description = definition.description.trim();

  return (
    <div className="flex flex-col gap-3">
      <section
        className="rounded-lg p-3"
        style={{ backgroundColor: 'var(--bg-raised)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-start gap-3">
          <div
            className="shrink-0 flex items-center justify-center rounded-lg"
            style={{
              width: 38,
              height: 38,
              color: 'var(--accent)',
              backgroundColor: 'var(--accent-light)',
              border: '1px solid color-mix(in srgb, var(--accent) 26%, var(--border))',
            }}
          >
            <Workflow size={18} />
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="text-[13px] font-semibold" style={{ color: 'var(--text-primary)' }}>
                {definition.name || '未命名工作流'}
              </span>
              <span
                className="text-[9px] px-1.5 py-0.5 rounded-full"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
              >
                v{definition.version}
              </span>
              <span
                className="text-[9px] px-1.5 py-0.5 rounded-full"
                style={{
                  color: definition.enabled ? 'var(--status-success)' : 'var(--text-tertiary)',
                  backgroundColor: definition.enabled ? 'var(--status-success-bg)' : 'var(--bg-field)',
                }}
              >
                {definition.enabled ? '已启用' : '未启用'}
              </span>
              <span
                className="text-[9px] px-1.5 py-0.5 rounded-full"
                style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
              >
                {TRIGGER_LABEL[definition.trigger.triggerType] ?? definition.trigger.triggerType}
              </span>
            </div>
            {description && (
              <div
                className="mt-1 text-[10px] leading-relaxed whitespace-pre-wrap break-words"
                style={{ color: 'var(--text-secondary)' }}
              >
                {truncateText(description, 240)}
              </div>
            )}
          </div>
        </div>

        <div
          className="grid grid-cols-3 gap-px mt-3 rounded-md overflow-hidden"
          style={{ backgroundColor: 'var(--border-light)', border: '1px solid var(--border-light)' }}
        >
          {[
            { label: '阶段', value: stageCount },
            { label: '节点', value: nodeCount },
            { label: '依赖关系', value: edgeCount },
          ].map((item) => (
            <div key={item.label} className="py-2 text-center" style={{ backgroundColor: 'var(--bg-field)' }}>
              <div className="text-[13px] font-semibold tabular-nums" style={{ color: 'var(--text-primary)' }}>{item.value}</div>
              <div className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>{item.label}</div>
            </div>
          ))}
        </div>
      </section>

      <div className="flex items-center gap-1.5 px-0.5">
        <GitBranch size={12} style={{ color: 'var(--accent)' }} />
        <span className="text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务依赖关系</span>
      </div>

      {definition.stages
        .slice()
        .sort((a, b) => a.order - b.order)
        .map((stage) => <StageGraph key={stage.id} stage={stage} />)}
    </div>
  );
}
