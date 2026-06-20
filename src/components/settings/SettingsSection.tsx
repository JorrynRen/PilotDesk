import type { ReactNode } from 'react';

interface SettingsSectionProps {
  title: string;
  description?: string;
  /** 标题行右侧的操作按钮 */
  actions?: ReactNode;
  children: ReactNode;
}

export function SettingsSection({ title, description, actions, children }: SettingsSectionProps) {
  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
          {title}
        </h3>
        {actions && <div className="flex items-center gap-2">{actions}</div>}
      </div>
      {description && (
        <p className="text-xs mb-3" style={{ color: 'var(--text-secondary)' }}>
          {description}
        </p>
      )}
      {children}
    </section>
  );
}
