import type { ReactNode } from 'react';
import {
  ArrowRightLeft,
  CheckCircle2,
  ClipboardCheck,
  Flag,
  Gauge,
  ListChecks,
  Megaphone,
  ShieldCheck,
  SkipForward,
  Target,
  Trash2,
  UserCog,
  Users,
  XCircle,
} from 'lucide-react';
import { MarkdownRenderer } from '../message/MarkdownRenderer';
import type { GroupChatStageExtra } from '../../types/groupchat';

type MentionMap = ReadonlyMap<string, string>;

/** 阶段名称 → 图标 + 标题（+ 强调色）。所有圆桌阶段消息共用这套映射。 */
const STAGE_META: Record<string, { title: string; icon: typeof Target; tone?: string }> = {
  goal: { title: '目标理解', icon: Target },
  schedule_tasks: { title: '调度-任务', icon: ListChecks },
  schedule_reconcile: { title: '调度-对账', icon: ClipboardCheck },
  schedule_roster: { title: '调度-名册', icon: Users },
  schedule_roles: { title: '调度-角色分配', icon: UserCog },
  schedule_speaker: { title: '调度-选人', icon: Megaphone },
  schedule: { title: '调度', icon: ClipboardCheck },
  tool_decision: { title: '裁决-工具授权', icon: ShieldCheck },
  progress: { title: '进度-盘点', icon: Gauge },
  task_result: { title: '任务-完成', icon: CheckCircle2, tone: 'var(--status-success)' },
};

/** 任务终态 → 标题 + 图标 + 强调色（完成/失败/跳过/中止统一在此收敛）。 */
const TASK_RESULT_META: Record<string, { title: string; icon: typeof Target; tone?: string }> = {
  success: { title: '任务-完成', icon: CheckCircle2, tone: 'var(--status-success)' },
  failed: { title: '任务-失败', icon: XCircle, tone: 'var(--status-danger)' },
  skipped: { title: '任务-跳过', icon: SkipForward, tone: 'var(--text-tertiary)' },
  aborted: { title: '任务-中止', icon: XCircle, tone: 'var(--text-tertiary)' },
};

const STAGE_PREFIX: Record<string, { stage: string; re: RegExp }> = {
  goal: { stage: 'goal', re: /^\s*【目标理解】\s*/ },
  reconcile: { stage: 'schedule_reconcile', re: /^\s*【调度】\s*对账\s*/ },
  roster: { stage: 'schedule_roster', re: /^\s*【调度】\s*(?:名册|已新增参与者)\s*/ },
  speaker: { stage: 'schedule_speaker', re: /^\s*【调度】\s*选择\s*/ },
  scheduling: { stage: 'schedule', re: /^\s*【调度】\s*/ },
  progress: { stage: 'progress', re: /^\s*【进度】\s*[：:]?\s*/ },
  task_result: { stage: 'task_result', re: /^\s*\[任务(完成|失败|跳过)?\][：:\s]*/ },
};

/** 旧版工具裁决消息是固定格式的 bullet，解析为表格行以便历史消息也表格化。 */
function parseToolDecisionLines(text: string): GroupChatStageExtra['items'] {
  const items: NonNullable<GroupChatStageExtra['items']> = [];
  for (const line of text.split('\n')) {
    const m = /^- (.+?)（(.+?)）→ (放行|拒绝)(?:（(.+)）)?\s*$/.exec(line.trim());
    if (m) {
      items.push({ tool: m[1], risk: m[2], allow: m[3] === '放行', reason: m[4] ?? '' });
    }
  }
  return items;
}

/** 解析消息 extra 中的阶段载荷（无 stage 字段时返回 null，走原文渲染）。 */
export function parseStageExtra(extra: string): GroupChatStageExtra | null {
  if (!extra || !extra.trim().startsWith('{')) return null;
  try {
    const parsed = JSON.parse(extra) as GroupChatStageExtra;
    return parsed && typeof parsed.stage === 'string' ? parsed : null;
  } catch {
    return null;
  }
}

/**
 * 兼容旧消息/未结构化消息：按正文前缀推导阶段标题，正文仍按 Markdown 渲染，
 * 保证历史消息与新消息视觉统一（不依赖重新落库）。
 */
export function deriveStage(content: string, kind: string): GroupChatStageExtra | null {
  if (kind === 'summary') {
    const m = STAGE_PREFIX.progress.re.exec(content);
    if (m) return { stage: 'progress', body: content.slice(m[0].length) };
  }
  if (!content) return null;
  const tool = /^\s*本轮工具裁决[^：:\n]*[：:]\s*/.exec(content);
  if (tool) {
    const items = parseToolDecisionLines(content.slice(tool[0].length)) ?? [];
    // 解析成功则走表格；格式异常时保留原文渲染，避免吞掉信息。
    return items.length > 0
      ? { stage: 'tool_decision', items }
      : { stage: 'tool_decision', body: content.slice(tool[0].length) };
  }
  for (const key of ['goal', 'reconcile', 'roster', 'speaker', 'scheduling', 'progress']) {
    const m = STAGE_PREFIX[key].re.exec(content);
    if (m) return { stage: STAGE_PREFIX[key].stage, body: content.slice(m[0].length) };
  }
  if (kind === 'task_result') {
    const m = STAGE_PREFIX.task_result.re.exec(content);
    const status =
      m?.[1] === '失败' ? 'failed' : m?.[1] === '跳过' ? 'skipped' : m?.[1] === '完成' ? 'success' : 'aborted';
    return { stage: 'task_result', status, body: m ? content.slice(m[0].length) : content };
  }
  return null;
}

/** 立场声明前缀：`【立场：支持】` / `立场：支持`，解析后正文不再重复展示该行。 */
export function parseStance(content: string): { attitude: string; body: string } | null {
  const m = /^\s*【?\s*立场\s*[:：]\s*(支持|反对|中立)\s*】?\s*/.exec(content);
  if (!m) return null;
  return { attitude: m[1], body: content.slice(m[0].length) };
}

const STANCE_TONE: Record<string, string> = {
  支持: 'var(--status-success)',
  反对: 'var(--status-danger)',
  中立: 'var(--text-tertiary)',
};

/** 立场徽标：气泡顶部的小 chip，替代独立大标题，避免高频消息的视觉噪音。 */
export function StanceBadge({ attitude }: { attitude: string }) {
  const tone = STANCE_TONE[attitude] ?? 'var(--text-tertiary)';
  return (
    <div className="flex mb-1">
      <span
        className="inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[10px] font-medium"
        style={{ color: tone, backgroundColor: 'var(--bg-tertiary)', border: `1px solid ${tone}` }}
      >
        <Flag size={10} style={{ flexShrink: 0 }} />
        立场：{attitude}
      </span>
    </div>
  );
}

function mentionName(id: string | null | undefined, mentionMap: MentionMap): string {
  if (!id) return '—';
  return `@${mentionMap.get(id) || id}`;
}

function StageTable({
  head,
  rows,
}: {
  head: string[];
  rows: ReactNode[][];
}) {
  if (rows.length === 0) return null;
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-[11px]">
        <thead>
          <tr>
            {head.map((h) => (
              <th
                key={h}
                className="text-left font-medium px-1.5 py-1 whitespace-nowrap"
                style={{ color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border)' }}
              >
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, ri) => (
            <tr key={ri}>
              {row.map((cell, ci) => (
                <td
                  key={ci}
                  className="px-1.5 py-1 align-top"
                  style={{ color: 'var(--text-secondary)', borderBottom: '1px solid var(--border)' }}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Section({ label, children }: { label?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      {label && (
        <div className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
          {label}
        </div>
      )}
      {children}
    </div>
  );
}

function TaskCell({ text }: { text: string }) {
  return (
    <span className="block max-w-[260px] truncate" title={text}>
      {text}
    </span>
  );
}

/** 风险等级 → 颜色（兼容中文「低/中/高」与英文 low/medium/high 两种写法）。 */
function riskTone(risk?: string): string {
  const r = risk ?? '';
  if (r.includes('高') || /high/i.test(r)) return 'var(--status-danger)';
  if (r.includes('中') || /medium|mid/i.test(r)) return 'var(--status-warning)';
  return 'var(--text-tertiary)';
}

/**
 * 风险等级 → 极简标签（低/中/高）。后端 risk 是完整描述句（如
 * 「中风险操作（可能修改文件或访问网络）」），整句放进 nowrap 列会把表格撑出横向滚动，
 * 故只展示等级，完整描述挂在 title 上。
 */
function riskLabel(risk?: string): string {
  const r = risk ?? '';
  if (r.includes('高') || /high/i.test(r)) return '高';
  if (r.includes('中') || /medium|mid/i.test(r)) return '中';
  if (r.includes('低') || /low/i.test(r)) return '低';
  return r;
}

/**
 * 圆桌阶段消息统一卡片：`图标 + 标题 + 分隔线 + 内容/表格`。
 * 结构化数据（extra.stage）优先；旧消息按正文前缀降级为纯 Markdown 正文。
 */
export function StageNotice({
  payload,
  mentionMap,
  formatText = (t: string) => t,
}: {
  payload: GroupChatStageExtra;
  mentionMap: MentionMap;
  /** 自由文本字段的展示替换（@提及 → 显示名）；表格里的参与者列已单独处理。 */
  formatText?: (text: string) => string;
}) {
  const meta =
    payload.stage === 'task_result'
      ? TASK_RESULT_META[payload.status ?? 'success'] ?? TASK_RESULT_META.success
      : STAGE_META[payload.stage] ?? STAGE_META.schedule;
  const Icon = meta.icon;
  const tone = meta.tone ?? 'var(--accent)';

  const hasStructured =
    Boolean(payload.goal) ||
    Boolean(payload.questions?.length) ||
    Boolean(payload.digest?.length) ||
    Boolean(payload.roster?.length) ||
    Boolean(payload.items?.length) ||
    Boolean(payload.rosterRejected?.length) ||
    Boolean(payload.roles?.length) ||
    Boolean(payload.tasks?.length) ||
    Boolean(payload.removed?.length) ||
    Boolean(payload.reassigned?.length) ||
    Boolean(payload.speaker);

  return (
    <div
      className="rounded-lg overflow-hidden my-1.5"
      style={{ backgroundColor: 'var(--bg-raised)', border: '1px solid var(--border)' }}
    >
      <div
        className="flex items-center gap-1.5 px-2.5 py-1.5"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        <Icon size={13} style={{ color: tone, flexShrink: 0 }} />
        <span className="text-[11px] font-semibold" style={{ color: 'var(--text-primary)' }}>
          {meta.title}
          {payload.stage === 'task_result' && payload.taskNo ? (
            <span style={{ color: 'var(--text-tertiary)', fontWeight: 400 }}> · T{payload.taskNo}</span>
          ) : null}
        </span>
      </div>

      <div className="px-2.5 py-2 flex flex-col gap-2">
        {payload.notice && (
          <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
            {formatText(payload.notice)}
          </div>
        )}

        {payload.goal && (
          <Section label="目标">
            <div className="text-[11px] leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
              {formatText(payload.goal)}
            </div>
          </Section>
        )}

        {payload.digest && payload.digest.length > 0 && (
          <Section label="已确认">
            <ul className="text-[11px] leading-relaxed flex flex-col gap-0.5" style={{ color: 'var(--text-secondary)' }}>
              {payload.digest.map((d, i) => (
                <li key={i}>· {formatText(d)}</li>
              ))}
            </ul>
          </Section>
        )}

        {payload.questions && payload.questions.length > 0 && (
          <Section label="待确认问题">
            <ol className="text-[11px] leading-relaxed flex flex-col gap-0.5" style={{ color: 'var(--text-secondary)' }}>
              {payload.questions.map((q, i) => (
                <li key={i}>
                  {i + 1}. {formatText(q)}
                </li>
              ))}
            </ol>
          </Section>
        )}

        {payload.speaker && (
          <Section label="本轮发言">
            <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
              <span style={{ color: 'var(--accent)' }}>{mentionName(payload.speaker, mentionMap)}</span>
              {payload.speakerReason ? ` · ${formatText(payload.speakerReason)}` : ''}
            </div>
            {payload.workContent && (
              <div className="text-[11px] leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
                {formatText(payload.workContent)}
              </div>
            )}
          </Section>
        )}

        {payload.items && payload.items.length > 0 && (
          <Section label="本轮裁决">
            <StageTable
              head={['工具', '风险', '裁决', '依据']}
              rows={payload.items.map((d) => [
                <span
                  className="font-medium break-all"
                  style={{ color: 'var(--text-primary)' }}
                  title={d.tool}
                >
                  {d.tool}
                </span>,
                <span
                  className="whitespace-nowrap font-medium"
                  style={{ color: riskTone(d.risk) }}
                  title={d.risk || undefined}
                >
                  {riskLabel(d.risk) || '—'}
                </span>,
                d.allow === false ? (
                  <span className="whitespace-nowrap font-medium" style={{ color: 'var(--status-danger)' }}>
                    拒绝
                  </span>
                ) : (
                  <span className="whitespace-nowrap font-medium" style={{ color: 'var(--status-success)' }}>
                    放行
                  </span>
                ),
                <TaskCell text={formatText(d.reason || '—')} />,
              ])}
            />
          </Section>
        )}

        {payload.roster && payload.roster.length > 0 && (
          <Section label="新增参与者">
            <StageTable
              head={['参与者', '角色', '原因']}
              rows={payload.roster.map((r) => [
                <span style={{ color: 'var(--accent)' }}>{mentionName(r.participant, mentionMap)}</span>,
                r.role || '—',
                <TaskCell text={formatText(r.reason || '—')} />,
              ])}
            />
          </Section>
        )}

        {payload.rosterRejected && payload.rosterRejected.length > 0 && (
          <Section label="未通过">
            <ul className="text-[11px] leading-relaxed flex flex-col gap-0.5" style={{ color: 'var(--status-danger)' }}>
              {payload.rosterRejected.map((r, i) => (
                <li key={i}>· {formatText(r)}</li>
              ))}
            </ul>
          </Section>
        )}

        {payload.roles && payload.roles.length > 0 && (
          <Section label="角色分配">
            <StageTable
              head={['参与者', '角色', '结果']}
              rows={payload.roles.map((r) => [
                <span style={{ color: 'var(--accent)' }}>{mentionName(r.participant, mentionMap)}</span>,
                r.role || '—',
                r.status === 'rejected' ? (
                  <span style={{ color: 'var(--status-danger)' }}>未通过{r.reason ? `：${formatText(r.reason)}` : ''}</span>
                ) : r.status === 'unassigned' ? (
                  <span style={{ color: 'var(--text-tertiary)' }}>未分配</span>
                ) : (
                  <span style={{ color: 'var(--status-success)' }}>已分配</span>
                ),
              ])}
            />
          </Section>
        )}

        {payload.tasks && payload.tasks.length > 0 && (
          <Section label="任务">
            <StageTable
              head={['编号', '任务', '负责人', '依赖']}
              rows={payload.tasks.map((t) => [
                <span className="whitespace-nowrap">T{t.no}</span>,
                <TaskCell text={formatText(t.description)} />,
                mentionName(t.assignee, mentionMap),
                t.dependsOn && t.dependsOn.length > 0 ? t.dependsOn.map((n) => `T${n}`).join('、') : '—',
              ])}
            />
          </Section>
        )}

        {payload.removed && payload.removed.length > 0 && (
          <Section label="移除任务">
            <StageTable
              head={['编号', '原因']}
              rows={payload.removed.map((t) => [
                <span className="whitespace-nowrap inline-flex items-center gap-1">
                  <Trash2 size={10} style={{ color: 'var(--text-tertiary)' }} />T{t.no}
                </span>,
                <TaskCell text={formatText(t.reason || '判定冗余')} />,
              ])}
            />
          </Section>
        )}

        {payload.reassigned && payload.reassigned.length > 0 && (
          <Section label="任务分配">
            <StageTable
              head={['编号', '原负责人', '新负责人']}
              rows={payload.reassigned.map((t) => [
                <span className="whitespace-nowrap inline-flex items-center gap-1">
                  <ArrowRightLeft size={10} style={{ color: 'var(--text-tertiary)' }} />T{t.no}
                </span>,
                mentionName(t.from, mentionMap),
                <span style={{ color: 'var(--accent)' }}>{mentionName(t.to, mentionMap)}</span>,
              ])}
            />
          </Section>
        )}

        {payload.stage === 'task_result' && payload.description && (
          <Section label="任务描述">
            <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
              {formatText(payload.description)}
            </div>
          </Section>
        )}

        {payload.stage === 'task_result' && payload.body ? (
          <MarkdownRenderer content={payload.body} />
        ) : !hasStructured && payload.body ? (
          <MarkdownRenderer content={payload.body} />
        ) : null}
      </div>
    </div>
  );
}
