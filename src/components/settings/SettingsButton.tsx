import type { ReactNode } from 'react';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'success' | 'warning' | 'danger';

interface SettingsButtonProps {
  children: ReactNode;
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
    backgroundColor: 'var(--bg-secondary)',
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
