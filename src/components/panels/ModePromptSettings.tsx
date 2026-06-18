import { useState, useEffect, useCallback } from 'react';
import type { ChatMode } from '../../types';
import { getModePrompt, MODE_LABELS, MODE_PROMPTS_DEFAULTS, invoke } from '../../types';
import { SettingsSection, SettingsButton, SettingsInput } from '../settings';

export function ModePromptSettings() {
  const [prompts, setPrompts] = useState<Record<ChatMode, string>>({
    native: '',
    fast: '',
    think: '',
    expert: '',
  });
  const [saving, setSaving] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      const loaded: Record<ChatMode, string> = { native: '', fast: '', think: '', expert: '' };
      for (const m of ['native', 'fast', 'think', 'expert'] as ChatMode[]) {
        loaded[m] = await getModePrompt(m);
      }
      setPrompts(loaded);
    })();
  }, []);

  const handleChange = useCallback((mode: ChatMode, value: string) => {
    setPrompts((prev) => ({ ...prev, [mode]: value }));
    setSaved(null);
  }, []);

  const handleSave = useCallback(async (mode: ChatMode) => {
    setSaving(mode);
    setSaved(null);
    try {
      const key = `mode_prompt_${mode}`;
      await invoke('set_app_setting', { key, value: prompts[mode] });
      setSaved(mode);
    } catch (e) {
      console.error('saveModePrompt failed:', e);
    }
    setSaving(null);
    setTimeout(() => setSaved(null), 2000);
  }, [prompts, invoke]);

  const handleReset = useCallback(async (mode: ChatMode) => {
    setPrompts((prev) => ({ ...prev, [mode]: MODE_PROMPTS_DEFAULTS[mode] }));
    const key = `mode_prompt_${mode}`;
    try {
      await invoke('set_app_setting', { key, value: MODE_PROMPTS_DEFAULTS[mode] });
    } catch (e) {
      console.error('reset failed:', e);
    }
    setSaved(mode);
    setTimeout(() => setSaved(null), 2000);
  }, [invoke]);

  return (
    <SettingsSection
      title="对话模式设置"
      description="自定义各对话模式的系统提示词。发送消息时将使用自定义系统提示词追加到用户消息的头部一同发出。"
    >
      {(['native', 'fast', 'think', 'expert'] as ChatMode[]).map((mode) => (
        <div
          key={mode}
          className="mb-3 last:mb-0 rounded-lg p-3"
          style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
        >
          <div className="flex items-center justify-between mb-2">
            <span style={{ color: 'var(--accent)', fontSize: '12px', fontWeight: 500 }}>
              {MODE_LABELS[mode]}
            </span>
            <div className="flex items-center gap-2">
              <SettingsButton onClick={() => handleReset(mode)} variant="secondary">
                重置
              </SettingsButton>
              <SettingsButton
                onClick={() => handleSave(mode)}
                disabled={saving === mode}
                variant={saved === mode ? 'success' : 'primary'}
              >
                {saving === mode ? '保存中...' : saved === mode ? '✓ 已保存' : '保存'}
              </SettingsButton>
            </div>
          </div>

          <SettingsInput
            value={prompts[mode]}
            onChange={(value) => handleChange(mode, value)}
            placeholder={`输入 ${MODE_LABELS[mode]} 模式的系统提示词...`}
            multiline
            rows={3}
          />
        </div>
      ))}
    </SettingsSection>
  );
}
