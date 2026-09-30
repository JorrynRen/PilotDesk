import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type ApprovalCardProps =
  | {
      variant?: 'approval';
      callId: string;
      sessionId: string;
      toolName: string;
      /** 工具调用参数（原始 JSON 字符串），可折叠展示 */
      args?: string;
      /** 风险等级描述（后端 RiskLevel::description） */
      risk?: string;
      /** 等待截止时间戳（ms），未决时用于本地倒计时 */
      deadline?: number;
      /** 已决结果：true 批准、false 拒绝；undefined 表示未决 */
      approved?: boolean;
      /** 是否由超时自动裁决 */
      timedOut?: boolean;
    }
  | {
      variant: 'continue';
      sessionId: string;
      /** 已达成的迭代轮数 */
      current: number;
      /** 当前迭代上限 */
      max: number;
      /** 等待截止时间戳（ms），未决时用于本地倒计时 */
      deadline?: number;
      /** 已决结果：continue 继续、stop 终止；undefined 表示未决 */
      decision?: 'continue' | 'stop';
      /** 是否由超时自动裁决 */
      timedOut?: boolean;
    };

/**
 * 内联决策卡：替换原 portal 弹窗，随思维链条一起展示与持久化。
 * 审批请求与迭代上限确认共用同一视觉外壳和倒计时逻辑，以 `variant` 区分。
 *
 * - `approval`：未决时本地每秒倒计时；点击「批准 / 拒绝」调用 `agent_approve_tool`，
 *   后端返回是否命中待决请求（false = 已超时/已决/不存在，卡片置为失效），命中则等待
 *   后端 `agent-approval-resolved` 事件翻转状态。历史回看时按钮同样可用。
 * - `continue`：未决时本地每秒倒计时；点击「终止 / 继续执行」调用 `agent_continue_loop`。
 *   该命令无返回值（无法判定是否命中待决请求）：点击后本地乐观置为已决；若后端仍有待决
 *   请求，随后的 `agent-iteration-limit-resolved` 事件会以真实裁决覆盖。历史回看时按钮同样可用。
 */
export function ApprovalCard(props: ApprovalCardProps) {
  const { sessionId, deadline, timedOut } = props;
  const isContinue = props.variant === 'continue';

  // continue 分支点击后的本地乐观裁决（等待事件覆盖）；approval 分支不使用。
  const [localDecision, setLocalDecision] = useState<'continue' | 'stop' | undefined>(undefined);
  const decision = isContinue ? (props.decision ?? localDecision) : undefined;
  const approved = isContinue ? undefined : props.approved;
  const resolved = isContinue ? decision !== undefined : approved !== undefined;

  const [submitting, setSubmitting] = useState(false);
  const [stale, setStale] = useState(false);
  const [showArgs, setShowArgs] = useState(false);
  const [remaining, setRemaining] = useState(() =>
    deadline ? Math.max(0, Math.ceil((deadline - Date.now()) / 1000)) : 0,
  );

  useEffect(() => {
    if (resolved || !deadline) return;
    const update = () => setRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    update();
    const timer = setInterval(update, 1000);
    return () => clearInterval(timer);
  }, [resolved, deadline]);

  const respond = async (confirm: boolean) => {
    if (submitting || resolved) return;
    setSubmitting(true);
    try {
      if (props.variant === 'continue') {
        await invoke('agent_continue_loop', { sessionId, shouldContinue: confirm });
        // 后端命令无返回值：本地乐观置决，真实裁决随后由事件覆盖。
        setLocalDecision(confirm ? 'continue' : 'stop');
      } else {
        const found = await invoke<boolean>('agent_approve_tool', { sessionId, callId: props.callId, approved: confirm });
        if (!found) {
          // 后端已无待决请求（超时/已处理）：置为失效，不抛错
          setStale(true);
          setSubmitting(false);
        }
        // 命中则保持禁用，等待 agent-approval-resolved 事件翻转状态
      }
    } catch (err) {
      console.error('[Approval] respond failed:', err);
      setSubmitting(false);
    }
  };

  const statusText = isContinue
    ? decision === 'continue'
      ? (timedOut ? '✅ 已继续执行（超时自动继续）' : '✅ 已继续执行')
      : decision === 'stop'
        ? '🚫 已终止'
        : remaining <= 0
          ? '⏳ 等待超时裁决…'
          : `⏳ ${remaining} 秒后超时`
    : approved !== undefined
      ? approved
        ? (timedOut ? '✅ 已批准（超时自动批准）' : '✅ 已批准')
        : (timedOut ? '🚫 已拒绝（超时自动拒绝）' : '🚫 已拒绝')
      : stale
        ? '⚠ 该审批已失效（已超时或已处理）'
        : submitting
          ? '已提交，等待后端确认…'
          : remaining <= 0
            ? '⏳ 等待超时裁决…'
            : `⏳ ${remaining} 秒后超时`;
  const statusColor = isContinue
    ? decision === 'continue'
      ? 'var(--success, #22c55e)'
      : decision === 'stop'
        ? 'var(--danger, #ef4444)'
        : 'var(--warning, #f59e0b)'
    : approved !== undefined
      ? (approved ? 'var(--success, #22c55e)' : 'var(--danger, #ef4444)')
      : stale
        ? 'var(--text-tertiary)'
        : 'var(--warning, #f59e0b)';
  const interactive = !resolved && !stale && !submitting;

  let hasArgs = false;
  let prettyArgs = '';
  if (props.variant !== 'continue') {
    hasArgs = !!props.args && props.args !== 'null';
    prettyArgs = props.args ?? '';
    if (hasArgs) {
      try {
        prettyArgs = JSON.stringify(JSON.parse(props.args as string), null, 2);
      } catch { /* 非 JSON：回退原文 */ }
    }
  }

  return (
    <div
      className="my-1 rounded-lg overflow-hidden"
      style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
    >
      {/* Header */}
      <div className="px-3 py-2 flex items-center gap-2">
        <span className="shrink-0" style={{ color: 'var(--warning, #f59e0b)' }}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        </span>
        <span className="text-[11px] font-semibold" style={{ color: 'var(--text-primary)' }}>
          {isContinue ? '迭代上限确认' : '工具审批请求'}
        </span>
        {!isContinue && (
          <code className="text-[10px] px-1 rounded" style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)' }}>
            {props.toolName}
          </code>
        )}
        <span className="ml-auto text-[10px] shrink-0" style={{ color: statusColor }}>{statusText}</span>
      </div>

      {/* Body */}
      <div className="px-3 pb-2">
        {isContinue && (
          <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
            当前 {props.current} / {props.max} 轮，是否继续执行？
          </div>
        )}
        {!isContinue && props.risk && (
          <div className="text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>风险等级：{props.risk}</div>
        )}
        {hasArgs && (
          <div>
            <button
              className="text-[10px] cursor-pointer hover:opacity-80"
              style={{ color: 'var(--accent, #7c3aed)' }}
              onClick={() => setShowArgs((v) => !v)}
            >
              {showArgs ? '▾ 收起参数' : '▸ 展开参数'}
            </button>
            {showArgs && (
              <pre
                className="mt-1 px-2 py-1 text-[10px] font-mono rounded overflow-x-auto whitespace-pre-wrap break-all"
                style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', maxHeight: '200px' }}
              >
                {prettyArgs}
              </pre>
            )}
          </div>
        )}
      </div>

      {/* Actions（未决且未失效时可用，历史回看同样可用）*/}
      {interactive && (
        <div className="px-3 pb-2 flex items-center gap-2">
          {isContinue ? (
            <>
              <button
                className="px-2.5 py-1 text-[11px] rounded transition-colors hover:opacity-80"
                style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                onClick={() => respond(false)}
              >
                终止
              </button>
              <button
                className="px-2.5 py-1 text-[11px] rounded transition-colors hover:opacity-80"
                style={{ backgroundColor: 'var(--accent, #7c3aed)', color: '#fff' }}
                onClick={() => respond(true)}
              >
                继续执行
              </button>
            </>
          ) : (
            <>
              <button
                className="px-2.5 py-1 text-[11px] rounded transition-colors hover:opacity-80"
                style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                onClick={() => respond(false)}
              >
                拒绝
              </button>
              <button
                className="px-2.5 py-1 text-[11px] rounded transition-colors hover:opacity-80"
                style={{ backgroundColor: 'var(--accent, #7c3aed)', color: '#fff' }}
                onClick={() => respond(true)}
              >
                批准
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
