import { useEffect, useId, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

interface ConversionDialogShellProps {
  icon: ReactNode;
  kicker: string;
  title: string;
  description: ReactNode;
  size?: 'room' | 'compact' | 'preview';
  busy?: boolean;
  onClose: () => void;
  headerAction?: ReactNode;
  footer?: ReactNode;
  bodyClassName?: string;
  children: ReactNode;
}

/**
 * Shared frame for conversion flows. The fixed header/footer keep the primary
 * action visible while the content area owns scrolling and responsive layout.
 */
export function ConversionDialogShell({
  icon,
  kicker,
  title,
  description,
  size = 'room',
  busy = false,
  onClose,
  headerAction,
  footer,
  bodyClassName,
  children,
}: ConversionDialogShellProps) {
  const titleId = useId();

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) onClose();
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [busy, onClose]);

  return createPortal(
    <div
      className="pd-conversion-backdrop"
      onMouseDown={() => {
        if (!busy) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={`pd-conversion-dialog is-${size}`}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="pd-conversion-head">
          <span className="pd-conversion-head-icon" aria-hidden="true">{icon}</span>
          <div className="pd-conversion-head-copy">
            <span className="pd-conversion-kicker">{kicker}</span>
            <h2 id={titleId}>{title}</h2>
            <p>{description}</p>
          </div>
          <div className="pd-conversion-head-actions">
            {headerAction}
            <button
              type="button"
              className="pd-conversion-close"
              onClick={onClose}
              disabled={busy}
              aria-label="关闭"
              title="关闭"
            >
              <X size={15} />
            </button>
          </div>
        </header>

        <div className={`pd-conversion-body pd-scroll-stable${bodyClassName ? ` ${bodyClassName}` : ''}`}>
          {children}
        </div>

        {footer && <footer className="pd-conversion-footer">{footer}</footer>}
      </div>
    </div>,
    document.body,
  );
}
