/**
 * WorkflowNodeTimeline — 一次执行的**逐节点**执行记录。
 *
 * 与「最终产出」卡片互补：产出卡片回答"结果是什么"，这里回答"结果是怎么一步步出来的"，
 * 并且标出哪一条就是最终产出的来源（用户不必再猜哪条才是真正的结果）。
 *
 * 数据来自 `get_node_executions`（事件派生：node/start + node/status + node/result），
 * 节点名与类型来自当前定义（`meta`），定义被改过时退回节点 id。
 */

import React, { useState } from 'react';
import { ChevronDown, ChevronRight, CheckCircle, XCircle, Ban, Clock, AlertTriangle, ScrollText } from 'lucide-react';
import { MarkdownRenderer } from '../message/MarkdownRenderer';
import { normalizeWorkflowOutput } from '../../utils/workflowOutput';

/** 后端返回的节点执行记录：input/output 形态随节点类型而变，按未知值处理后再窄化 */
export interface NodeExecRow {
  nodeId: string;
  status: string;
  input?: unknown;
  output?: unknown;
  error?: string | null;
  startedAt?: number | null;
  finishedAt?: number | null;
}

export interface NodeMeta {
  label: string;
  type: string;
}

interface Props {
  rows: NodeExecRow[];
  /** nodeId → 展示信息（来自当前定义） */
  meta?: Record<string, NodeMeta>;
  loading?: boolean;
  /** 取数失败（如脱离 Tauri 运行） */
  error?: string | null;
  /** 该执行的最终产出（用于标出"最终产出"来自哪个节点） */
  finalOutput?: unknown;
}

const NODE_STATUS: Record<string, { label: string; color: string; bg: string; icon: React.ReactNode }> = {
  pending: { label: '待执行', color: '#6B7280', bg: 'rgba(107,114,128,0.12)', icon: <Clock size={10} /> },
  running: { label: '运行中', color: '#3B82F6', bg: 'rgba(59,130,246,0.12)', icon: <Clock size={10} /> },
  completed: { label: '完成', color: '#10B981', bg: 'rgba(16,185,129,0.12)', icon: <CheckCircle size={10} /> },
  failed: { label: '失败', color: '#EF4444', bg: 'rgba(239,68,68,0.12)', icon: <XCircle size={10} /> },
  skipped: { label: '跳过', color: '#6B7280', bg: 'rgba(107,114,128,0.12)', icon: <Ban size={10} /> },
  cancelled: { label: '已取消', color: '#6B7280', bg: 'rgba(107,114,128,0.12)', icon: <Ban size={10} /> },
  timeout: { label: '超时', color: '#EF4444', bg: 'rgba(239,68,68,0.12)', icon: <AlertTriangle size={10} /> },
};

const NODE_TYPE_LABEL: Record<string, string> = {
  start: '开始', end: '结束', agent: '智能体', api: 'API', transform: '转换',
  interact: '人工交互', plugin: '插件', subflow: '子工作流',
};

/** 节点类型标记色（节点类型签标与文字色） */
const NODE_TYPE_COLOR: Record<string, string> = {
  start: '#6B7280', end: '#6B7280', agent: '#3B82F6', api: '#8B5CF6',
  transform: '#F59E0B', interact: '#EC4899', plugin: '#10B981', subflow: '#0EA5E9',
};

/** 取不到类型时用中性灰（CSS 变量） */
const typeColorOf = (type: string): string => NODE_TYPE_COLOR[type] || 'var(--text-tertiary)';

/** CSS 变量拼不出带透明度的色值，只有 hex 才给签标上底色 */
const isHexColor = (color: string): boolean => /^#[0-9a-fA-F]{6}$/.test(color);

function formatDuration(start?: number | null, end?: number | null): string {
  if (!start) return '';
  const diff = Math.max(0, (end || Math.floor(Date.now() / 1000)) - start);
  if (diff < 60) return `${diff}秒`;
  if (diff < 3600) return `${Math.floor(diff / 60)}分${diff % 60}秒`;
  return `${Math.floor(diff / 3600)}时${Math.floor((diff % 3600) / 60)}分`;
}

/** 键序无关的规范化序列化（判定"这条就是最终产出"用） */
function stableKey(v: unknown): string {
  if (v === null || typeof v !== 'object') {
    return v === undefined ? 'undefined' : JSON.stringify(v);
  }
  if (Array.isArray(v)) return `[${v.map(stableKey).join(',')}]`;
  const obj = v as Record<string, unknown>;
  return `{${Object.keys(obj).sort().map((k) => `${JSON.stringify(k)}:${stableKey(obj[k])}`).join(',')}}`;
}

/** 产出是否与最终产出等价（比较两者的规范化序列化，不受键顺序影响） */
function sameJson(a: unknown, b: unknown): boolean {
  try {
    return stableKey(a) === stableKey(b);
  } catch {
    return false;
  }
}

/** 单行预览：跳过 `---` / `***` / 空行这类 Markdown 噪音行，取第一行有实际文字的内容 */
function previewLine(body: string): string {
  for (const line of body.split('\n')) {
    const text = line.trim();
    if (!text) continue;
    // 有字母 / 数字 / 中日韩文字才算"有内容"
    if (/[A-Za-z0-9\u4e00-\u9fff]/.test(text)) return text;
  }
  return body.trim();
}

const NodeRow: React.FC<{ row: NodeExecRow; meta?: NodeMeta; isFinalSource: boolean; index: number }> = ({
  row, meta, isFinalSource, index,
}) => {
  // 一律从折叠态开始：逐条展开由用户决定，避免出现"有的展开有的没展开"的困惑；
  // 最终产出的那一条靠签标标明来源，不靠默认展开。
  const [open, setOpen] = useState(false);
  const [showRaw, setShowRaw] = useState(false);

  const status = NODE_STATUS[row.status] ?? {
    label: row.status || '未知', color: 'var(--text-tertiary)', bg: 'var(--bg-tertiary)', icon: null,
  };
  const normalized = normalizeWorkflowOutput(row.output);
  const label = meta?.label || row.nodeId;
  const type = meta?.type || '';
  const typeColor = typeColorOf(type);
  const preview = normalized.empty
    ? row.status === 'skipped' ? '条件分支未命中，未执行' : '（无产出）'
    : previewLine(normalized.body);

  return (
    <div style={{ borderBottom: '1px solid var(--border)' }}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="pd-btn w-full flex items-center gap-2 px-2.5 py-2 text-left transition-colors"
      >
        {open ? <ChevronDown size={12} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
              : <ChevronRight size={12} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />}
        <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)', minWidth: 16 }}>
          {index + 1}
        </span>
        <span className="text-[11px] truncate" style={{ color: 'var(--text-primary)', maxWidth: 200 }} title={label}>
          {label}
        </span>
        {type && (
          // 节点类型用带色签标出：裸文字与产出预览同为弱色文本，扫一眼分不清哪个是类型
          <span
            className="inline-flex items-center text-[9px] px-1.5 py-0.5 rounded shrink-0 leading-none"
            style={{
              color: typeColor,
              backgroundColor: isHexColor(typeColor) ? `${typeColor}1A` : 'var(--bg-tertiary)',
              border: `1px solid ${isHexColor(typeColor) ? `${typeColor}40` : 'var(--border)'}`,
            }}
          >
            {NODE_TYPE_LABEL[type] || type}
          </span>
        )}
        {isFinalSource && (
          <span
            className="inline-flex items-center gap-1 text-[9px] px-1.5 rounded-full shrink-0"
            style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
            title="最终产出取自该节点"
          >
            <ScrollText size={9} />最终产出
          </span>
        )}
        <span className="flex-1 text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }}>
          {preview}
        </span>
        {formatDuration(row.startedAt, row.finishedAt) && (
          <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            {formatDuration(row.startedAt, row.finishedAt)}
          </span>
        )}
        <span
          className="inline-flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded-full shrink-0"
          style={{ backgroundColor: status.bg, color: status.color }}
        >
          {status.icon}{status.label}
        </span>
      </button>

      {open && (
        <div className="px-2.5 pb-2.5 pl-10 space-y-2">
          {row.error && (
            <div
              className="text-[10px] p-1.5 rounded"
              style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444', whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}
            >
              {row.error}
            </div>
          )}

          {/* 输入 */}
          {row.input !== null && row.input !== undefined && (
            <div>
              <div className="text-[9px] mb-0.5" style={{ color: 'var(--text-tertiary)' }}>输入</div>
              <pre
                className="m-0 p-1.5 rounded text-[10px] whitespace-pre-wrap break-all"
                style={{
                  backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)',
                  fontFamily: 'var(--font-mono, "Cascadia Code", monospace)', maxHeight: 120, overflowY: 'auto',
                }}
              >
                {typeof row.input === 'string' ? row.input : JSON.stringify(row.input, null, 2)}
              </pre>
            </div>
          )}

          {/* 产出 */}
          <div>
            <div className="flex items-center gap-2 mb-0.5">
              <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>产出</span>
              {!normalized.empty && (
                <button
                  onClick={() => setShowRaw((v) => !v)}
                  className="pd-btn text-[9px] px-1 rounded transition-colors"
                  style={{ color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
                >
                  {showRaw ? '收起原始数据' : '原始数据'}
                </button>
              )}
            </div>
            {normalized.empty ? (
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>该节点没有产出内容</div>
            ) : showRaw ? (
              <pre
                className="m-0 p-1.5 rounded text-[10px] whitespace-pre-wrap break-all"
                style={{
                  backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)',
                  fontFamily: 'var(--font-mono, "Cascadia Code", monospace)', maxHeight: 200, overflowY: 'auto',
                }}
              >
                {normalized.rawJson}
              </pre>
            ) : (
              <div className="space-y-1.5">
                <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-primary)' }}>
                  <MarkdownRenderer content={normalized.body} />
                </div>
                {normalized.fields.map((f, i) => (
                  <div key={i} className="flex items-start gap-2">
                    <span className="text-[9px] shrink-0 pt-0.5" style={{ color: 'var(--text-tertiary)', minWidth: 56 }}>{f.label}</span>
                    <pre className="m-0 flex-1 text-[10px] whitespace-pre-wrap break-all" style={{ color: 'var(--text-secondary)', fontFamily: 'inherit' }}>
                      {f.text}
                    </pre>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

export const WorkflowNodeTimeline: React.FC<Props> = ({ rows, meta, loading, error, finalOutput }) => {
  if (loading) {
    return <div className="text-[11px] py-3 text-center" style={{ color: 'var(--text-tertiary)' }}>加载执行记录…</div>;
  }
  if (error) {
    return <div className="text-[11px] py-3 text-center" style={{ color: 'var(--text-tertiary)' }}>执行记录读取失败：{error}</div>;
  }
  if (rows.length === 0) {
    return <div className="text-[11px] py-3 text-center" style={{ color: 'var(--text-tertiary)' }}>没有节点执行记录</div>;
  }

  const hasFinal = finalOutput !== undefined && finalOutput !== null;

  return (
    <div className="rounded" style={{ border: '1px solid var(--border)', overflow: 'hidden' }}>
      {rows.map((row, i) => (
        <NodeRow
          key={row.nodeId}
          row={row}
          meta={meta?.[row.nodeId]}
          index={i}
          isFinalSource={hasFinal && sameJson(row.output, finalOutput)}
        />
      ))}
    </div>
  );
};
