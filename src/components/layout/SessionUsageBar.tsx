import { useEffect, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import type { UsageTotals } from '../../types';

interface UsagePayload {
  sessionId: string;
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  cachedTokens?: number;
}

interface Acc {
  prompt: number;
  completion: number;
  cached: number;
}

function fmt(n: number): string {
  const trim = (s: string) => s.replace(/\.?0+$/, '');
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(2))}m`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(2))}k`;
  return String(n);
}

/**
 * 会话内用量条：位于输入框底部。
 * - 仅「API 直连」会话常驻显示：先加载该会话持久化累计（api_usage_log）作为基数，
 *   再对 `agent-usage` 实时事件累加；切换会话重新加载/清零。
 * - CLI/子进程会话（claude/codex/hermes 等）无 token 用量体系，不显示，避免误导性 0。
 */
export function SessionUsageBar({
  sessionId,
  agentType,
}: {
  sessionId: string | null;
  agentType: string | null;
}) {
  const [acc, setAcc] = useState<Acc>({ prompt: 0, completion: 0, cached: 0 });

  const isApi = agentType === 'api';

  useEffect(() => {
    setAcc({ prompt: 0, completion: 0, cached: 0 });
    if (!sessionId || agentType !== 'api') return;
    let disposed = false;
    let un: UnlistenFn | null = null;

    (async () => {
      // 先以 DB 持久化累计为基数（非首次会话不再从 0 开始）。
      try {
        const base = await invoke<UsageTotals>('get_session_usage', { sessionId });
        if (disposed) return;
        setAcc({
          prompt: base.promptTokens ?? 0,
          completion: base.completionTokens ?? 0,
          cached: base.cachedTokens ?? 0,
        });
      } catch {
        // 加载失败按 0 处理，实时事件仍可累加。
      }
      const u = await listen<UsagePayload>('agent-usage', (event) => {
        if (disposed) return;
        if (event.payload.sessionId !== sessionId) return;
        setAcc((prev) => ({
          prompt: prev.prompt + (event.payload.promptTokens ?? 0),
          completion: prev.completion + (event.payload.completionTokens ?? 0),
          cached: prev.cached + (event.payload.cachedTokens ?? 0),
        }));
      });
      if (disposed) {
        u();
      } else {
        un = u;
      }
    })();

    return () => {
      disposed = true;
      if (un) un();
    };
  }, [sessionId, agentType]);

  if (!sessionId || !isApi) return null;

  const rate = acc.prompt > 0 ? Math.min(acc.cached, acc.prompt) / acc.prompt : 0;

  return (
    <div
      className="flex items-center gap-3 px-4 py-1 text-[10px]"
      style={{ color: 'var(--text-tertiary)' }}
    >
      <span>本会话用量（含历史累计）</span>
      <span>输入 {fmt(acc.prompt)} tok</span>
      <span>输出 {fmt(acc.completion)} tok</span>
      <span>缓存命中 {fmt(acc.cached)} tok</span>
      <span>命中率 {(rate * 100).toFixed(1)}%</span>
    </div>
  );
}
