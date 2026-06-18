import type { ReactNode } from 'react';

interface SettingsSectionProps {
  title: string;
  description?: string;
  children: ReactNode;
}

export function SettingsSection({ title, description, children }: SettingsSectionProps) {
  return (
    <section>
      <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>
        {title}
      </h3>
      {description && (
        <p className="text-xs mb-3" style={{ color: 'var(--text-secondary)' }}>
          {description}
        </p>
      )}
      {children}
    </section>
  );
}
