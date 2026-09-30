/**
 * 消息多选条：出现在消息区顶部，把勾选的若干条消息当成**一段对话**沉淀成知识。
 *
 * 会话模式（`MessageList`）与群聊（`GroupChatPage`）共用这一条 —— 两处各自的差别只有
 * "哪些消息可选 / 角色标签怎么取 / 消息形状长什么样"，那些留在调用点。
 * 状态机与剪贴板工具在 `messageSelection.ts`（组件文件只导出组件，保证 Fast Refresh 可用）。
 */

import { Copy } from 'lucide-react';

export function MessageSelectionBar({
  count,
  hint,
  onCopy,
  onDigest,
  onClear,
}: {
  count: number;
  /** 一句话说明这批会被怎么用（两处口径一致，只是措辞可微调） */
  hint: string;
  onCopy: () => void;
  onDigest: () => void;
  onClear: () => void;
}) {
  const disabled = count === 0;
  const plainBtn = {
    color: 'var(--text-secondary)',
    backgroundColor: 'var(--bg-secondary)',
    border: '1px solid var(--border)',
    opacity: disabled ? 0.5 : 1,
  } as const;

  return (
    <div
      className="shrink-0 px-4 py-1.5 flex items-center gap-2 text-[11px]"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-tertiary)' }}
    >
      <span style={{ color: 'var(--text-primary)' }}>已选 {count} 条</span>
      <span className="text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }}>{hint}</span>
      <div className="flex-1" />
      <button
        onClick={onCopy}
        disabled={disabled}
        className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg"
        style={plainBtn}
        title="按「角色：正文」复制选中的消息"
      >
        <Copy size={11} />
        复制
      </button>
      <button
        onClick={onDigest}
        disabled={disabled}
        className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg"
        style={{
          backgroundColor: 'var(--accent)',
          color: '#fff',
          border: 'none',
          opacity: disabled ? 0.5 : 1,
          cursor: disabled ? 'not-allowed' : 'pointer',
        }}
        title="把选中的消息交给 AI 整理成知识（进「待确认」队列，核对后采纳）"
      >
        沉淀为知识
      </button>
      <button onClick={onClear} disabled={disabled} className="pd-btn px-2 py-1 rounded-lg" style={plainBtn}>
        清空
      </button>
    </div>
  );
}
