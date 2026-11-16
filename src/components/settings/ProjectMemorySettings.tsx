import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FilePlus2, Loader2, Save } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import { Select } from '../common/Select';
import { useProjectStore } from '../../stores/projectStore';
import {
  getProjectMemory,
  listProjectRoots,
  projectMemoryTemplate,
  updateProjectMemory,
  type ProjectMemoryInfo,
} from '../../types';
import { errorMessage } from '../../utils/errorMessage';
import { normalizeDisplayPath } from '../../utils/path';
import { projectPrimaryDirectory, sameProjectPath } from '../../utils/project';

const MAX_CHARS = 12000;

/** 设置路由「记忆管理」：项目级 AGENTS.md 编辑器（会话记忆 v1） */
export function ProjectMemorySettings() {
  const [roots, setRoots] = useState<string[]>([]);
  const [selected, setSelected] = useState<string>('');
  const [loaded, setLoaded] = useState<ProjectMemoryInfo | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [projectsLoadFinished, setProjectsLoadFinished] = useState(false);
  const [globalWorkspacePath, setGlobalWorkspacePath] = useState('');
  const projects = useProjectStore((state) => state.projects);
  const loadProjects = useProjectStore((state) => state.loadProjects);
  const projectsReady = projects.length > 0 || projectsLoadFinished;

  useEffect(() => {
    if (projects.length > 0) return;
    let disposed = false;
    void loadProjects().finally(() => {
      if (!disposed) setProjectsLoadFinished(true);
    });
    return () => {
      disposed = true;
    };
  }, [loadProjects, projects.length]);

  useEffect(() => {
    let disposed = false;
    void invoke<string>('resolve_default_workspace')
      .then((path) => {
        if (!disposed) setGlobalWorkspacePath(normalizeDisplayPath(path));
      })
      .catch(() => {
        if (!disposed) setGlobalWorkspacePath('');
      });
    return () => {
      disposed = true;
    };
  }, []);

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

  // 切换项目根时加载对应 AGENTS.md
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
  const rootOptions = useMemo(
    () =>
      roots.map((root) => {
        const project = projects.find(
          (item) => sameProjectPath(projectPrimaryDirectory(item), root),
        );
        const name = project
          ? project.name
          : sameProjectPath(globalWorkspacePath, root)
            ? '全局工作空间'
            : projectsReady
              ? '项目目录'
              : '正在识别项目';
        return {
          value: root,
          label: `[${name}] ${normalizeDisplayPath(root)}`,
        };
      }),
    [globalWorkspacePath, projects, projectsReady, roots],
  );
  const selectedRootLabel =
    rootOptions.find((option) => option.value === selected)?.label
    ?? normalizeDisplayPath(selected);

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
    <SettingsSection
      title="项目记忆（AGENTS.md）"
      description="每个项目主目录一份 AGENTS.md；PilotDesk API 会话会自动读取并注入。CLI/ACP 宿主会话由宿主自行发现和处理，PilotDesk 不注入内容。"
      actions={
        <>
          <SettingsButton
            onClick={handleSave}
            variant="primary"
            disabled={!selected || saving || !dirty || overLimit}
            icon={saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
          >
            {saving ? '保存中…' : '保存'}
          </SettingsButton>
          {selected && !loaded?.exists && (
            <SettingsButton
              onClick={handleTemplate}
              disabled={saving}
              icon={<FilePlus2 size={12} />}
            >
              使用模板创建
            </SettingsButton>
          )}
        </>
      }
    >
      {roots.length === 0 ? (
        <div className="pd-settings-empty">
          <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            暂无可管理的项目
          </p>
          <p className="pd-settings-note">创建项目并设置主目录后，可在此维护项目记忆。</p>
        </div>
      ) : (
        <div className="space-y-3">
          {/* 项目选择器 */}
          <div>
            <label className="block text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
              项目 / 全局工作空间
            </label>
            <Select
              value={selected}
              onChange={(v) => setSelected(v)}
              options={rootOptions}
              className="w-full"
            />
          </div>

          {selected && (
            <>
              {/* 编辑器 */}
              <div>
                <div className="flex items-center justify-between mb-1">
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    AGENTS.md{loaded ? (loaded.exists ? '' : '（尚不存在）') : ''}
                  </span>
                  <span className="text-[10px]" style={{ color: overLimit ? 'var(--danger, #EF4444)' : 'var(--text-tertiary)' }}>
                    {chars}/{MAX_CHARS} 字符
                  </span>
                </div>
                <textarea
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  spellCheck={false}
                  placeholder={loaded?.exists ? '在此编辑项目长期记忆…' : '该目录还没有 AGENTS.md。点击「使用模板创建」或直接输入内容后保存。'}
                  className="w-full px-3 py-2 rounded-lg text-xs font-mono leading-relaxed resize-y outline-none"
                  style={{
                    backgroundColor: 'var(--bg-field)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                    minHeight: '220px',
                  }}
                />
              </div>

              {error && <div className="text-xs" style={{ color: 'var(--danger, #EF4444)' }}>{error}</div>}

              <div className="flex flex-wrap items-center gap-2">
                {dirty && !overLimit && (
                  <span className="pd-settings-note">有未保存修改</span>
                )}
                <span className="pd-settings-note min-w-0 truncate">
                  记忆根：{selectedRootLabel}
                  {!loaded?.exists && '（尚未创建 AGENTS.md，保存后自动创建）'}
                </span>
              </div>
            </>
          )}
        </div>
      )}
    </SettingsSection>
  );
}
