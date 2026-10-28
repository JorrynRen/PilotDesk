import type { ReactNode } from 'react';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'success' | 'warning' | 'danger' | 'update';

interface SettingsButtonProps {
  children?: ReactNode;
  onClick?: () => void;
  variant?: ButtonVariant;
  disabled?: boolean;
  className?: string;
  title?: string;
  icon?: ReactNode;
}

const VARIANT_STYLES: Record<ButtonVariant, React.CSSProperties> = {
  primary: {
    backgroundColor: 'var(--accent)',
    color: '#fff',
    border: 'none',
  },
  secondary: {
    backgroundColor: 'var(--bg-field)',
    color: 'var(--text-secondary)',
    border: '1px solid var(--border)',
  },
  ghost: {
    backgroundColor: 'transparent',
    color: 'var(--text-secondary)',
    border: 'none',
  },
  success: {
    backgroundColor: '#10B981',
    color: '#fff',
    border: 'none',
  },
  warning: {
    backgroundColor: '#F59E0B',
    color: '#fff',
    border: 'none',
  },
  // 与「可更新」徽标同一套 warning 色调的浅底版本：既能和主色「获取」区分，
  // 又避免白字压琥珀底（12px 下对比度不足）。
  update: {
    backgroundColor: 'var(--status-warning-bg)',
    color: 'var(--status-warning)',
    border: '1px solid var(--status-warning)',
  },
  danger: {
    backgroundColor: '#EF4444',
    color: '#fff',
    border: 'none',
  },
};

export function SettingsButton({
  children,
  onClick,
  variant = 'secondary',
  disabled,
  className = '',
  title,
  icon,
}: SettingsButtonProps) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`pd-btn px-2.5 py-1.5 rounded-lg text-xs ${className}`}
      style={VARIANT_STYLES[variant]}
    >
      {icon}
      {children}
    </button>
  );
}
