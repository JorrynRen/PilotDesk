// 共享确认块组件：会话模式（ask_user 工具）与群聊模式（confirmation_request 消息）共用。
// 纯展示 + 本地交互状态；提交时输出 responses 数组（itemId/value），持久化策略由调用方决定。

import { useState } from 'react';
import { HelpCircle } from 'lucide-react';
import type {
  GroupChatConfirmationRequest,
  GroupChatConfirmationResponseInput,
} from '../../types/groupchat';
import { showToast } from '../../utils/toast';
import { headChars } from '../../utils/text';
import { Select } from '../common/Select';

/** 确认块标题栏兜底：后端未生成 title 时，取确认问题首行截断到 22 字（无省略号）。 */
function confirmationTitle(prompt: string): string {
  const line = (prompt || '').split('\n')[0].trim();
  if (!line) return '确认请求';
  return headChars(line, 22);
}

/** 判断选项是否属于"其他类"（选中后需补充自由文本，避免再来一轮确认）。 */
function isOtherOption(value: string): boolean {
  return /^(其他|其它|自定义|另[外他]|Other)/i.test((value || '').trim());
}

/** 确认项的有效提交值：选中"其他类"选项且填写了补充文本时，用补充文本作为 value。 */
function effectiveValue(
  item: GroupChatConfirmationRequest['items'][number],
  values: Record<string, string>,
  customValues: Record<string, string>,
): string {
  const v = values[item.id] ?? '';
  if (item.inputType !== 'text' && isOtherOption(v)) {
    const custom = (customValues[item.id] ?? '').trim();
    if (custom) return custom;
  }
  return v;
}

interface ConfirmationCardProps {
  confirmation: GroupChatConfirmationRequest;
  /** 数据驱动的「已提交」态（群聊跨会话恢复用；会话模式无需传）。 */
  responded?: boolean;
  /** 提交回调：返回被勾选确认项的用户回复数组。 */
  onSubmit: (responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
  /** 提交后的提示文案（默认「已提交，等待继续…」）。 */
  submittedText?: string;
  /** 已提交时的结构化回复（confirmation_response 的 extra.responses）：恢复勾选态与输入/选择值到控件。 */
  submittedResponses?: GroupChatConfirmationResponseInput[];
  /** open 模式下的提示文案（默认「需要你的决定，请直接回复。」）。 */
  openHint?: string;
  /** 等待超时（会话模式 60s 未回复时置位，禁用控件）。 */
  timeout?: boolean;
  /** 剩余等待秒数（会话模式倒计时显示；缺省不显示）。 */
  countdown?: number;
}

/** 消息流内嵌确认块的数据（会话模式传给 MessageBubble/MessageList 渲染）。 */
export interface ConfirmationBlockData {
  request: GroupChatConfirmationRequest;
  /** 剩余等待秒数（倒计时显示）。 */
  countdown?: number;
  onSubmit: (responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
}

export function ConfirmationCard({
  confirmation,
  responded = false,
  onSubmit,
  submittedText = '已提交，等待继续…',
  submittedResponses,
  openHint = '需要你的决定，请直接回复。',
  timeout = false,
  countdown,
}: ConfirmationCardProps) {
  // 已提交且存在结构化回复时，还原勾选态（有回复的项勾选）与输入/选择值；否则默认全勾选、空值。
  const [checked, setChecked] = useState<Record<string, boolean>>(() => {
    if (responded && submittedResponses) {
      const replied = new Set(submittedResponses.map((r) => r.itemId));
      return Object.fromEntries(confirmation.items.map((it) => [it.id, replied.has(it.id)]));
    }
    return Object.fromEntries(confirmation.items.map((it) => [it.id, true]));
  });
  const [values, setValues] = useState<Record<string, string>>(() => {
    if (responded && submittedResponses) {
      // 自定义补充还原：提交值不在选项内时，把 select 回填到"其他类"选项，原值进 customValues。
      return Object.fromEntries(
        submittedResponses.map((r) => {
          const item = confirmation.items.find((it) => it.id === r.itemId);
          if (item && item.inputType !== 'text' && r.value && !(item.options ?? []).includes(r.value)) {
            const otherOpt = (item.options ?? []).find((o) => isOtherOption(o));
            return [r.itemId, otherOpt ?? ''];
          }
          return [r.itemId, r.value];
        }),
      );
    }
    return {};
  });
  // 选中"其他类"选项时补充的自由文本（提交时作为该确认项的 value）。
  const [customValues, setCustomValues] = useState<Record<string, string>>(() => {
    if (responded && submittedResponses) {
      return Object.fromEntries(
        submittedResponses
          .map((r) => {
            const item = confirmation.items.find((it) => it.id === r.itemId);
            if (item && item.inputType !== 'text' && r.value && !(item.options ?? []).includes(r.value)) {
              return [r.itemId, r.value];
            }
            return null;
          })
          .filter((e): e is [string, string] => e !== null),
      );
    }
    return {};
  });
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);

  // 数据驱动的已提交态与本地提交态取并集：提交瞬间本地先置位，后端落库后由 responded 兜底。
  const isSubmitted = responded || submitted;

  const isOpen = confirmation.replyMode === 'open' || confirmation.items.length === 0;
  // 标题优先用后端生成的确认事项摘要（≤22 字），缺失时兜底取 prompt 首行。
  const title = confirmation.title?.trim() || confirmationTitle(confirmation.prompt);

  function setValue(id: string, value: string) {
    setValues((v) => ({ ...v, [id]: value }));
  }

  function toggleChecked(id: string) {
    setChecked((c) => ({ ...c, [id]: !c[id] }));
  }

  async function submit() {
    const active = confirmation.items.filter((it) => checked[it.id] !== false);
    if (active.length === 0) {
      showToast('请至少勾选一项确认项', 'info');
      return;
    }
    const missing = active
      .filter((it) => it.required && !effectiveValue(it, values, customValues).trim())
      .map((it) => it.label);
    if (missing.length > 0) {
      showToast(`请完成必填项：${missing.join('、')}`, 'info');
      return;
    }
    // 只提交被勾选的项，未勾选项不参与回复。
    const responses: GroupChatConfirmationResponseInput[] = active.map((it) => ({
      itemId: it.id,
      value: effectiveValue(it, values, customValues),
    }));
    setSubmitting(true);
    try {
      await onSubmit(responses);
      setSubmitted(true);
    } catch {
      // 错误已由调用方处理，此处保持可重试状态。
    } finally {
      setSubmitting(false);
    }
  }

  const locked = isSubmitted || timeout;

  if (isOpen) {
    return (
      <div className="mt-2 rounded-lg overflow-hidden" style={{ border: '1px solid var(--accent)' }}>
        <div className="flex items-center gap-1.5 px-3 h-8" style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>
          <HelpCircle size={12} className="shrink-0" />
          <span className="text-[11px] font-medium truncate">{title}</span>
          {typeof countdown === 'number' && countdown > 0 && (
            <span className="ml-auto text-[10px] opacity-80 shrink-0">{countdown}s</span>
          )}
        </div>
        <div className="px-3 py-2 text-[11px]" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
          {openHint}
        </div>
      </div>
    );
  }

  return (
    <div className="mt-2 rounded-lg overflow-hidden" style={{ border: '1px solid var(--accent)' }}>
      {/* 确认块标题栏 */}
      <div className="flex items-center gap-1.5 px-3 h-8 shrink-0" style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>
        <HelpCircle size={12} className="shrink-0" />
        <span className="text-[11px] font-medium truncate shrink-0">{title}</span>
        {isSubmitted && (
          <span className="ml-auto text-[10px] opacity-80 shrink-0">已提交</span>
        )}
        {timeout && !isSubmitted && (
          <span className="ml-auto text-[10px] opacity-80 shrink-0">已超时</span>
        )}
        {!isSubmitted && !timeout && typeof countdown === 'number' && countdown > 0 && (
          <span className="ml-auto text-[10px] opacity-80 shrink-0">{countdown}s</span>
        )}
      </div>
      <div className="p-3 flex flex-col gap-2" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
        {confirmation.prompt && (
          <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>{confirmation.prompt}</div>
        )}
      {confirmation.items.map((it) => {
        const isChecked = checked[it.id] !== false;
        // 选项兜底：confirm 固定是/否；select 缺选项（后端空数组会省略字段）时回退是/否，避免用户无法选择。
        const itOptions = it.options ?? [];
        const options = it.inputType === 'confirm' ? ['是', '否'] : (itOptions.length > 0 ? itOptions : ['是', '否']);
        return (
          <div key={it.id} className="flex flex-col gap-1">
            <label className="flex items-center gap-1.5 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={isChecked}
                onChange={() => toggleChecked(it.id)}
                disabled={locked}
                style={{ accentColor: 'var(--accent)' }}
              />
              <span
                className="text-[11px]"
                style={{ color: isChecked ? 'var(--text-primary)' : 'var(--text-tertiary)' }}
              >
                {it.label || '确认项'}
                {it.required && <span style={{ color: 'var(--status-danger)' }}> *</span>}
              </span>
            </label>
            {isChecked && (it.inputType === 'text' ? (
              <input
                type="text"
                value={values[it.id] ?? ''}
                onChange={(e) => setValue(it.id, e.target.value)}
                disabled={locked}
                placeholder={it.placeholder || '请输入'}
                className="px-3 py-2 rounded-lg text-xs outline-none"
                style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              />
            ) : (
              <>
              <Select
                value={values[it.id] ?? ''}
                onChange={(v) => setValue(it.id, v)}
                disabled={locked}
                placeholder="请选择"
                options={[
                  { value: '', label: '请选择' },
                  ...options.map((opt) => ({ value: opt, label: opt })),
                ]}
              />
              {/* 选中"其他类"选项时展开自由文本输入，避免需再经一轮确认补充细节 */}
              {isOtherOption(values[it.id] ?? '') && (
                <input
                  type="text"
                  value={customValues[it.id] ?? ''}
                  onChange={(e) => setCustomValues((v) => ({ ...v, [it.id]: e.target.value }))}
                  disabled={locked}
                  placeholder="请补充具体内容（提交后将作为该项回复）"
                  className="px-3 py-2 rounded-lg text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                />
              )}
              </>
            ))}
          </div>
        );
      })}
      {locked ? (
        <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          {timeout && !isSubmitted ? '等待超时，模型将根据已有信息继续。' : submittedText}
        </div>
      ) : (
        <button
          onClick={submit}
          disabled={submitting}
          className="pd-btn pd-btn-sm pd-btn-primary self-end"
        >
          {submitting ? '提交中…' : '提交确认'}
        </button>
      )}
      </div>
    </div>
  );
}
