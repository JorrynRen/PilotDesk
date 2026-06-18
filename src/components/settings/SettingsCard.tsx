import type { ReactNode } from 'react';

interface SettingsCardProps {
  children: ReactNode;
  className?: string;
  highlight?: boolean;
  style?: React.CSSProperties;
}

export function SettingsCard({ children, className = '', highlight, style }: SettingsCardProps) {
  return (
    <div
      className={`flex items-center justify-between px-3 py-2.5 rounded-lg transition-colors ${className}`}
      style={{
        backgroundColor: highlight ? 'rgba(245, 158, 11, 0.06)' : 'var(--bg-tertiary)',
        border: highlight ? '1px solid rgba(245, 158, 11, 0.2)' : '1px solid transparent',
        ...style,
      }}
    >
      {children}
    </div>
  );
}
