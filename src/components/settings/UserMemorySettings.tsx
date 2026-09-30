import { useEffect, useState } from 'react';
import { SettingsSection, SettingsButton } from './index';
import {
  getUserPreferences,
  updateUserPreferences,
  userPreferencesTemplate,
  type ProjectMemoryInfo,
} from '../../types';
import { errorMessage } from '../../utils/errorMessage';

const MAX_CHARS = 12000;

/** 设置路由「记忆管理」→ 用户偏好：统一配置根下的 USER.md（随每次会话注入模型上下文） */
export function UserMemorySettings() {
  const [loaded, setLoaded] = useState<ProjectMemoryInfo | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    (async () => {
      setLoaded(null);
      setDraft('');
      setError('');
      try {
        const mem = await getUserPreferences();
        setLoaded(mem);
        setDraft(mem.content);
      } catch (e) {
        setError(errorMessage(e));
      }
    })();
  }, []);

  const chars = draft.length;
  const overLimit = chars > MAX_CHARS;
  const dirty = loaded !== null && draft !== loaded.content;

  const handleSave = async () => {
    if (overLimit || saving) return;
    setSaving(true);
    setError('');
    try {
      const mem = await updateUserPreferences(draft);
      setLoaded(mem);
      setDraft(mem.content);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const handleTemplate = async () => {
    setError('');
    try {
      setDraft(await userPreferencesTemplate());
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <SettingsSection title="用户偏好（USER.md）">
      <p className="text-xs mb-3" style={{ color: 'var(--text-secondary)' }}>
        全局用户级偏好文件，位于统一配置根，会随每次对话注入模型上下文。填写跨项目的通用偏好（语言/框架、沟通风格、常用命令、命名习惯等）。
      </p>

      <div className="space-y-3">
        <div>
          <div className="flex items-center justify-between mb-1">
            <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              USER.md{loaded ? (loaded.exists ? '' : '（尚不存在）') : ''}
            </span>
            <span className="text-[10px]" style={{ color: overLimit ? 'var(--danger, #EF4444)' : 'var(--text-tertiary)' }}>
              {chars}/{MAX_CHARS} 字符
            </span>
          </div>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            spellCheck={false}
            placeholder={loaded?.exists ? '在此编辑全局用户偏好…' : '配置根还没有 USER.md。点击「使用模板创建」或直接输入内容后保存。'}
            className="w-full px-3 py-2 rounded-lg text-xs font-mono leading-relaxed resize-y outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
              minHeight: '220px',
            }}
          />
        </div>

        {error && <div className="text-xs" style={{ color: 'var(--danger, #EF4444)' }}>{error}</div>}

        <div className="flex flex-wrap items-center gap-2">
          <SettingsButton onClick={handleSave} disabled={saving || !dirty || overLimit}>
            {saving ? '保存中…' : '保存'}
          </SettingsButton>
          {!loaded?.exists && (
            <SettingsButton onClick={handleTemplate} disabled={saving}>
              使用模板创建
            </SettingsButton>
          )}
          {dirty && !overLimit && (
            <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>有未保存修改</span>
          )}
          {loaded && (
            <span className="text-[11px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
              记忆根：{loaded.root}
              {!loaded.exists && '（尚未创建 USER.md，保存后自动创建）'}
            </span>
          )}
        </div>
      </div>
    </SettingsSection>
  );
}
