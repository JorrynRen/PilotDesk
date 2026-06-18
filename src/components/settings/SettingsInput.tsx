interface SettingsInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
  multiline?: boolean;
  rows?: number;
}

export function SettingsInput({
  value,
  onChange,
  placeholder,
  className = '',
  multiline,
  rows,
}: SettingsInputProps) {
  const baseStyle: React.CSSProperties = {
    backgroundColor: 'var(--bg-secondary)',
    color: 'var(--text-primary)',
    border: '1px solid var(--border)',
    outline: 'none',
  };

  if (multiline) {
    return (
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        rows={rows ?? 3}
        className={`w-full px-3 py-2 rounded-lg text-xs outline-none resize-none ${className}`}
        style={baseStyle}
      />
    );
  }

  return (
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className={`flex-1 px-3 py-2 rounded-lg text-sm outline-none ${className}`}
      style={baseStyle}
    />
  );
}
