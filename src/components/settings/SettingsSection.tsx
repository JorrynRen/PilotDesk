import type { ReactNode } from 'react';

interface SettingsSectionProps {
  title: string;
  description?: ReactNode;
  /** 标题行右侧的操作按钮 */
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
  contentClassName?: string;
  /** 仅渲染标题与操作区；用于导入面板等按需展开、当前没有正文的区块。 */
  bodyless?: boolean;
}

export function SettingsSection({
  title,
  description,
  actions,
  children,
  className = '',
  contentClassName = '',
  bodyless = false,
}: SettingsSectionProps) {
  return (
    <section
      className={`pd-settings-section ${bodyless ? 'pd-settings-section-bodyless' : ''} ${className}`}
    >
      <div className="pd-settings-section-header">
        <div className="pd-settings-section-heading">
          <h3 className="pd-settings-section-title">{title}</h3>
          {description && <div className="pd-settings-section-description">{description}</div>}
        </div>
        {actions && <div className="pd-settings-section-actions">{actions}</div>}
      </div>
      {!bodyless && (
        <div className={`pd-settings-section-body ${contentClassName}`}>{children}</div>
      )}
    </section>
  );
}
