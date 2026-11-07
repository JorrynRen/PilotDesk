import { ArrowDown } from 'lucide-react';

interface Props {
  /** 由滚动容器（Virtuoso 的 atBottomStateChange）维护：false = 已离开底部。 */
  visible: boolean;
  onClick: () => void;
}

/**
 * 消息窗口的「直达底部」浮动按钮。
 *
 * 消息较多、或用户向上滚动浏览历史时，虚拟列表不再跟随新消息；
 * 此时在右下角浮出一个 32px 圆形按钮，点击平滑回到最后一条消息。
 * 宿主容器需是定位上下文（position: relative），按钮不参与布局。
 *
 * 隐藏时只淡出 + 禁用指针，不卸载：滚动到底/离底切换时不会反复重挂载。
 */
export function ScrollToBottomButton({ visible, onClick }: Props) {
  return (
    <button
      type="button"
      onClick={onClick}
      title="回到最新消息"
      aria-label="回到最新消息"
      aria-hidden={!visible}
      tabIndex={visible ? 0 : -1}
      className="pd-btn absolute bottom-4 right-4 z-20 w-8 h-8 inline-flex items-center justify-center rounded-full bg-[var(--bg-secondary)] text-[var(--text-secondary)] shadow-[var(--shadow-md)] hover:text-[var(--accent)] transition-colors"
      style={{
        // 边框走内联：全局 button 重置里有未分层的 `border: none`，Tailwind 的 border 类盖不住它
        border: '1px solid var(--border)',
        opacity: visible ? 1 : 0,
        transform: visible ? undefined : 'translateY(6px)',
        pointerEvents: visible ? 'auto' : 'none',
        transition: 'opacity 160ms ease, transform 160ms ease, color 120ms ease',
      }}
    >
      <ArrowDown size={16} />
    </button>
  );
}
