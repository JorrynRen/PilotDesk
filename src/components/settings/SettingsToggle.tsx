interface SettingsToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  title?: string;
  className?: string;
  'aria-label'?: string;
}

/** 设置页统一开关：尺寸、轨道色、滑块位移与禁用态共用一套样式。 */
export function SettingsToggle({
  checked,
  onChange,
  disabled,
  title,
  className = '',
  'aria-label': ariaLabel,
}: SettingsToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`pd-settings-toggle ${className}`}
      data-checked={checked}
    >
      <span className="pd-settings-toggle-knob" aria-hidden="true" />
    </button>
  );
}
