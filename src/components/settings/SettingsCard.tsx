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
      className={`pd-settings-card ${highlight ? 'pd-settings-card-highlight' : ''} flex min-h-[38px] items-center justify-between gap-3 rounded-md px-3 py-2 transition-colors ${className}`}
      style={style}
    >
      {children}
    </div>
  );
}
