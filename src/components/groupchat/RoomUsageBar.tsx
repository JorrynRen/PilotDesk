import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { getRoomUsage, type RoomUsage, type UsageGroup } from '../../types';

function fmt(n: number): string {
  const trim = (s: string) => s.replace(/\.?0+$/, '');
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(2))}m`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(2))}k`;
  return n.toLocaleString('en-US');
}

function fmtRate(rate: number): string {
  return `${rate.toFixed(1)}%`;
}

function ModelRows({ rows }: { rows: UsageGroup[] }) {
  if (rows.length === 0) return <div className="text-xs py-1" style={{ color: 'var(--text-tertiary)' }}>暂无模型明细</div>;
  return (
    <div className="mt-1.5">
      <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
        <thead>
          <tr style={{ color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border)' }}>
            <th className="text-left py-0.5">模型</th>
            <th className="text-right py-0.5">调用</th>
            <th className="text-right py-0.5" title="输入总量 = 未命中 + 缓存命中 + 缓存写">输入总量</th>
            <th className="text-right py-0.5" title="未命中输入（缓存命中是单独一桶）">输入·未命中</th>
            <th className="text-right py-0.5">输出</th>
            <th className="text-right py-0.5">缓存命中</th>
            <th className="text-right py-0.5">缓存写</th>
            <th className="text-right py-0.5">命中率</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((g) => (
            <tr key={g.name} style={{ color: 'var(--text-secondary)' }}>
              <td className="py-0.5 pr-2" style={{ color: 'var(--text-primary)' }}>{g.name}</td>
              <td className="text-right py-0.5">{fmt(g.totals.callCount)}</td>
              <td className="text-right py-0.5">{fmt(g.totals.promptTokens + g.totals.cacheReadTokens + g.totals.cacheWriteTokens)}</td>
              <td className="text-right py-0.5">{fmt(g.totals.promptTokens)}</td>
              <td className="text-right py-0.5">{fmt(g.totals.completionTokens)}</td>
              <td className="text-right py-0.5">{fmt(g.totals.cacheReadTokens)}</td>
              <td className="text-right py-0.5">{fmt(g.totals.cacheWriteTokens)}</td>
              <td className="text-right py-0.5">{fmtRate(g.totals.cacheHitRate)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * 群聊房间用量合计（输入框底部）：director + 各参与者按房间聚合，
 * 随 `usage-recorded` 事件防抖整量刷新（以服务端为准）；可展开按模型明细。
 */
export function RoomUsageBar({ roomId }: { roomId: string | null }) {
  const [data, setData] = useState<RoomUsage | null>(null);
  const [expanded, setExpanded] = useState(false);
  // 切换房间时清空展示。用「渲染期修正」（React adjust-during-render）而不是 effect——
  // 在 effect 里同步 setState 会多一轮级联渲染（`react-hooks/set-state-in-effect`），
  // 而"外部值（roomId）变了就把本地状态重置"正是该写法的适用场景。
  const [syncedRoomId, setSyncedRoomId] = useState(roomId);
  if (syncedRoomId !== roomId) {
    setSyncedRoomId(roomId);
    setData(null);
    setExpanded(false);
  }
  const seqRef = useRef(0);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = async (rid: string) => {
    const seq = ++seqRef.current;
    try {
      const d = await getRoomUsage(rid);
      if (seq === seqRef.current) setData(d);
    } catch {
      // 忽略，保持旧值
    }
  };

  useEffect(() => {
    if (!roomId) return;
    let disposed = false;
    let un: UnlistenFn | null = null;
    const rid = roomId;

    // effect 体内不允许同步 setState（react-hooks/set-state-in-effect）：把首帧拉取推迟一个微任务，
    // 仍在同一帧内执行，观感与原先一致
    queueMicrotask(() => { void refresh(rid); });

    const onDirty = () => {
      if (disposed || document.hidden) return;
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        if (!disposed) refresh(rid);
      }, 400);
    };

    (async () => {
      const u = await listen('usage-recorded', onDirty);
      if (disposed) {
        u();
      } else {
        un = u;
      }
    })();

    return () => {
      disposed = true;
      if (debounceRef.current) clearTimeout(debounceRef.current);
      if (un) un();
    };
  }, [roomId]);

  if (!roomId || !data || data.totals.callCount === 0) return null;

  const t = data.totals;
  // 命中率 = 缓存读 / (输入 + 读 + 写)，与后端口径一致（避免把缓存写 token 当命中）。
  const read = t.cacheReadTokens ?? 0;
  const write = t.cacheWriteTokens ?? 0;
  const denom = t.promptTokens + read + write;
  const rate = denom > 0 ? (read / denom) * 100 : 0;

  return (
    <div
      className="px-4 py-1 text-[10px] w-full"
      style={{ color: 'var(--text-tertiary)' }}
    >
      <button className="flex items-center gap-2 hover:opacity-90" onClick={() => setExpanded((v) => !v)}>
        {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        <span>本房间用量（含历史累计）</span>
        <span>调用 {fmt(t.callCount)}</span>
        <span style={{ opacity: 0.5 }}>|</span>
        <span title="输入按「总量/未命中」展示；缓存命中是单独一桶">输入 {fmt(t.promptTokens + read + write)}/{fmt(t.promptTokens)}(未命中) tok</span>
        <span style={{ opacity: 0.5 }}>|</span>
        <span>输出 {fmt(t.completionTokens)} tok</span>
        <span style={{ opacity: 0.5 }}>|</span>
        <span title="缓存命中只算缓存读取；命中率 = 缓存命中 / 总输入">缓存命中 {fmt(t.cacheReadTokens)} tok</span>
        <span style={{ opacity: 0.5 }}>|</span>
        <span>命中率 {fmtRate(rate)}</span>
      </button>
      {expanded && (
        <div className="mt-1.5">
          <ModelRows rows={data.byModel} />
        </div>
      )}
    </div>
  );
}
