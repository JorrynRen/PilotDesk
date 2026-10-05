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

          {summary.trend.length > 0 && (
            <Block title="趋势" hint="按天缓存命中率（用来看优化有没有生效）">
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
            </Block>
          )}
        </>
      )}
    </SettingsSection>
  );
}
