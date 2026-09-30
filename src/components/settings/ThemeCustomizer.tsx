import { useEffect, useRef, useState } from 'react';
import { useThemeStore } from '../../stores/themeStore';
import { SettingsSection, SettingsButton } from './index';

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
  /** 原生取色器的真实身份：按钮点击时代它触发，直接弹出系统色域窗口 */
  const colorInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    loadColors();
  }, [loadColors]);

  // 主题色变化时同步取色器回显。用「渲染期修正」（React adjust-during-render）而不是 effect——
  // 在 effect 里同步 setState 会多一轮级联渲染（`react-hooks/set-state-in-effect`），
  // 而"外部值（colors.accent）变了就把本地状态同步成它"正是该写法的适用场景。
  const [syncedAccent, setSyncedAccent] = useState(colors.accent);
  if (syncedAccent !== colors.accent) {
    setSyncedAccent(colors.accent);
    setCustomColor(colors.accent);
  }

  const handlePresetClick = (color: string) => {
    setAccentColor(color);
  };

  const handleCustomColorChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const color = e.target.value;
    setCustomColor(color);
    setAccentColor(color);
  };

  return (
    <SettingsSection title="主题色">

      {/* 预设色 + 自定义/重置同一行：前者是"快选"、后者是"微调"，同属一次选色动作，
          拆成两行会让下方两个按钮看起来像独立设置项。窄宽度时整行自动折行。 */}
      <div className="flex flex-wrap items-center gap-2">
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

        <span className="relative">
          <SettingsButton onClick={() => colorInputRef.current?.click()} variant="secondary" title="自定义主题色">
            <span className="flex items-center gap-1.5">
              <span className="inline-block w-4 h-4 rounded" style={{ backgroundColor: colors.accent }} />
              自定义颜色
            </span>
          </SettingsButton>
          {/* 原生取色器：按钮已带颜色预览，所以不再在下方展开一条色条，点击直接唤起系统色域弹窗。
              它必须**保持被渲染**（display:none / 未挂载都点不开），故用 1px 透明盒子贴在按钮左下角，
              让弹窗锚在按钮附近；不用 input.showPicker() 是因为它对未渲染元素会抛异常。 */}
          <input
            ref={colorInputRef}
            type="color"
            value={customColor}
            onChange={handleCustomColorChange}
            aria-hidden="true"
            tabIndex={-1}
            className="absolute left-0 bottom-0 w-px h-px opacity-0 pointer-events-none"
          />
        </span>
        <SettingsButton onClick={resetColors} variant="secondary" title="恢复默认主题色">
          重置
        </SettingsButton>
      </div>
    </SettingsSection>
  );
}
