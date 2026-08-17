import { useEffect, useRef, useState } from 'react';
import type { ThinkingChainStep } from '../layout/MainPanel';
import { collapseBlankLines } from './MarkdownRenderer';

interface ThinkingChainProps {
  steps: ThinkingChainStep[];
  defaultCollapsed?: boolean;
}

/**
 * 思维链/工具调用聚合展示（可折叠面板），会话模式与群聊模式共用。
 * 从 MessageBubble 抽取，避免两处重复实现工具调用的聚合展示逻辑。
 */
export function ThinkingChain({ steps, defaultCollapsed = true }: ThinkingChainProps) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (containerRef.current && !collapsed && steps.length > 0) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [steps, collapsed]);

  if (steps.length === 0) return null;

  const reasoningCount = steps.filter((s) => s.type === 'reasoning').length;
  const toolCount = steps.filter((s) => s.type === 'tool_start').length;

  return (
    <div
      className="w-full mb-2.5 rounded-lg overflow-hidden"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '0.5px solid var(--border)',
        maxHeight: collapsed ? '30px' : '220px',
        transition: 'max-height 0.2s ease',
      }}
    >
      <button
        className="w-full flex items-center gap-1.5 px-3.5 py-1 text-[11px] cursor-pointer hover:opacity-80"
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
      {!collapsed && (
        <div ref={containerRef} className="px-2.5 pb-2 overflow-y-auto" style={{ maxHeight: '190px' }}>
          {steps.map((step) => {
            if (step.type === 'reasoning') {
              return (
                <div key={step.id} className="flex items-start gap-1.5 py-0.5">
                  <span className="text-[10px] shrink-0 mt-0.5" style={{ color: 'var(--accent, #7c3aed)' }}>💭</span>
                  <span className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>{step.content}</span>
                </div>
              );
            }
            if (step.type === 'tool_start') {
              let displayArgs = '';
              try {
                if (step.toolArgs) {
                  const parsed = JSON.parse(step.toolArgs);
                  displayArgs = Object.entries(parsed)
                    .map(([k, v]) => `${k}=${typeof v === 'string' ? v.slice(0, 40) : String(v).slice(0, 40)}`)
                    .join(', ');
                }
              } catch { /* ignore */ }
              return (
                <div key={step.id} className="flex items-start gap-1.5 py-0.5">
                  <span className="text-[10px] shrink-0 mt-0.5">🔧</span>
                  <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                    调用 <code className="text-[10px] px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>{step.toolName}</code>
                    {displayArgs ? ` (${displayArgs})` : ''}
                  </span>
                </div>
              );
            }
            if (step.type === 'tool_result') {
              const maxLen = 200;
              const result = collapseBlankLines(step.toolResult || '');
              const truncated = result.length > maxLen ? result.slice(0, maxLen) + '...' : result;
              return (
                <div key={step.id} className="flex items-start gap-1.5 py-0.5">
                  <span className="text-[10px] shrink-0 mt-0.5">{step.toolSuccess ? '✅' : '❌'}</span>
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {step.toolSuccess ? '完成' : '失败'}: <code className="text-[10px] px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>{step.toolName}</code>
                    {result && <span className="block mt-0.5 text-[10px] opacity-75 whitespace-pre-wrap break-all">{truncated}</span>}
                  </span>
                </div>
              );
            }
            if (step.type === 'file_diff') {
              const diff = collapseBlankLines(step.fileDiff || '');
              return (
                <div key={step.id} className="my-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                  <div className="px-2 py-1 text-[10px] flex items-center gap-1" style={{ color: 'var(--text-secondary)', borderBottom: '1px solid var(--border)' }}>
                    <span>📝</span>
                    <span className="truncate" title={step.filePath}>{step.filePath}</span>
                  </div>
                  <pre className="px-2 py-1 text-[10px] leading-4 overflow-x-auto whitespace-pre-wrap break-all" style={{ color: 'var(--text-primary)', fontFamily: 'var(--font-mono, monospace)' }}>{diff}</pre>
                </div>
              );
            }
            return null;
          })}
        </div>
      )}
    </div>
  );
}
