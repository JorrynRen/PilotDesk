import { useEffect, useState } from 'react';
import type { ThinkingChainStep } from '../layout/MainPanel';
import { collapseBlankLines } from './MarkdownRenderer';

interface ThinkingChainProps {
  steps: ThinkingChainStep[];
  defaultCollapsed?: boolean;
  /** 当前正在流式输出的条目原始 id（如 reasoning 步骤），流式期间自动展开、结束后收起。 */
  liveStepKeys?: ReadonlySet<string>;
}

/**
 * 把工具步骤 id 归一化为"工具调用键"用于配对。
 * 各数据源给 tool_start / tool_result 的 id 前缀不同（会话持久化 persisted-tool-/persisted-result-、
 * 会话实时 result-{toolId}、群聊持久化 t-/tr-、群聊实时 {toolId}-start/{toolId}-result），
 * 剥离前缀后归一到同一调用键，才能把同一工具调用的 start 与 result 合并为一条条目
 * （否则各自独立显示、重复且缺参数）。
 */
function toolKey(id: string): string {
  return id
    .replace(/^persisted-(tool|result)-/, '')
    .replace(/^result-/, '')
    .replace(/^tr-/, '')
    .replace(/^t-/, '')
    .replace(/-result$/, '')
    .replace(/-start$/, '');
}

/**
 * 思维链/工具调用聚合展示，会话模式与群聊模式共用。
 *
 * 折叠分两档：
 * - 面板档：整个思维链可折叠（点击标题栏），defaultCollapsed 控制；会话模式流式结束
 *   （isStreaming=false）、群聊任务完成后均自动折叠（defaultCollapsed 变化同步）。
 * - 条目档：面板内每一条消息（推理步骤 / 工具调用 / 文件变更）默认折叠为一行，
 *   点击展开该条的完整明细。工具调用行直接内嵌完成状态：
 *   `🔧 调用 edit_file ✅ 完成`（调用中无徽标，完成后 ✅/❌）——工具结果不再单独成行。
 *
 * 展开规则：所有条目（推理步骤 / 工具调用 / 文件变更）默认折叠为一行，点击展开/收起明细；
 * 用户点击后以用户状态为准，无任何自动展开干扰。工具调用行内嵌完成状态：
 * `🔧 调用 edit_file ✅ 完成`（调用中无徽标，完成后 ✅/❌）。
 */
export function ThinkingChain({ steps, defaultCollapsed = true, liveStepKeys }: ThinkingChainProps) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed);
  // 用户手动展开/折叠状态（条目唯一 key → boolean），点击后覆盖自动规则。
  const [manualState, setManualState] = useState<Map<string, boolean>>(() => new Map());

  // 外部折叠意图（defaultCollapsed）变化时同步面板状态：组件实例被列表（Virtuoso 等）
  // 复用时 useState 不响应 prop 变化，由该 effect 兜底，保证"过程中展开、完成后自动折叠"可靠生效。
  useEffect(() => {
    setCollapsed(defaultCollapsed);
  }, [defaultCollapsed]);

  if (steps.length === 0) return null;

  const reasoningCount = steps.filter((s) => s.type === 'reasoning').length;
  const toolCount = steps.filter((s) => s.type === 'tool_start').length;

  // ── 把原始步骤合并为条目列表（保持出现顺序）──
  // tool_start 与同 id 的 tool_result 合并为一条工具条目（参数 + 完成状态 + 结果）；
  // 孤立的 tool_result（无配对 start，如流式事件只到结果）同样合成工具条目，不再单独成行。
  interface ToolItem {
    id: string;
    name: string;
    args?: string;
    result?: string;
    success?: boolean;
    done: boolean;
  }
  type ChainItem =
    | { kind: 'reasoning'; id: string; content?: string }
    | { kind: 'tool'; tool: ToolItem }
    | { kind: 'diff'; id: string; filePath?: string; diff?: string };

  const items: ChainItem[] = [];
  const toolIndex = new Map<string, number>(); // 归一化工具键 -> items 中 tool 条目下标
  const diffSeen = new Map<string, number>(); // diff 原始 id -> 出现次数（兜底去重）
  for (const s of steps) {
    if (s.type === 'reasoning') {
      items.push({ kind: 'reasoning', id: s.id, content: s.content });
    } else if (s.type === 'tool_start') {
      const tool: ToolItem = { id: s.id, name: s.toolName ?? 'tool', args: s.toolArgs, done: false };
      toolIndex.set(toolKey(s.id), items.length);
      items.push({ kind: 'tool', tool });
    } else if (s.type === 'tool_result') {
      const idx = toolIndex.get(toolKey(s.id));
      if (idx !== undefined && items[idx].kind === 'tool') {
        const t = (items[idx] as { kind: 'tool'; tool: ToolItem }).tool;
        t.result = s.toolResult;
        t.success = s.toolSuccess;
        t.done = true;
      } else {
        // 孤立结果（无配对 start）：合成工具条目（无参数），点击展开结果内容。
        items.push({
          kind: 'tool',
          tool: { id: s.id, name: s.toolName ?? 'tool', result: s.toolResult, success: s.toolSuccess, done: true },
        });
      }
    } else if (s.type === 'file_diff') {
      // 上游 id 可能重复（如历史数据同毫秒多个 diff），追加序号保证条目 key 唯一。
      const raw = s.id || 'diff';
      const n = (diffSeen.get(raw) ?? 0) + 1;
      diffSeen.set(raw, n);
      items.push({ kind: 'diff', id: n > 1 ? `${raw}#${n}` : raw, filePath: s.filePath, diff: s.fileDiff });
    }
  }

  // 条目唯一 key（类型前缀，杜绝跨类型 id 相同导致 React key 冲突 / 展开状态串扰）。
  const reasoningKeyOf = (id: string) => `reasoning:${id}`;
  const toolKeyOf = (id: string) => `tool:${id}`;
  const diffKeyOf = (id: string) => `diff:${id}`;

  // 展开规则：流式输出中的条目（liveStepKeys 命中的推理步骤 / 未完成的工具调用）自动展开，
  // 完成后自动折叠；用户手动点击后以用户状态为准（manualState 优先），无自动干扰。
  const resolveExpanded = (key: string, rawId: string, toolLive = false): boolean =>
    manualState.get(key) ?? (toolLive || (liveStepKeys?.has(rawId) ?? false));

  const toggleItem = (key: string) => {
    setManualState((prev) => {
      const cur = prev.get(key) ?? false;
      const next = new Map(prev);
      next.set(key, !cur);
      return next;
    });
  };

  // 推理步骤序号（供折叠行标题区分多条推理）。
  let reasoningSeq = 0;

  return (
    <div
      className="w-full mb-2.5 rounded-lg overflow-hidden"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '0.5px solid var(--border)',
        // 面板折叠：30px=标题栏；展开不设实际高度上限（100000px 仅为保留 max-height 折叠动画，
        // 实际内容远小于该值，高度随内容自然变化、不截断），配合条目级默认折叠。
        maxHeight: collapsed ? '30px' : '100000px',
        transition: 'max-height 0.2s ease',
        // 统一行高：避免继承外部气泡容器（群聊 text-xs leading-relaxed / 会话默认）导致
        // 工具调用行等无显式行高元素（如 code）在两种模式下间距不一致；以组件自身为准。
        lineHeight: '1.5',
      }}
    >
      <button
        className="w-full flex items-center gap-1.5 px-3.5 h-[30px] text-[11px] cursor-pointer hover:opacity-80"
        style={{ color: 'var(--text-tertiary)' }}
        onClick={() => setCollapsed(!collapsed)}
      >
        <svg
          width="11"
          height="11"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          style={{ transform: collapsed ? 'rotate(-90deg)' : 'rotate(0deg)', transition: 'transform 0.15s ease' }}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
        <span>思考过程</span>
        {reasoningCount > 0 && <span className="opacity-60">· {reasoningCount} 步推理</span>}
        {toolCount > 0 && <span className="opacity-60">· {toolCount} 次工具调用</span>}
      </button>

      <div className="px-2.5 pb-2">
        {items.map((item) => {
          if (item.kind === 'reasoning') {
            reasoningSeq += 1;
            const key = reasoningKeyOf(item.id);
            const expanded = resolveExpanded(key, item.id);
            return (
              <div key={key}>
                <button
                  className="w-full flex items-center gap-1.5 py-0.5 text-left cursor-pointer hover:opacity-80"
                  onClick={() => toggleItem(key)}
                >
                  <span className="text-[10px] shrink-0 mt-0.5" style={{ color: 'var(--accent, #7c3aed)' }}>💭</span>
                  <span className="text-[11px] min-w-0 flex-1" style={{ color: 'var(--text-secondary)' }}>
                    推理步骤 {reasoningSeq}
                  </span>
                  <svg
                    width="10"
                    height="10"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.5"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    className="shrink-0 opacity-60"
                    style={{ transform: expanded ? 'rotate(0deg)' : 'rotate(-90deg)', transition: 'transform 0.15s ease' }}
                  >
                    <polyline points="6 9 12 15 18 9" />
                  </svg>
                </button>
                {expanded && (
                  <div className="pl-4 mt-0.5 text-[11px] break-all whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
                    {item.content}
                  </div>
                )}
              </div>
            );
          }
          if (item.kind === 'tool') {
            const t = item.tool;
            const key = toolKeyOf(t.id);
            // 工具调用未完成（流式进行中）时自动展开，结果到达后自动折叠。
            const expanded = resolveExpanded(key, t.id, !t.done);
            return (
              <div key={key} className="py-0.5">
                {/* 工具调用行：🔧 调用 xxx + 完成徽标（✅/❌，调用中无） */}
                <button
                  className="w-full flex items-center gap-1.5 text-left cursor-pointer hover:opacity-80"
                  onClick={() => toggleItem(key)}
                >
                  <span className="text-[10px] shrink-0 mt-0.5">🔧</span>
                  <span className="text-[11px] min-w-0 flex-1" style={{ color: 'var(--text-secondary)' }}>
                    调用 <code className="text-[10px] px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>{t.name}</code>
                  </span>
                  {t.done && (
                    <span className="text-[10px] shrink-0" style={{ color: t.success ? 'var(--success, #22c55e)' : 'var(--danger, #ef4444)' }}>
                      {t.success ? '✅ 完成' : '❌ 失败'}
                    </span>
                  )}
                  <svg
                    width="10"
                    height="10"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.5"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    className="shrink-0 opacity-60"
                    style={{ transform: expanded ? 'rotate(0deg)' : 'rotate(-90deg)', transition: 'transform 0.15s ease' }}
                  >
                    <polyline points="6 9 12 15 18 9" />
                  </svg>
                </button>
                {expanded && (
                  <div className="pl-4 mt-0.5">
                    {(() => {
                      // 参数：优先结构化解析展示；非 JSON / 解析失败时回退原文（不丢信息）。
                      let displayArgs = t.args ?? '';
                      if (t.args) {
                        try {
                          const parsed = JSON.parse(t.args);
                          if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
                            displayArgs = Object.entries(parsed)
                              .map(([k, v]) => `${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`)
                              .join(', ');
                          }
                        } catch { /* 回退原文 */ }
                      }
                      // 结果：完整显示（不截断），避免"信息不全"。
                      const resultText = collapseBlankLines(t.result ?? '');
                      return (
                        <>
                          {displayArgs && (
                            <div className="text-[10px] break-all whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
                              参数：{displayArgs}
                            </div>
                          )}
                          {t.result !== undefined && (
                            <div className="text-[10px] break-all" style={{ color: 'var(--text-tertiary)' }}>
                              {resultText && <span className="whitespace-pre-wrap opacity-75">{resultText}</span>}
                            </div>
                          )}
                        </>
                      );
                    })()}
                  </div>
                )}
              </div>
            );
          }
          if (item.kind === 'diff') {
            const diff = collapseBlankLines(item.diff ?? '');
            const key = diffKeyOf(item.id);
            const expanded = resolveExpanded(key, item.id);
            return (
              <div key={key}>
                <button
                  className="w-full flex items-center gap-1.5 py-0.5 text-left cursor-pointer hover:opacity-80"
                  onClick={() => toggleItem(key)}
                >
                  <span className="text-[10px] shrink-0 mt-0.5">📝</span>
                  <span className="text-[11px] min-w-0 flex-1 truncate" style={{ color: 'var(--text-secondary)' }}>{item.filePath}</span>
                  <svg
                    width="10"
                    height="10"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.5"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    className="shrink-0 opacity-60"
                    style={{ transform: expanded ? 'rotate(0deg)' : 'rotate(-90deg)', transition: 'transform 0.15s ease' }}
                  >
                    <polyline points="6 9 12 15 18 9" />
                  </svg>
                </button>
                {expanded && (
                  <pre
                    className="mt-0.5 px-2 py-1 text-[10px] leading-4 overflow-x-auto whitespace-pre-wrap break-all"
                    style={{ color: 'var(--text-primary)', fontFamily: 'var(--font-mono, monospace)', backgroundColor: 'var(--bg-tertiary)' }}
                  >
                    {diff}
                  </pre>
                )}
              </div>
            );
          }
          return null;
        })}
      </div>
    </div>
  );
}
