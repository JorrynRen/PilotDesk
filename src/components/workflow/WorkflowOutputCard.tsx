/**
 * WorkflowOutputCard — 工作流「最终产出」卡片。
 *
 * 实例详情（WorkflowMonitor）与定义卡「查看结果」抽屉共用：
 * 后端把最终产出（End → End 上游 → 兜底节点）落在 `instance.output` 上，
 * 这里负责归一化后渲染（解包包装键 / 识别主正文 / 其余字段分块），
 * 并提供「原始数据」开关，避免归一化丢信息时用户无从核对。
 *
 * 还能把这份产出**沉淀成知识**（`SaveToKnowledgeDialog`）：工作流跑出来的报告/结论
 * 常常就是要长期留用的东西，而它没有"原文文件"可导入 —— 与片段 / AI 生成同一类交付物。
 */

import React, { useMemo, useState } from 'react';
import { ScrollText, Copy, Check, ChevronDown, ChevronRight, Info, BookPlus } from 'lucide-react';
import type { WorkflowInstance } from '../../types/workflow';
import { MarkdownRenderer } from '../message/MarkdownRenderer';
import { SaveToKnowledgeDialog } from '../knowledge/SaveToKnowledgeDialog';
import {
  normalizeWorkflowOutput,
  OUTPUT_SOURCE_LABEL,
  explainEmptyOutput,
} from '../../utils/workflowOutput';

interface Props {
  instance: WorkflowInstance;
  /** 紧凑形态（定义卡抽屉里用），默认 false */
  compact?: boolean;
}

export const WorkflowOutputCard: React.FC<Props> = ({ instance, compact = false }) => {
  const [showRaw, setShowRaw] = useState(false);
  const [copied, setCopied] = useState(false);
  /** 「存为知识」弹窗：把这份最终产出显式沉淀进某个知识库（进「待确认」队列） */
  const [saveToKb, setSaveToKb] = useState(false);

  const normalized = useMemo(() => normalizeWorkflowOutput(instance.output), [instance.output]);
  const sourceLabel = OUTPUT_SOURCE_LABEL[instance.outputSource ?? 'none'] ?? instance.outputSource ?? '';
  const from = instance.outputNodeLabel || sourceLabel;

  /**
   * 交给知识库的那份文本：与「复制正文」**完全同一份**（`body || rawJson`）。
   * 刻意不另做一套"更全"的取法 —— 同屏两个动作给出两种文本，用户核对时会怀疑哪个才是产出。
   */
  const kbText = normalized.body || normalized.rawJson;

  /**
   * 沉淀时给模型看的**角色标签**：用工作流名字（产出不是对话，标签说明"这是谁产出的"）。
   * 加「工作流」前缀是为了避开 `user/assistant/system` 这类会被后端映射成「用户/助手」的保留词。
   */
  const kbRole = instance.definitionName
    ? `工作流「${instance.definitionName}」`
    : '工作流产出';

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(normalized.body || normalized.rawJson);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      /* 剪贴板不可用时静默：用户仍可展开原始数据自行复制 */
    }
  };

  return (
    <>
    <div
      className="rounded"
      style={{
        border: '1px solid var(--border)',
        backgroundColor: 'var(--bg-secondary)',
        overflow: 'hidden',
      }}
    >
      {/* 头部：标题 + 来源 + 操作 */}
      <div
        className="flex items-center gap-2 px-2.5 h-7"
        style={{ backgroundColor: 'var(--bg-tertiary)', borderBottom: '1px solid var(--border)' }}
      >
        <ScrollText size={11} style={{ color: 'var(--accent)', flexShrink: 0 }} />
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>最终产出</span>
        {!normalized.empty && from && (
          <span
            className="text-[10px] px-1.5 rounded-full"
            style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-secondary)', whiteSpace: 'nowrap' }}
            title={`产出取值来源：${sourceLabel}`}
          >
            来自「{from}」
          </span>
        )}
        <span className="flex-1" />
        <button
          onClick={() => setShowRaw((v) => !v)}
          className="pd-btn text-[10px] px-1.5 py-0.5 rounded transition-colors"
          style={{ color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
          title="查看未归一化的原始数据"
        >
          {showRaw ? '收起原始数据' : '原始数据'}
        </button>
        {!normalized.empty && (
          <button
            onClick={handleCopy}
            className="pd-btn p-1 rounded transition-colors"
            style={{ color: copied ? 'var(--accent)' : 'var(--text-tertiary)' }}
            title={copied ? '已复制' : '复制正文'}
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
          </button>
        )}
        {!normalized.empty && (
          <button
            onClick={() => setSaveToKb(true)}
            className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
            style={{ color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}
            title="存为知识：选知识库后进入「待确认」，核对后采纳（正文不会被改写）"
          >
            <BookPlus size={11} />
            存为知识
          </button>
        )}
      </div>

      {/* 内容 */}
      <div className={compact ? 'p-2' : 'p-2.5'} style={{ maxHeight: compact ? 320 : 420, overflowY: 'auto' }}>
        {normalized.empty ? (
          <div className="flex items-start gap-1.5 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            <Info size={12} style={{ flexShrink: 0, marginTop: 1 }} />
            <span>{explainEmptyOutput(instance.outputSource, instance.status)}</span>
          </div>
        ) : showRaw ? (
          <pre
            className="m-0 text-[10px] whitespace-pre-wrap break-all leading-relaxed"
            style={{ color: 'var(--text-secondary)', fontFamily: 'var(--font-mono, "Cascadia Code", monospace)' }}
          >
            {normalized.rawJson}
          </pre>
        ) : (
          <div className="space-y-2">
            {normalized.body && (
              <div className="text-[12px] leading-relaxed" style={{ color: 'var(--text-primary)' }}>
                <MarkdownRenderer content={normalized.body} />
              </div>
            )}
            {normalized.fields.length > 0 && (
              <div
                className="space-y-1 pt-1.5"
                style={normalized.body ? { borderTop: '1px dashed var(--border)' } : undefined}
              >
                {normalized.fields.map((f, i) => (
                  <div key={i} className="flex items-start gap-2">
                    <span
                      className="text-[10px] shrink-0 pt-0.5"
                      style={{ color: 'var(--text-tertiary)', minWidth: 64, wordBreak: 'break-all' }}
                    >
                      {f.label}
                    </span>
                    <div className="flex-1 min-w-0">
                      {f.markdown ? (
                        <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
                          <MarkdownRenderer content={f.text} />
                        </div>
                      ) : (
                        <pre
                          className="m-0 text-[11px] whitespace-pre-wrap break-all"
                          style={{ color: 'var(--text-secondary)', fontFamily: 'inherit' }}
                        >
                          {f.text}
                        </pre>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>

    {/* 沉淀弹窗：把这份最终产出显式存进某个知识库（默认「原样存入」，正文不被改写） */}
    {saveToKb && (
      <SaveToKnowledgeDialog
        messages={[{ role: kbRole, content: kbText }]}
        onClose={() => setSaveToKb(false)}
      />
    )}
    </>
  );
};

/**
 * 值预览：与产出卡片同一套归一化，但有文本内容才渲染 Markdown 正文 + 其余字段，
 * 拿不到可读文本时（纯结构化数据）退回格式化 JSON——"预览"在两处含义一致。
 *
 * 不设内部滚动（`maxHeight` 默认不限）：调用方多半已有滚动容器，套两层滚动很难用。
 */
export const ValuePreview: React.FC<{ value: unknown; maxHeight?: number }> = ({ value, maxHeight }) => {
  const normalized = useMemo(() => normalizeWorkflowOutput(value), [value]);
  if (normalized.empty) {
    return <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>（无内容）</div>;
  }
  return (
    <div className="space-y-1.5" style={maxHeight ? { maxHeight, overflowY: 'auto' } : undefined}>
      {normalized.body && (
        <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-primary)' }}>
          <MarkdownRenderer content={normalized.body} />
        </div>
      )}
      {normalized.fields.length > 0 && (
        <div
          className="space-y-1 pt-1"
          style={normalized.body ? { borderTop: '1px dashed var(--border)' } : undefined}
        >
          {normalized.fields.map((f, i) => (
            <div key={i} className="flex items-start gap-2">
              <span
                className="text-[10px] shrink-0 pt-0.5"
                style={{ color: 'var(--text-tertiary)', minWidth: 64, wordBreak: 'break-all' }}
              >
                {f.label}
              </span>
              <pre
                className="m-0 flex-1 text-[10px] whitespace-pre-wrap break-all"
                style={{ color: 'var(--text-secondary)', fontFamily: 'inherit' }}
              >
                {f.text}
              </pre>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

/** 折叠容器：把「运行上下文」这类次要信息收起来，避免挤占产出的注意力 */
export const CollapsibleSection: React.FC<{
  title: string;
  defaultOpen?: boolean;
  /** 内容区最大高度（内滚动），默认 240 */
  maxHeight?: number;
  children: React.ReactNode;
}> = ({ title, defaultOpen = false, maxHeight = 240, children }) => {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="rounded" style={{ border: '1px solid var(--border)' }}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="pd-btn w-full flex items-center gap-1 px-2.5 h-7 text-[11px] text-left transition-colors"
        style={{ color: 'var(--text-tertiary)' }}
      >
        {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        {title}
      </button>
      {open && (
        <div
          className="p-2.5 text-[10px]"
          style={{ borderTop: '1px solid var(--border)', maxHeight, overflowY: 'auto' }}
        >
          {children}
        </div>
      )}
    </div>
  );
};
