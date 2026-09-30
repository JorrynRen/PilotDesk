import { useEffect } from 'react';
import { useConfirmStore } from '../../stores/confirmStore';

/**
 * 全局确认弹窗（App 顶层挂载一次，配合 `confirmDialog()` 使用）。
 *
 * 替代 `window.confirm`（Tauri WebView 下常被禁用/不触发）；样式与工作流页的删除确认一致。
 * 支持 Esc 或点击遮罩取消。
 */
export function ConfirmDialog() {
  const options = useConfirmStore((s) => s.options);
  const settle = useConfirmStore((s) => s.settle);

  useEffect(() => {
    if (!options) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') settle(false);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [options, settle]);

  if (!options) return null;
  const {
    title = '确认操作',
    message,
    confirmText = '确定',
    cancelText = '取消',
    danger = true,
  } = options;

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={() => settle(false)}
    >
      <div
        className="rounded-xl p-5 shadow-xl max-w-sm w-full mx-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
          {title}
        </div>
        {/* whitespace-pre-line：message 里的 \n 按换行渲染（如卸载技能要摊开目录路径）；
            break-words：Windows 长路径不撑破弹窗 */}
        <div className="text-xs mb-4 leading-relaxed whitespace-pre-line break-words" style={{ color: 'var(--text-secondary)' }}>
          {message}
        </div>
        <div className="flex justify-end gap-2">
          <button
            onClick={() => settle(false)}
            className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
            style={{
              border: '1px solid var(--border)',
              background: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
            }}
          >
            {cancelText}
          </button>
          <button
            onClick={() => settle(true)}
            className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
            style={
              danger
                ? { backgroundColor: '#ef4444', color: '#fff' }
                : { backgroundColor: 'var(--accent)', color: '#fff' }
            }
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}
