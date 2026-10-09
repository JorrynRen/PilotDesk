import { useState, useEffect, useRef } from 'react';
import { SECURITY_MODES } from './securityModes';

// ============================================================
// 会话安全模式选择器（会话窗口/群聊窗口共用）
// - 不显示选择器标题；选项显示「模式名称 + 行为说明」
// - 默认标准模式；无限制模式红色警示
// - 运行时随消息/波次传参，不持久化
// - 说明按"审批尺度"口径写：模式越严，越低风险的调用也要确认（命令/路径
//   另按各自的子策略矩阵分级，见 agent_loop.rs 的 command_action/path_action）
// ============================================================

export type SecurityModeValue = 'strict' | 'standard' | 'relaxed' | 'unrestricted';

export function SecurityModeSelector({
  value,
  onChange,
  dropUp = true,
}: {
  value: SecurityModeValue;
  onChange: (v: SecurityModeValue) => void;
  /**
   * 弹出方向：默认向上（输入区在窗口底部时正好）。
   * 会话默认页把输入区搬到了顶部，向上展开会被窗口上边缘截断，那里传 `false` 改为向下展开。
   */
  dropUp?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const current = SECURITY_MODES.find((m) => m.value === value) ?? SECURITY_MODES[1];

  // 点击外部关闭
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [open]);

  return (
    <div ref={ref} className="relative shrink-0">
      {/* 触发器：仅显示当前模式名称（说明在展开时显示）；样式与会话模式按钮统一 */}
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs outline-none transition-colors"
        style={{
          backgroundColor: 'var(--bg-tertiary)',
          color: current.danger ? 'var(--status-danger)' : 'var(--text-secondary)',
          border: `1px solid ${current.danger ? 'var(--status-danger)' : 'var(--border)'}`,
          fontWeight: current.danger ? 600 : 400,
          height: '24px',
        }}
        title={current.desc}
      >
        <current.icon size={11} style={{ flexShrink: 0, color: current.color }} />
        <span>{current.name}</span>
        <svg
          width="10"
          height="10"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          style={{ transform: open ? 'rotate(180deg)' : 'none', transition: 'transform .15s' }}
        >
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>

      {/* 下拉（默认向上、左对齐展开；会话默认页输入区在顶部时向下）：模式名称 + 行为说明。
          宽度取够一行放下最长说明（"风险/未知命令与路径、中高风险工具需确认"），说明不再折行 */}
      {open && (
        <div
          className={`absolute left-0 ${dropUp ? 'bottom-full mb-1' : 'top-full mt-1'} rounded-lg shadow-lg py-1 z-50`}
          style={{ backgroundColor: 'var(--bg-panel)', border: '1px solid var(--border)', minWidth: 276 }}
        >
          {SECURITY_MODES.map((m) => {
            const active = m.value === value;
            return (
              <button
                key={m.value}
                onClick={() => {
                  onChange(m.value);
                  setOpen(false);
                }}
                className="w-full text-left px-3 py-2 flex flex-col gap-0.5 outline-none"
                style={{
                  backgroundColor: active ? 'var(--bg-hover)' : 'transparent',
                  width: '100%',
                  textAlign: 'left',
                  alignItems: 'stretch',
                }}
              >
                <span
                  className="flex items-center gap-1.5"
                  style={{ justifyContent: 'flex-start', textAlign: 'left' }}
                >
                  <m.icon size={13} style={{ color: m.color, flexShrink: 0 }} />
                  <span
                    className="text-xs font-semibold"
                    style={{ color: m.danger ? 'var(--status-danger)' : 'var(--text-primary)' }}
                  >
                    {m.name}
                  </span>
                  {active && <span style={{ color: 'var(--accent)' }}> ✓</span>}
                </span>
                <span
                  className="text-[10px]"
                  style={{ color: 'var(--text-tertiary)', textAlign: 'left', paddingLeft: 19, whiteSpace: 'nowrap' }}
                >
                  {m.desc}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
