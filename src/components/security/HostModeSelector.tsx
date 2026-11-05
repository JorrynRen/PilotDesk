import { useState, useEffect, useRef } from 'react';
import { ChevronDown } from 'lucide-react';
import type { AcpModeOption } from '../../stores/acpModeStore';

/**
 * ACP 宿主会话模式选择器（取代 ACP 会话下的「安全模式」）。
 *
 * 模式列表**原样来自宿主**（`session/new|resume|load` 的 `modes`），平台不做映射，
 * 因此选项名称/描述是适配器提供的原文（通常是英文）；不同适配器档位数量也不同。
 *
 * 语义：真正的权限由宿主按所选模式执行；本应用只在宿主**不提供模式**时，
 * 才退回自己的安全模式（兜底审批策略）。
 *
 * 刻意**不**按 id/name 关键词猜"哪一档是完全放开"：协议没有这个语义位，
 * 关键词只能覆盖已知适配器，反而会让用户把"没标红"误读成"安全"。
 * 判断依据只交给宿主自己写的 name / description。
 */
interface HostModeSelectorProps {
  modes: AcpModeOption[];
  /** 用户固定值（`null` = 跟随宿主，不调用 `session/set_mode`） */
  pinned: string | null;
  /** 宿主当前值（"跟随宿主"时展示） */
  currentModeId: string | null;
  /** false = 还没从宿主拿到列表（会话尚未建立过一轮） */
  known: boolean;
  dropUp?: boolean;
  onChange: (modeId: string | null) => void;
}

export function HostModeSelector({
  modes,
  pinned,
  currentModeId,
  known,
  dropUp = true,
  onChange,
}: HostModeSelectorProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [open]);

  const pinnedMode = pinned ? modes.find((m) => m.id === pinned) : undefined;
  const currentMode = currentModeId ? modes.find((m) => m.id === currentModeId) : undefined;
  const disabled = !known || modes.length === 0;

  const label = !known
    ? '宿主模式'
    : pinnedMode
      ? pinnedMode.name || pinnedMode.id
      : currentMode
        ? `跟随宿主 · ${currentMode.name || currentMode.id}`
        : '跟随宿主';

  const choose = (mode: AcpModeOption | null) => {
    setOpen(false);
    onChange(mode ? mode.id : null);
  };

  return (
    <div ref={ref} className="relative shrink-0">
      <button
        onClick={() => {
          if (!disabled) setOpen((o) => !o);
        }}
        disabled={disabled}
        className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs outline-none transition-colors disabled:opacity-60"
        style={{
          backgroundColor: 'var(--bg-field)',
          color: 'var(--text-secondary)',
          border: '1px solid var(--border)',
          height: '24px',
        }}
        title={
          disabled
            ? '该宿主未提供会话模式，或尚未建立会话'
            : '权限档位由宿主提供；切换后从下一条消息开始生效'
        }
      >
        <span className="truncate" style={{ maxWidth: 148 }}>
          {label}
        </span>
        <ChevronDown size={10} style={{ flexShrink: 0 }} />
      </button>

      {open && (
        <div
          className={`absolute left-0 ${dropUp ? 'bottom-full mb-1' : 'top-full mt-1'} z-50 overflow-y-auto`}
          style={{
            padding: 4,
            borderRadius: 'var(--radius-lg)',
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            boxShadow: '0 10px 28px rgba(0,0,0,0.22)',
            minWidth: 288,
            maxHeight: 280,
          }}
        >
          {/* 跟随宿主：不写回模式，完全沿用宿主侧当前档位 */}
          <button
            onClick={() => choose(null)}
            className="w-full text-left flex flex-col gap-0.5 outline-none transition-colors hover:bg-[var(--bg-tertiary)]"
            style={{
              padding: '6px 8px',
              borderRadius: 7,
              backgroundColor: pinned ? 'transparent' : 'var(--bg-tertiary)',
              width: '100%',
              textAlign: 'left',
              alignItems: 'stretch',
            }}
          >
            <span className="flex items-center gap-1.5">
              <span className="text-xs font-semibold" style={{ color: 'var(--text-primary)' }}>
                跟随宿主
              </span>
              {!pinned && <span style={{ color: 'var(--accent)' }}> ✓</span>}
            </span>
            <span
              className="text-[10px]"
              style={{ color: 'var(--text-tertiary)', textAlign: 'left', paddingLeft: 0 }}
            >
              {currentMode ? `当前：${currentMode.name || currentMode.id}` : '不写回模式，沿用宿主当前档位'}
            </span>
          </button>

          {modes.map((m) => {
            const active = m.id === pinned;
            return (
              <button
                key={m.id}
                onClick={() => choose(m)}
                className="w-full text-left flex flex-col gap-0.5 outline-none transition-colors hover:bg-[var(--bg-tertiary)]"
                style={{
                  padding: '6px 8px',
                  borderRadius: 7,
                  backgroundColor: active ? 'var(--bg-tertiary)' : 'transparent',
                  width: '100%',
                  textAlign: 'left',
                  alignItems: 'stretch',
                }}
              >
                <span className="flex items-center gap-1.5">
                  <span
                    className="text-xs font-semibold"
                    style={{ color: 'var(--text-primary)' }}
                  >
                    {m.name || m.id}
                  </span>
                  {active && <span style={{ color: 'var(--accent)' }}> ✓</span>}
                </span>
                {m.description && (
                  <span
                    className="text-[10px]"
                    style={{
                      color: 'var(--text-tertiary)',
                      textAlign: 'left',
                      whiteSpace: 'normal',
                    }}
                  >
                    {m.description}
                  </span>
                )}
              </button>
            );
          })}

          <div
            className="px-2 pt-2 pb-1 text-[10px]"
            style={{ color: 'var(--text-tertiary)', borderTop: '1px solid var(--border)' }}
          >
            模式由宿主提供 · 切换后从下一条消息生效
          </div>
        </div>
      )}
    </div>
  );
}
