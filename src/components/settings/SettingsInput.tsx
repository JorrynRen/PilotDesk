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
  if (multiline) {
    return (
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        rows={rows ?? 3}
        className={`pd-settings-input w-full resize-y font-sans ${className}`}
      />
    );
  }

  return (
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className={`pd-settings-input flex-1 ${className}`}
    />
  );
}
