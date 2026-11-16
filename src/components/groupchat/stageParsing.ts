import type { GroupChatStageExtra } from '../../types/groupchat';

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
  const report = /^\s*(?:#{1,6}\s*)?(?:T\s*)?(\d{1,2})?\s*执行完成报告\s*\n/.exec(content);
  if (report) {
    return {
      stage: 'task_result',
      status: 'success',
      taskNo: report[1] ? Number(report[1]) : undefined,
      body: content.slice(report[0].length),
    };
  }
  const tool = /^\s*本轮工具裁决[^：:\n]*[：:]\s*/.exec(content);
  if (tool) {
    const items = parseToolDecisionLines(content.slice(tool[0].length)) ?? [];
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

/** 立场声明：`【立场：支持】` / `立场：支持`，解析后正文不再重复展示该行。 */
export function parseStance(content: string): { attitude: string; body: string } | null {
  const m = /【\s*立场\s*[:：]\s*(支持|反对|中立)\s*】|立场\s*[:：]\s*(支持|反对|中立)/.exec(
    content.slice(0, 240),
  );
  if (!m) return null;
  const lineStart = content.lastIndexOf('\n', m.index) + 1;
  const nl = content.indexOf('\n', m.index);
  const lineEnd = nl === -1 ? content.length : nl;
  const rest = content.slice(lineStart, lineEnd).replace(m[0], '').replace(/[>*#\-|。：:\s]/g, '');
  if (rest.length > 8) return null;
  const attitude = m[1] ?? m[2];
  const before = content.slice(0, lineStart).replace(/\s*---\s*$/, '').trim();
  const after = content.slice(lineEnd).replace(/^\s*---\s*/, '').trim();
  return { attitude, body: [before, after].filter(Boolean).join('\n\n') };
}
