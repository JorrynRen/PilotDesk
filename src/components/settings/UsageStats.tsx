import { useEffect, useRef, useState, type ReactNode } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { ChevronDown } from 'lucide-react';
import { SettingsSection, SettingsButton } from '../settings';
import { showToast } from '../../utils/toast';
import {
  getUsageSummary,
  getUsageAttribution,
  type UsageSummary,
  type UsageAttribution,
  type UsageGroup,
  type UsageDay,
} from '../../types';
import { errorMessage } from '../../utils/errorMessage';

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

/**
 * 命中率口径说明（放在命中率数字下方）。
 *
 * 页头 description 已经讲过「四桶互斥 / 缓存写只有 Anthropic 上报故恒为 0 / 命中率公式」，
 * 这里只补它**没讲**、而恰好是与模型商后台对不上时最需要的两点：
 * ① 缓存读、分母三桶都取自模型商上报值（不是估算），但**比率本身是本机算的**，展示公式可能与厂商不同；
 * ② 口径只覆盖经本应用发起的调用 —— 终端里直接跑 CLI 的用量根本不入库，厂商后台却会算进去。
 */
const CACHE_RATE_NOTE =
  '命中率由本机按上式计算的派生值（分子分母均取模型商上报值，非估算），与模型商后台的展示口径可能不同；'
  + '且只统计经本应用发起的调用 —— 终端里直接跑 CLI 的用量不入库，而厂商后台会计入。';

/* ── 趋势：按区间跨度自动换粒度（日 / 周 / 月）─────────────────────────────
   为什么必须聚合：「全部」会返回表里每一个有数据的日子，逐日一行时用一年就是几百行 ——
   那既看不出形状（是清单不是趋势），又把页面拉成一条长带。
   粒度按**时间跨度**而不是行数决定：跨度才是决定横轴密度的量（用得少的人行数少、跨度却可能很大）。 */
type TrendGranularity = 'day' | 'week' | 'month';

function granularityOf(spanDays: number): TrendGranularity {
  if (spanDays <= 60) return 'day';
  if (spanDays <= 420) return 'week';
  return 'month';
}

/** 某个日期属于哪个桶：日 = 当天；周 = 当周周一（周一为一周之始）；月 = `YYYY-MM`。 */
function bucketKey(date: string, g: TrendGranularity): string {
  if (g === 'day') return date;
  if (g === 'month') return date.slice(0, 7);
  const d = new Date(`${date}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() - ((d.getUTCDay() + 6) % 7));
  return d.toISOString().slice(0, 10);
}

/** 下一个桶 key（按 key 自身步进，用来把中间缺数据的桶补出来）。 */
function nextKey(key: string, g: TrendGranularity): string {
  if (g === 'month') {
    const [y, m] = key.split('-').map(Number);
    return m >= 12 ? `${y + 1}-01` : `${y}-${String(m + 1).padStart(2, '0')}`;
  }
  const d = new Date(`${key}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() + (g === 'week' ? 7 : 1));
  return d.toISOString().slice(0, 10);
}

interface TrendBucket {
  key: string;
  /** 桶内实际有数据的天数（周/月聚合时用来说明"这几天合计"） */
  days: number;
  /** 未命中输入 */
  prompt: number;
  /** 缓存命中读取 */
  read: number;
  /** 缓存写入 */
  write: number;
  /** 输入总量 = 未命中 + 缓存命中 + 缓存写（与「输入·总量」列、命中率分母同口径；不含输出） */
  input: number;
  hitRate: number;
}

/**
 * 把逐日趋势聚合到指定粒度。
 *
 * 两个关键点：
 * 1. 命中率是**比率**，聚合时**不能取平均** —— 必须按桶求和后重算（Σ读 / Σ(未命中+读+写)），
 *    否则"某天只调了 1 次、命中 100%"会把整周拉高；
 * 2. 中间没有数据的桶补 0：横轴要按时距等距，否则"隔了半年"会被画成相邻两根柱（假趋势）。
 */
function bucketize(trend: UsageDay[], g: TrendGranularity): TrendBucket[] {
  const acc = new Map<string, { days: number; prompt: number; read: number; write: number }>();
  for (const d of trend) {
    const k = bucketKey(d.date, g);
    const cur = acc.get(k) ?? { days: 0, prompt: 0, read: 0, write: 0 };
    cur.days += 1;
    cur.prompt += d.promptTokens ?? 0;
    cur.read += d.cacheReadTokens ?? 0;
    cur.write += d.cacheWriteTokens ?? 0;
    acc.set(k, cur);
  }
  if (acc.size === 0) return [];
  const keys = [...acc.keys()].sort(); // 同格式的 ISO key：字典序即时序
  const zero = { days: 0, prompt: 0, read: 0, write: 0 };
  const out: TrendBucket[] = [];
  // 上限只是防 key 步进出错时死循环（正常最多几百个桶）
  for (let k = keys[0]; k <= keys[keys.length - 1] && out.length < 4000; k = nextKey(k, g)) {
    const v = acc.get(k) ?? zero;
    const input = v.prompt + v.read + v.write;
    out.push({
      key: k,
      days: v.days,
      prompt: v.prompt,
      read: v.read,
      write: v.write,
      input,
      hitRate: input > 0 ? (v.read / input) * 100 : 0,
    });
  }
  return out;
}

/** 输入构成三桶的柱色：同一主色的深浅阶梯，含义由图例给（不引入新色相）。 */
const SEG_READ = 'var(--accent)';
const SEG_PROMPT = 'color-mix(in srgb, var(--accent) 18%, var(--bg-tertiary))';
const SEG_WRITE = 'color-mix(in srgb, var(--accent) 50%, var(--bg-tertiary))';

/**
 * 时间轴柱状图：每桶一列，柱高 = 值 / 上限。用 flex 列 + 百分比高度画，不引图表库 ——
 * 只有几十根柱，库带来的体积与窄面板自适应成本都不划算。
 *
 * `stacked` 时按三桶堆叠（`flexGrow` 直接给出桶间比例，不必自己算百分比）；
 * 否则单色一列（命中率，纵轴固定 0–100）。
 */
function TrendColumns({
  buckets,
  max,
  height,
  stacked,
  titleOf,
}: {
  buckets: TrendBucket[];
  /** 纵轴上限：输入构成用窗口内峰值；命中率固定 100 */
  max: number;
  height: number;
  stacked?: boolean;
  titleOf: (b: TrendBucket) => string;
}) {
  return (
    <div className="flex items-stretch gap-px" style={{ height }}>
      {buckets.map((b) => {
        const value = stacked ? b.input : b.hitRate;
        const h = max > 0 ? Math.min(100, (value / max) * 100) : 0;
        // 单色时的 flexGrow 取 1：柱高已由外层 height 决定，段内比例无所谓
        const segs = stacked
          ? [
              { g: b.write, c: SEG_WRITE },
              { g: b.prompt, c: SEG_PROMPT },
              { g: b.read, c: SEG_READ },
            ]
          : [{ g: 1, c: SEG_READ }];
        return (
          <div key={b.key} className="flex-1 min-w-0 flex flex-col justify-end" title={titleOf(b)}>
            <div className="flex flex-col overflow-hidden" style={{ height: `${h}%`, borderRadius: '2px 2px 0 0' }}>
              {segs.map((s, i) => (
                <div key={i} style={{ flexGrow: s.g, backgroundColor: s.c }} />
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function StatCell({ label, value }: { label: string; value: string }) {
  return (
  // 每行 4 个（窄屏自动减少），避免 7 个指标挤在一行、最后一个落单
    <div
      className="min-w-[110px] rounded-lg px-3 py-2"
      style={{ background: 'var(--bg-tertiary)', flex: '1 1 calc(25% - 6px)' }}
    >
      <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{label}</div>
      <div className="text-sm font-medium mt-0.5" style={{ color: 'var(--text-primary)' }}>{value}</div>
    </div>
  );
}

/**
 * 用量页的内容块：块标题 + 上分隔线。
 *
 * 这一页纵向叠了「概览 / 明细 / 趋势」三块，但原来块与块之间只有一点间距 —— 卡片、表格、
 * 说明文字、命中率条连成一片，看不出哪里换了另一件事。统一用块头分开，
 * 顺带给每块一句话说明（这一页数字口径很多，标题比分隔线更省解释）。
 */
function Block({
  title,
  hint,
  first,
  children,
}: {
  title: string;
  hint?: string;
  /** 第一块不画上分隔线（它紧跟在时间范围工具条后面） */
  first?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      className={first ? '' : 'mt-3 pt-3'}
      style={first ? undefined : { borderTop: '1px solid var(--border)' }}
    >
      <div className="flex items-baseline gap-2 mb-2">
        {/* 比区块标题（`SettingsSection` 的 text-xs）小一档：它是页内的分块，不能与页面标题抢层级 */}
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>{title}</span>
        {hint && (
          <span className="text-[10px] truncate min-w-0" style={{ color: 'var(--text-tertiary)' }}>{hint}</span>
        )}
      </div>
      {children}
    </section>
  );
}

function GroupTable({ title, rows, action }: { title: string; rows: UsageGroup[]; action?: ReactNode }) {
  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-1">
        <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{title}</div>
        {action}
      </div>
      {rows.length === 0 ? (
        <div className="text-xs py-2" style={{ color: 'var(--text-tertiary)' }}>暂无数据</div>
      ) : (
        // `borderCollapse: collapse` 是必须的：默认 separate 模式下 `tr` 的 border 不渲染，
        // 表头那条分隔线就画不出来（表头与内容行会连成一片 —— 只靠加粗在小字号下看不出来）
        <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
          <thead>
            <tr
              className="whitespace-nowrap"
              style={{
                color: 'var(--text-secondary)',
                backgroundColor: 'var(--bg-tertiary)',
                borderBottom: '1px solid var(--border)',
              }}
            >
              <th className="text-left py-1.5">名称</th>
              <th className="text-right py-1.5">调用</th>
              <th className="text-right py-1.5" title="输入总量 = 未命中 + 缓存命中 + 缓存写">输入总量</th>
              <th className="text-right py-1.5" title="未命中输入（缓存命中是单独一桶）">输入·未命中</th>
              <th className="text-right py-1.5">输出</th>
              <th className="text-right py-1.5">缓存命中</th>
              <th className="text-right py-1.5">缓存写</th>
              <th className="text-right py-1.5">命中率</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((g, i) => (
              // 会话维度可能出现同名行（标题可重复），以序号兜底保证 key 唯一
              <tr
                key={`${g.name}-${i}`}
                style={{ color: 'var(--text-secondary)', borderBottom: '1px solid var(--border)' }}
              >
                <td className="py-1.5 pr-2" style={{ color: 'var(--text-primary)' }}>
                  {/* 名称限宽 + 省略号：会话标题可能很长，避免挤压右侧统计列 */}
                  <span className="block truncate" style={{ maxWidth: 180 }} title={g.name}>
                    {g.name}
                  </span>
                </td>
                <td className="text-right py-1.5">{fmt(g.totals.callCount)}</td>
                <td className="text-right py-1.5">{fmt(g.totals.promptTokens + g.totals.cacheReadTokens + g.totals.cacheWriteTokens)}</td>
                <td className="text-right py-1.5">{fmt(g.totals.promptTokens)}</td>
                <td className="text-right py-1.5">{fmt(g.totals.completionTokens)}</td>
                <td className="text-right py-1.5">{fmt(g.totals.cacheReadTokens)}</td>
                <td className="text-right py-1.5">{fmt(g.totals.cacheWriteTokens)}</td>
                <td className="text-right py-1.5">{fmtRate(g.totals.cacheHitRate)}</td>
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

/** 一级分页：provider/model 是「按什么调的」，归因是「谁花的」。 */
type TabKey = 'provider' | 'model' | 'attribution';

/** 归因二级维度（仅「按归因」下钻用）。 */
type DimensionKey = 'session' | 'groupchat' | 'workflow' | 'knowledge';

const TABS: { key: TabKey; label: string }[] = [
  { key: 'provider', label: '按 Provider' },
  { key: 'model', label: '按 Model' },
  { key: 'attribution', label: '按归因' },
];

/** 维度展示名（key 与后端 `UsageDimension.key` 一一对应，顺序固定）。 */
const DIMENSION_ORDER: DimensionKey[] = ['session', 'groupchat', 'workflow', 'knowledge'];
const DIMENSION_META: Record<DimensionKey, { label: string; title: string }> = {
  session: { label: '会话', title: '按会话（一行一会话）' },
  groupchat: { label: '群聊', title: '按群聊（按房间聚合）' },
  workflow: { label: '工作流', title: '按工作流（按定义聚合）' },
  knowledge: {
    label: '知识库',
    title: '按知识库（含投喂整理 / 文件补整理 / 片段与工作沉淀 / 对话沉淀 / AI 生成）',
  },
};

/** 归因维度口径提示：CLI Agent 不经宿主，故各维度合计可能小于全局总量。 */
const DIMENSION_HINT =
  '仅统计经宿主发起的 API 调用；CLI Agent（终端 / 插件 / claude、codex 等子进程）自行计费、不回流用量，故各维度合计可能小于全局总量。';

export function UsageStats() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [attribution, setAttribution] = useState<UsageAttribution | null>(null);
  const [days, setDays] = useState(30);
  const [tab, setTab] = useState<TabKey>('provider');
  /** 归因下钻维度；'summary' = 默认只显示三维度聚合总览。 */
  const [dimension, setDimension] = useState<'summary' | DimensionKey>('summary');
  const [dimMenuOpen, setDimMenuOpen] = useState(false);
  const dimMenuRef = useRef<HTMLDivElement | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  /**
   * 知识库 id → 库名。
   *
   * 知识库维度的分组名**只能拿到库 id**：库定义在 MEMORY.db，用量归因查的是主库，跨库不 JOIN
   * （见 `commands/usage.rs::usage_attribution`）。所以名字在这一层补 —— 映射不到就显示 id
   * （库被删了也能看懂是哪一条），不影响其它维度。
   */
  const [kbNames, setKbNames] = useState<Record<string, string>>({});
  const seqRef = useRef(0);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unlistenersRef = useRef<UnlistenFn[]>([]);
  const daysRef = useRef(days);

  const refresh = async (forDays = days) => {
    const seq = ++seqRef.current;
    setLoading(true);
    try {
      const [data, attr] = await Promise.all([
        getUsageSummary(forDays),
        getUsageAttribution(forDays),
      ]);
      if (seq !== seqRef.current) return;
      setSummary(data);
      setAttribution(attr);
      setError('');
    } catch (e) {
      if (seq !== seqRef.current) return;
      setError(errorMessage(e));
    } finally {
      if (seq === seqRef.current) setLoading(false);
    }
  };

  /** 导出报表：选保存路径后由后端按当前时间窗口生成 CSV（对所有用户开放） */
  const handleExportReport = async () => {
    try {
      const filePath = await saveDialog({
        defaultPath: 'usage-report.csv',
        filters: [{ name: 'CSV', extensions: ['csv'] }],
      });
      if (!filePath) return;
      await invoke('export_usage_report_csv', { days, filePath });
      showToast('报表导出成功', 'success');
    } catch (e) {
      showToast(`报表导出失败: ${errorMessage(e)}`, 'error');
    }
  };

  useEffect(() => {
    daysRef.current = days;
  }, [days]);

  // 知识库名映射：挂载时拉一次（库列表变动很少，失败就退化成显示 id）
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const bases = await invoke<{ id: string; name: string }[]>('kb_list_bases');
        if (!alive) return;
        setKbNames(Object.fromEntries(bases.map((b) => [b.id, b.name])));
      } catch {
        /* 拿不到就显示 id —— 用量页不该因为知识库模块出错而不可用 */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    // 不在 effect 里同步调用 `refresh`：它开头就 setLoading(true)，属于"effect 体内同步 setState"
    // （`react-hooks/set-state-in-effect` 判为级联渲染）。推到微任务 —— 同一个任务、早于绘制，行为一致。
    void Promise.resolve().then(() => refresh(days));
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

  // 点击外部关闭维度下拉
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (dimMenuRef.current && !dimMenuRef.current.contains(e.target as Node)) {
        setDimMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, []);

  const t = summary?.totals;
  const dimensions = attribution?.dimensions ?? [];
  // 默认总览：每个维度各一行（顺序与后端一致：会话 → 群聊 → 工作流 → 知识库）
  const summaryRows: UsageGroup[] = dimensions.map((d) => ({
    name: DIMENSION_META[d.key as DimensionKey]?.label ?? d.key,
    totals: d.totals,
  }));
  // 知识库维度的分组名是库 id → 在这一层换成库名（其余维度原样）。
  const detailGroups =
    dimension === 'summary' ? [] : (dimensions.find((d) => d.key === dimension)?.groups ?? []);
  const detailRows: UsageGroup[] =
    dimension === 'knowledge'
      ? detailGroups.map((g) => ({ ...g, name: kbNames[g.name] ?? g.name }))
      : detailGroups;

  /**
   * 趋势图的派生数据：先按**跨度**选粒度（日/周/月），再聚合。
   * 这里读的是数据自带的日期字符串，不读当前时钟。
   */
  const trend = summary?.trend ?? [];
  // 排序只为取最早/最晚（不依赖后端的 ORDER BY）：跨度为 0 时按"按天"处理
  const trendDates = trend.map((d) => d.date).sort();
  const trendSpanDays = trendDates.length > 1
    ? Math.round(
        (Date.parse(`${trendDates[trendDates.length - 1]}T00:00:00Z`) - Date.parse(`${trendDates[0]}T00:00:00Z`)) / 86400000,
      )
    : 0;
  const trendGranularity = granularityOf(trendSpanDays);
  const trendBuckets = bucketize(trend, trendGranularity);
  // 纵轴上限：输入构成的峰值（下限 1 防 0 除）。命中率那根固定 0–100，用不到它。
  const trendPeak = Math.max(1, ...trendBuckets.map((b) => b.input));
  /** 桶的时间说明，用在 tooltip 里（按天时不需要） */
  const bucketScope = trendGranularity === 'week' ? '那一周' : trendGranularity === 'month' ? '当月' : '';
  const granularityLabel =
    trendGranularity === 'day' ? '按天' : trendGranularity === 'week' ? '按周聚合' : '按月聚合';
  const inputTitle = (b: TrendBucket) =>
    `${b.key}${bucketScope}：输入 ${fmt(b.input)}（缓存命中 ${fmt(b.read)} / 未命中 ${fmt(b.prompt)} / 缓存写 ${fmt(b.write)}`
    + `${b.days > 1 ? `，${b.days} 天合计` : ''}）`;
  const rateTitle = (b: TrendBucket) =>
    `${b.key}${bucketScope}：缓存命中率 ${fmtRate(b.hitRate)}${b.days > 1 ? `（${b.days} 天合计后重算）` : ''}`;

  return (
    <SettingsSection
      title="用量统计"
      description="Token 用量与缓存命中（含单 Agent 会话、群聊与知识库整理/生成调用）；可按 Provider / Model 分组，并按会话 / 群聊 / 工作流 / 知识库归因。四桶互斥口径：输入=未命中输入（OpenAI 协议已扣除命中部分）、输出、缓存读=命中读取、缓存写=写入；合计 = 四桶之和。缓存写只有 Anthropic 上报（cache_creation_input_tokens），OpenAI 兼容协议（OpenAI / DeepSeek / Qwen 等）响应里没有该字段，故恒为 0。命中率 = 缓存读 / (输入 + 读 + 写)。"
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
          <SettingsButton
            onClick={() => void handleExportReport()}
            variant="secondary"
            title={days === 0 ? '导出全部用量的 CSV 报表' : `导出近 ${days} 天用量的 CSV 报表`}
          >
            导出报表
          </SettingsButton>
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
          <Block title="概览" hint="所选时间范围内的合计（四桶互斥）" first>
            <div className="flex flex-wrap gap-2 mb-3">
              <StatCell label="调用次数" value={fmt(t?.callCount ?? 0)} />
              <StatCell label="输入·总量(Token)" value={fmt((t?.promptTokens ?? 0) + (t?.cacheReadTokens ?? 0) + (t?.cacheWriteTokens ?? 0))} />
              <StatCell label="输入·未命中(Token)" value={fmt(t?.promptTokens ?? 0)} />
              <StatCell label="输出(Token)" value={fmt(t?.completionTokens ?? 0)} />
              <StatCell label="合计(Token)" value={fmt(t?.totalTokens ?? 0)} />
              <StatCell label="缓存总量(Token)" value={fmt(t?.cachedTokens ?? 0)} />
              <StatCell label="缓存读(Token)" value={fmt(t?.cacheReadTokens ?? 0)} />
              <StatCell label="缓存写(Token)" value={fmt(t?.cacheWriteTokens ?? 0)} />
            </div>
            <div>
              <div className="flex items-center justify-between text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
                <span>缓存命中率（读 / 输入+读+写）</span>
                <span style={{ color: 'var(--text-primary)' }}>{fmtRate(t?.cacheHitRate ?? 0)}</span>
              </div>
              <Bar value={t?.cacheHitRate ?? 0} />
              <div className="text-[10px] leading-relaxed mt-1.5" style={{ color: 'var(--text-tertiary)' }}>
                {CACHE_RATE_NOTE}
              </div>
            </div>
          </Block>

          <Block title="明细" hint="按 Provider / Model / 归因拆分；点上方页签切换">
            <div className="flex items-center gap-2 mb-2 flex-wrap">
              {TABS.map(({ key, label }) => (
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
                  {label}
                </button>
              ))}
              {/* 表格单位统一在这里声明，表头不再逐个带单位（列多时表头会挤不下） */}
              <span className="text-[10px] ml-auto" style={{ color: 'var(--text-tertiary)' }} title="调用 = 每次模型请求；群聊多参与者、多轮讨论与每轮工具迭代都会各计一次，因此会明显大于「你发起的次数」">单位：token · 调用按每次模型请求计（群聊多轮/多参与者会累计，可在「按归因」页用「场景」维度看拆分）</span>
              {/* 维度明细下拉：紧贴「按归因」之后，选中后展示该维度明细（默认只显示维度汇总） */}
              {tab === 'attribution' && (
                <div className="relative" ref={dimMenuRef}>
                  <button
                    onClick={() => setDimMenuOpen((v) => !v)}
                    className="flex items-center gap-1 px-2 py-0.5 rounded text-xs"
                    style={{
                      background: 'transparent',
                      color: dimension === 'summary' ? 'var(--text-secondary)' : 'var(--text-primary)',
                      border: '1px solid var(--border)',
                    }}
                    title="选择归因维度，查看该维度明细"
                  >
                    {dimension === 'summary' ? '维度明细' : DIMENSION_META[dimension].label}
                    <ChevronDown
                      size={10}
                      style={{
                        transform: dimMenuOpen ? 'rotate(180deg)' : 'none',
                        transition: 'transform 150ms',
                      }}
                    />
                  </button>
                  {dimMenuOpen && (
                    <div
                      className="absolute left-0 top-full mt-1 z-20 min-w-[104px] py-1 rounded-md"
                      style={{
                        background: 'var(--bg-secondary)',
                        border: '1px solid var(--border)',
                        boxShadow: '0 4px 12px rgba(0,0,0,0.18)',
                      }}
                    >
                      {DIMENSION_ORDER.map((key) => (
                        <button
                          key={key}
                          onClick={() => {
                            setDimension(key);
                            setDimMenuOpen(false);
                          }}
                          className="w-full text-left px-2 py-1 text-xs"
                          style={{
                            color: dimension === key ? 'var(--accent)' : 'var(--text-secondary)',
                            background: dimension === key ? 'var(--accent-light)' : 'transparent',
                          }}
                        >
                          {DIMENSION_META[key].label}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>

            {tab === 'provider' && <GroupTable title="按 Provider" rows={summary.byProvider} />}
            {tab === 'model' && <GroupTable title="按 Model" rows={summary.byModel} />}
            {tab === 'attribution' && (
              <>
                <GroupTable title="维度汇总（会话 / 群聊 / 工作流 / 知识库）" rows={summaryRows} />
                {dimension !== 'summary' && (
                  // 二级维度明细与上面的「维度汇总」是两层不同的东西（汇总=全维度合计，
                  // 明细=某一个维度按实体下钻），所以这里也要一条分隔线 —— 只有间距时两张表会连成一片
                  <div className="mt-3 pt-3" style={{ borderTop: '1px solid var(--border)' }}>
                    <GroupTable
                      title={DIMENSION_META[dimension].title}
                      rows={detailRows}
                      action={
                        <button
                          onClick={() => setDimension('summary')}
                          className="text-[10px]"
                          style={{ color: 'var(--accent)' }}
                        >
                          收起
                        </button>
                      }
                    />
                  </div>
                )}
                <div className="text-[10px] mt-1.5 leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  {DIMENSION_HINT}
                </div>
              </>
            )}
          </Block>

          {trendBuckets.length > 0 && (
            <Block
              title="趋势"
              hint={`上图每日输入构成、下图缓存命中率；${granularityLabel}（跨度 ${trendSpanDays} 天）`}
            >
              {/* 输入构成：柱高按窗口内峰值归一（纵轴不是 0–100%，故把峰值标出来） */}
              <div className="flex items-center justify-between text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
                <span>每日输入 Token（柱高按窗口内峰值归一）</span>
                <span>峰值 {fmt(trendPeak)}</span>
              </div>
              <TrendColumns buckets={trendBuckets} max={trendPeak} height={96} stacked titleOf={inputTitle} />

              {/* 缓存命中率：纵轴固定 0–100% */}
              <div className="flex items-center justify-between text-[10px] mt-3 mb-1" style={{ color: 'var(--text-tertiary)' }}>
                <span>每日缓存命中率（0–100%）</span>
                <span>窗口内 {fmtRate(t?.cacheHitRate ?? 0)}</span>
              </div>
              <TrendColumns buckets={trendBuckets} max={100} height={56} titleOf={rateTitle} />

              {/* 横轴两端：给时间轴一个落点（中间刻度靠悬停看具体日期） */}
              <div className="flex items-center justify-between text-[10px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
                <span>{trendBuckets[0].key}</span>
                <span>{trendBuckets[trendBuckets.length - 1].key}</span>
              </div>

              {/* 上图三桶的图例（同一主色的深浅阶梯，颜色本身不带语义，含义靠这里给） */}
              <div className="flex items-center flex-wrap gap-x-3 gap-y-1 mt-2">
                {[
                  { label: '缓存命中读取', color: SEG_READ },
                  { label: '未命中输入', color: SEG_PROMPT },
                  { label: '缓存写入', color: SEG_WRITE },
                ].map((s) => (
                  <span key={s.label} className="flex items-center gap-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    <span className="shrink-0" style={{ width: 8, height: 8, borderRadius: 2, backgroundColor: s.color }} />
                    {s.label}
                  </span>
                ))}
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  输入 = 三桶之和（不含输出）
                </span>
              </div>
            </Block>
          )}
        </>
      )}
    </SettingsSection>
  );
}
