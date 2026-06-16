import { useEffect, useState } from 'react';
import { useThemeStore } from '../../stores/themeStore';

const PRESET_COLORS = [
  '#3B82F6', // Blue (default)
  '#2563EB', // Blue dark
  '#8B5CF6', // Purple
  '#EC4899', // Pink
  '#EF4444', // Red
  '#F59E0B', // Amber
  '#10B981', // Emerald
  '#14B8A6', // Teal
  '#06B6D4', // Cyan
  '#6366F1', // Indigo
];

export function ThemeCustomizer() {
  const { colors, loadColors, setAccentColor, resetColors } = useThemeStore();
  const [customColor, setCustomColor] = useState(colors.accent);
  const [showPicker, setShowPicker] = useState(false);

  useEffect(() => {
    loadColors();
  }, [loadColors]);

  useEffect(() => {
    setCustomColor(colors.accent);
  }, [colors.accent]);

  const handlePresetClick = (color: string) => {
    setAccentColor(color);
  };

  const handleCustomColorChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const color = e.target.value;
    setCustomColor(color);
    setAccentColor(color);
  };

  return (
    <div className="p-4">
      <h3 className="text-sm font-medium mb-3" style={{ color: 'var(--text-primary)' }}>
        主题色
      </h3>

      {/* Preset colors */}
      <div className="flex flex-wrap gap-2 mb-3">
        {PRESET_COLORS.map((color) => (
          <button
            key={color}
            onClick={() => handlePresetClick(color)}
            className="w-7 h-7 rounded-full transition-transform hover:scale-110 active:scale-95"
            style={{
              backgroundColor: color,
              outline: colors.accent === color ? '2px solid var(--text-primary)' : 'none',
              outlineOffset: '2px',
            }}
            title={color}
          />
        ))}
      </div>

      {/* Custom color picker */}
      <div className="flex items-center gap-2">
        <button
          onClick={() => setShowPicker(!showPicker)}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs transition-all"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
          }}
        >
          <div
            className="w-4 h-4 rounded"
            style={{ backgroundColor: colors.accent }}
          />
          自定义颜色
        </button>
        <button
          onClick={resetColors}
          className="px-3 py-1.5 rounded-lg text-xs transition-all"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-tertiary)',
          }}
        >
          重置
        </button>
      </div>

      {showPicker && (
        <div className="mt-3">
          <input
            type="color"
            value={customColor}
            onChange={handleCustomColorChange}
            className="w-full h-10 rounded-lg cursor-pointer"
            style={{ border: '1px solid var(--border)' }}
          />
        </div>
      )}
    </div>
  );
}
