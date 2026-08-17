// 共享确认块组件：会话模式（ask_user 工具）与群聊模式（confirmation_request 消息）共用。
// 纯展示 + 本地交互状态；提交时输出 responses 数组（itemId/value），持久化策略由调用方决定。

import { useState } from 'react';
import { HelpCircle } from 'lucide-react';
import type {
  GroupChatConfirmationRequest,
  GroupChatConfirmationResponseInput,
} from '../../types/groupchat';
import { showToast } from '../../utils/toast';

/** 确认块标题栏兜底：后端未生成 title 时，取确认问题首行截断到 22 字（无省略号）。 */
function confirmationTitle(prompt: string): string {
  const line = (prompt || '').split('\n')[0].trim();
  if (!line) return '确认请求';
  return [...line].slice(0, 22).join('');
}

/** 解析确认请求消息/事件的 JSON（items/options 补齐安全数组）。 */
export function parseConfirmation(raw: string | undefined | null): GroupChatConfirmationRequest | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (!v || typeof v !== 'object' || typeof v.replyMode !== 'string') return null;
    if (!Array.isArray(v.items)) v.items = [];
    for (const it of v.items) {
      if (it && !Array.isArray(it.options)) it.options = [];
    }
    return v as GroupChatConfirmationRequest;
  } catch {
    return null;
  }
}

interface ConfirmationCardProps {
  confirmation: GroupChatConfirmationRequest;
  /** 数据驱动的「已提交」态（群聊跨会话恢复用；会话模式无需传）。 */
  responded?: boolean;
  /** 提交回调：返回被勾选确认项的用户回复数组。 */
  onSubmit: (responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
  /** 提交后的提示文案（默认「已提交，等待继续…」）。 */
  submittedText?: string;
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
  openHint = '需要你的决定，请直接回复。',
  timeout = false,
  countdown,
}: ConfirmationCardProps) {
  // 每项一个勾选开关：默认全部勾选，用户可取消勾选以跳过不回复的项。
  const [checked, setChecked] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(confirmation.items.map((it) => [it.id, true])),
  );
  const [values, setValues] = useState<Record<string, string>>({});
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
      .filter((it) => it.required && !(values[it.id] ?? '').trim())
      .map((it) => it.label);
    if (missing.length > 0) {
      showToast(`请完成必填项：${missing.join('、')}`, 'info');
      return;
    }
    // 只提交被勾选的项，未勾选项不参与回复。
    const responses: GroupChatConfirmationResponseInput[] = active.map((it) => ({
      itemId: it.id,
      value: values[it.id] ?? '',
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
          <span className="text-[11px] font-medium truncate" title={confirmation.prompt || '确认请求'}>{title}</span>
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
        <span className="text-[11px] font-medium truncate shrink-0" title={confirmation.prompt || '确认请求'}>{title}</span>
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
              <select
                value={values[it.id] ?? ''}
                onChange={(e) => setValue(it.id, e.target.value)}
                disabled={locked}
                className="px-3 py-2 rounded-lg text-xs outline-none"
                style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              >
                <option value="">请选择</option>
                {options.map((opt) => (
                  <option key={opt} value={opt}>{opt}</option>
                ))}
              </select>
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

/** 把确认回复格式化为可读文本（label：value，仅含被勾选项），供会话模式回传模型。 */
export function formatResponsesToText(
  items: GroupChatConfirmationRequest['items'],
  responses: GroupChatConfirmationResponseInput[],
): string {
  return responses
    .map((r) => {
      const label = items.find((it) => it.id === r.itemId)?.label?.trim();
      return `${label || r.itemId}：${r.value}`;
    })
    .join('\n');
}
