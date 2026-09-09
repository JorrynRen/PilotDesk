import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { SettingsSection, SettingsButton } from '../settings';
import { getUsageSummary, type UsageSummary, type UsageGroup } from '../../types';

function fmt(n: number): string {
  const trim = (s: string) => s.replace(/\.?0+$/, '');
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(2))}m`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(2))}k`;
  return n.toLocaleString('en-US');
}

function fmtRate(rate: number): string {
  return `${rate.toFixed(1)}%`;
}

function Bar({ value }: { value: number }) {
  const width = Math.max(0, Math.min(100, value));
  return (
    <div className="w-full h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--bg-tertiary)' }}>
      <div
        className="h-full rounded-full"
        style={{
          width: `${width}%`,
          background: 'var(--accent, #6366F1)',
        }}
      />
    </div>
  );
}

function StatCell({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex-1 min-w-[90px] rounded-lg px-3 py-2" style={{ background: 'var(--bg-tertiary)' }}>
      <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{label}</div>
      <div className="text-sm font-medium mt-0.5" style={{ color: 'var(--text-primary)' }}>{value}</div>
    </div>
  );
}

function GroupTable({ title, rows }: { title: string; rows: UsageGroup[] }) {
  return (
    <div>
      <div className="text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>{title}</div>
      {rows.length === 0 ? (
        <div className="text-xs py-2" style={{ color: 'var(--text-tertiary)' }}>暂无数据</div>
      ) : (
        <table className="w-full text-xs">
          <thead>
            <tr style={{ color: 'var(--text-tertiary)' }}>
              <th className="text-left py-1">名称</th>
              <th className="text-right py-1">调用</th>
              <th className="text-right py-1">输入(Token)</th>
              <th className="text-right py-1">输出(Token)</th>
              <th className="text-right py-1">缓存命中(Token)</th>
              <th className="text-right py-1">命中率</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((g) => (
              <tr key={g.name} style={{ color: 'var(--text-secondary)' }}>
                <td className="py-1 pr-2" style={{ color: 'var(--text-primary)' }}>{g.name}</td>
                <td className="text-right py-1">{fmt(g.totals.callCount)}</td>
                <td className="text-right py-1">{fmt(g.totals.promptTokens)}</td>
                <td className="text-right py-1">{fmt(g.totals.completionTokens)}</td>
                <td className="text-right py-1">{fmt(g.totals.cacheReadTokens)}</td>
                <td className="text-right py-1">{fmt(g.totals.cacheWriteTokens)}</td>
                <td className="text-right py-1">{fmtRate(g.totals.cacheHitRate)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

const RANGES: { label: string; value: number }[] = [
  { label: '全部', value: 0 },
  { label: '近 30 天', value: 30 },
  { label: '近 7 天', value: 7 },
];

export function UsageStats() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [days, setDays] = useState(30);
  const [tab, setTab] = useState<'provider' | 'model'>('provider');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const seqRef = useRef(0);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unlistenersRef = useRef<UnlistenFn[]>([]);
  const daysRef = useRef(days);

  const refresh = async (forDays = days) => {
    const seq = ++seqRef.current;
    setLoading(true);
    try {
      const data = await getUsageSummary(forDays);
      if (seq !== seqRef.current) return;
      setSummary(data);
      setError('');
    } catch (e) {
      if (seq !== seqRef.current) return;
      setError(String(e));
    } finally {
      if (seq === seqRef.current) setLoading(false);
    }
  };

  useEffect(() => {
    daysRef.current = days;
  }, [days]);

  useEffect(() => {
    refresh(days);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [days]);

  useEffect(() => {
    let disposed = false;
    const onDirty = () => {
      if (disposed) return;
      if (document.hidden) return;
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        if (!disposed) refresh(daysRef.current);
      }, 400);
    };

    const onVisibility = () => {
      if (!document.hidden && !disposed) refresh(daysRef.current);
    };

    (async () => {
      const p1 = listen('usage-recorded', onDirty);
      const p2 = listen('agent-usage', onDirty);
      const [u1, u2] = await Promise.all([p1, p2]);
      if (disposed) {
        u1();
        u2();
        return;
      }
      unlistenersRef.current.push(u1, u2);
    })();

    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      disposed = true;
      if (debounceRef.current) clearTimeout(debounceRef.current);
      unlistenersRef.current.forEach((u) => u());
      unlistenersRef.current = [];
      document.removeEventListener('visibilitychange', onVisibility);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const t = summary?.totals;

  return (
    <SettingsSection
      title="用量统计"
      description="Token 用量与缓存命中（含单 Agent 会话与群聊调用）。缓存读=前缀缓存命中读取、缓存写=缓存写入（如 Anthropic cache_creation）；命中率 = 缓存读 / (输入 + 读 + 写)。部分服务端不返回缓存字段时读/写为 0。"
    >
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-1">
          {RANGES.map((r) => (
            <button
              key={r.value}
              onClick={() => setDays(r.value)}
              className="px-2 py-0.5 rounded text-xs"
              style={{
                background: days === r.value ? 'var(--bg-tertiary)' : 'transparent',
                color: days === r.value ? 'var(--text-primary)' : 'var(--text-secondary)',
                border: '1px solid var(--border)',
              }}
            >
              {r.label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          {loading && (
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>刷新中…</span>
          )}
          <SettingsButton onClick={() => refresh(days)} variant="secondary" disabled={loading}>
            刷新
          </SettingsButton>
        </div>
      </div>

      {error && (
        <div className="text-xs mb-2" style={{ color: 'var(--danger, #EF4444)' }}>{error}</div>
      )}

      {!summary ? (
        <div className="text-xs py-3" style={{ color: 'var(--text-tertiary)' }}>
          {loading ? '加载中…' : '暂无用量数据'}
        </div>
      ) : (
        <>
          <div className="flex flex-wrap gap-2 mb-3">
            <StatCell label="调用次数" value={fmt(t?.callCount ?? 0)} />
            <StatCell label="输入(Token)" value={fmt(t?.promptTokens ?? 0)} />
            <StatCell label="输出(Token)" value={fmt(t?.completionTokens ?? 0)} />
            <StatCell label="合计(Token)" value={fmt(t?.totalTokens ?? 0)} />
            <StatCell label="缓存命中(Token)" value={fmt(t?.cachedTokens ?? 0)} />
            <StatCell label="缓存读(Token)" value={fmt(t?.cacheReadTokens ?? 0)} />
            <StatCell label="缓存写(Token)" value={fmt(t?.cacheWriteTokens ?? 0)} />
          </div>
          <div className="mb-3">
            <div className="flex items-center justify-between text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
              <span>缓存命中率（读 / 输入+读+写）</span>
              <span style={{ color: 'var(--text-primary)' }}>{fmtRate(t?.cacheHitRate ?? 0)}</span>
            </div>
            <Bar value={t?.cacheHitRate ?? 0} />
          </div>

          <div className="flex items-center gap-2 mb-2">
            {(['provider', 'model'] as const).map((key) => (
              <button
                key={key}
                onClick={() => setTab(key)}
                className="px-2 py-0.5 rounded text-xs"
                style={{
                  background: tab === key ? 'var(--bg-tertiary)' : 'transparent',
                  color: tab === key ? 'var(--text-primary)' : 'var(--text-secondary)',
                  border: '1px solid var(--border)',
                }}
              >
                {key === 'provider' ? '按 Provider' : '按 Model'}
              </button>
            ))}
          </div>

          {tab === 'provider' ? (
            <GroupTable title="按 Provider" rows={summary.byProvider} />
          ) : (
            <GroupTable title="按 Model" rows={summary.byModel} />
          )}

          {summary.trend.length > 0 && (
            <div className="mt-3">
              <div className="text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
                趋势（按天命中率）
              </div>
              {summary.trend.map((d) => (
                <div key={d.date} className="flex items-center gap-2 mb-1">
                  <span className="text-[10px] w-20 shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                    {d.date}
                  </span>
                  <div className="flex-1"><Bar value={d.cacheHitRate} /></div>
                  <span className="text-[10px] w-14 text-right" style={{ color: 'var(--text-secondary)' }}>
                    {fmtRate(d.cacheHitRate)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </SettingsSection>
  );
}
