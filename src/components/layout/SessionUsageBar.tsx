import { useEffect, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import type { UsageTotals } from '../../types';

interface Acc {
  prompt: number;
  completion: number;
  /** 缓存**读**（命中量只算读；写缓存不是命中） */
  cacheRead: number;
  /** 缓存写（仅用于命中率分母） */
  cacheWrite: number;
  /** 调用次数（与房间用量条口径一致，便于横向对比） */
  callCount: number;
}

function fmt(n: number): string {
  const trim = (s: string) => s.replace(/\.?0+$/, '');
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(2))}m`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(2))}k`;
  return String(n);
}

/**
 * 会话内用量条：位于输入框底部。
 *
 * 口径（与后端 `UsageTotals` 一致，勿混用）：
 * - 输入 = promptTokens；输出 = completionTokens；
 * - **缓存命中 = cacheReadTokens**（不是 cachedTokens——那是"读 + 写"之和，写缓存往往远大于读，
 *   拿它当命中量会出现"命中量 > 输入量"、命中率被夹到 100% 的假象）；
 * - 命中率 = cacheRead / (prompt + cacheRead + cacheWrite)。
 *
 * 数据源：只读 `get_session_usage`（api_usage_log 聚合，缓存读写本来就是两列），
 * 每次落库（`usage-recorded`）后按会话重新拉取——粒度与逐次累加一致，但口径不会错。
 * CLI/子进程会话（claude/codex/hermes 等）无 token 用量体系，不显示，避免误导性 0。
 */
export function SessionUsageBar({
  sessionId,
  agentType,
}: {
  sessionId: string | null;
  agentType: string | null;
}) {
  const [acc, setAcc] = useState<Acc>({ prompt: 0, completion: 0, cacheRead: 0, cacheWrite: 0, callCount: 0 });

  const isApi = agentType === 'api';

  // 会话/Agent 变化时清空累计值。用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），两者行为一致。
  const accKey = `${sessionId}|${agentType}`;
  const [prevAccKey, setPrevAccKey] = useState(accKey);
  if (prevAccKey !== accKey) {
    setPrevAccKey(accKey);
    setAcc({ prompt: 0, completion: 0, cacheRead: 0, cacheWrite: 0, callCount: 0 });
  }

  useEffect(() => {
    if (!sessionId || agentType !== 'api') return;
    let disposed = false;
    let un: UnlistenFn | null = null;

    const load = async () => {
      try {
        const t = await invoke<UsageTotals>('get_session_usage', { sessionId });
        if (disposed) return;
        setAcc({
          prompt: t.promptTokens ?? 0,
          completion: t.completionTokens ?? 0,
          cacheRead: t.cacheReadTokens ?? 0,
          cacheWrite: t.cacheWriteTokens ?? 0,
          callCount: t.callCount ?? 0,
        });
      } catch {
        // 读取失败保留上一次数值（首帧为 0），不影响主流程
      }
    };

    void load();

    (async () => {
      const u = await listen<{ sessionId?: string }>('usage-recorded', (event) => {
        if (disposed) return;
        if (event.payload.sessionId && event.payload.sessionId !== sessionId) return;
        void load();
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

  const denom = acc.prompt + acc.cacheRead + acc.cacheWrite;
  const rate = denom > 0 ? acc.cacheRead / denom : 0;

  return (
    <div
      className="flex items-center gap-2 px-4 py-1 text-[10px]"
      style={{ color: 'var(--text-tertiary)' }}
    >
      <span>本会话用量（含历史累计）</span>
      <span>调用 {fmt(acc.callCount)}</span>
      <span style={{ opacity: 0.5 }}>|</span>
      <span title="输入按「总量/未命中」展示；缓存命中是单独一桶">输入 {fmt(acc.prompt + acc.cacheRead + acc.cacheWrite)}/{fmt(acc.prompt)}(未命中) tok</span>
      <span style={{ opacity: 0.5 }}>|</span>
      <span>输出 {fmt(acc.completion)} tok</span>
      <span style={{ opacity: 0.5 }}>|</span>
      <span title="缓存命中只算缓存读取；命中率 = 缓存命中 / 总输入">缓存命中 {fmt(acc.cacheRead)} tok</span>
      <span style={{ opacity: 0.5 }}>|</span>
      <span>命中率 {(rate * 100).toFixed(1)}%</span>
    </div>
  );
}
