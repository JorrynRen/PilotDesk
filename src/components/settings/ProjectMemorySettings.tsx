import { useEffect, useState } from 'react';
import { SettingsSection, SettingsButton } from './index';
import { Select } from '../common/Select';
import {
  getProjectMemory,
  listProjectRoots,
  projectMemoryTemplate,
  updateProjectMemory,
  type ProjectMemoryInfo,
} from '../../types';
import { errorMessage } from '../../utils/errorMessage';

const MAX_CHARS = 12000;

/** 设置路由「记忆管理」：项目级 MEMORY.md 编辑器（会话记忆 v1） */
export function ProjectMemorySettings() {
  const [roots, setRoots] = useState<string[]>([]);
  const [selected, setSelected] = useState<string>('');
  const [loaded, setLoaded] = useState<ProjectMemoryInfo | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  // 加载可选项目根
  useEffect(() => {
    (async () => {
      try {
        const rs = await listProjectRoots();
        setRoots(rs);
        if (rs.length > 0) setSelected(rs[0]);
      } catch {
        setError('加载项目列表失败');
      }
    })();
  }, []);

  // 切换项目根时加载对应 MEMORY.md
  useEffect(() => {
    if (!selected) return;
    const root = selected;
    (async () => {
      setLoaded(null);
      setDraft('');
      setError('');
      try {
        const mem = await getProjectMemory(root);
        setLoaded(mem);
        setDraft(mem.content);
      } catch (e) {
        setError(errorMessage(e));
      }
    })();
  }, [selected]);

  const chars = draft.length;
  const overLimit = chars > MAX_CHARS;
  const dirty = loaded !== null && draft !== loaded.content;

  const handleSave = async () => {
    if (!selected || overLimit) return;
    setSaving(true);
    setError('');
    try {
      const mem = await updateProjectMemory(selected, draft);
      setLoaded(mem);
      setDraft(mem.content);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const handleTemplate = async () => {
    if (!selected) return;
    setError('');
    try {
      const tpl = await projectMemoryTemplate();
      setDraft(tpl);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <SettingsSection title="项目记忆（MEMORY.md）">
      <p className="text-xs mb-3" style={{ color: 'var(--text-secondary)' }}>
        每个项目（工作目录）一份 <code>MEMORY.md</code>，随会话发送时注入模型上下文。选择要管理的项目后编辑并保存。
      </p>

      {roots.length === 0 ? (
        <div className="rounded-lg px-3 py-4 text-center text-xs" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          暂无可管理的项目。请先在会话中设置工作目录，稍后刷新。
        </div>
      ) : (
        <div className="space-y-3">
          {/* 项目选择器 */}
          <div>
            <label className="block text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
              项目（工作目录）
            </label>
            <Select
              value={selected}
              onChange={(v) => setSelected(v)}
              options={roots.map((r) => ({ value: r, label: r }))}
              className="w-full"
            />
          </div>

          {selected && (
            <>
              {/* 编辑器 */}
              <div>
                <div className="flex items-center justify-between mb-1">
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    MEMORY.md{loaded ? (loaded.exists ? '' : '（尚不存在）') : ''}
                  </span>
                  <span className="text-[10px]" style={{ color: overLimit ? 'var(--danger, #EF4444)' : 'var(--text-tertiary)' }}>
                    {chars}/{MAX_CHARS} 字符
                  </span>
                </div>
                <textarea
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  spellCheck={false}
                  placeholder={loaded?.exists ? '在此编辑项目长期记忆…' : '该目录还没有 MEMORY.md。点击「使用模板创建」或直接输入内容后保存。'}
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
                <span className="text-[11px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                  记忆根：{selected}
                  {!loaded?.exists && '（尚未创建 MEMORY.md，保存后自动创建）'}
                </span>
              </div>
            </>
          )}
        </div>
      )}
    </SettingsSection>
  );
}
