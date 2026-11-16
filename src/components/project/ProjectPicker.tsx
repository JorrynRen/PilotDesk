import { useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { FolderKanban, FolderOpen, Loader2, X } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { Select } from '../common/Select';
import { useProjectStore } from '../../stores/projectStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { normalizeDisplayPath } from '../../utils/path';
import { projectPrimaryDirectory } from '../../utils/project';
import type { Project, ProjectWorkKind } from '../../types/project';

export const PROJECT_FILTER_ALL = '__all__';
export const PROJECT_FILTER_UNASSIGNED = '__unassigned__';
const PROJECT_SELECT_MANAGE = '__manage_projects__';
/** 会话、圆桌等紧凑列表共用的项目徽标宽度与收缩规则。 */
export const PROJECT_BADGE_LIST_CLASS = 'shrink-0 max-w-[112px]';

function projectLabel(project: Project): string {
  return `${project.icon || '📁'} ${project.name}`;
}

export function ProjectFilter({
  value,
  onChange,
  className,
  width = 132,
}: {
  value: string;
  onChange: (value: string) => void;
  className?: string;
  width?: number | string;
}) {
  const projects = useProjectStore((state) => state.projects);
  const loadProjects = useProjectStore((state) => state.loadProjects);

  useEffect(() => {
    if (projects.length === 0) void loadProjects();
  }, [loadProjects, projects.length]);

  const options = useMemo(
    () => [
      { value: PROJECT_FILTER_ALL, label: '全部项目' },
      { value: PROJECT_FILTER_UNASSIGNED, label: '未归属项目' },
      ...projects.map((project) => ({
        value: project.id,
        label: `${projectLabel(project)}${project.archivedAt ? '（已归档）' : ''}`,
      })),
    ],
    [projects],
  );

  return (
    <Select
      value={value}
      onChange={onChange}
      options={options}
      size="sm"
      className={className}
      style={{ width }}
      title="按项目筛选"
      ariaLabel="按项目筛选"
    />
  );
}

export function ProjectSelect({
  value,
  onChange,
  className = 'w-full',
  disabled = false,
  ariaLabel = '选择项目',
}: {
  value: string;
  onChange: (value: string) => void;
  className?: string;
  disabled?: boolean;
  ariaLabel?: string;
}) {
  const navigate = useNavigate();
  const projects = useProjectStore((state) => state.projects);
  const loadProjects = useProjectStore((state) => state.loadProjects);

  useEffect(() => {
    if (projects.length === 0) void loadProjects();
  }, [loadProjects, projects.length]);

  const options = useMemo(
    () => [
      { value: '', label: '未归属项目' },
      ...projects
        .filter((project) => !project.archivedAt || project.id === value)
        .map((project) => ({
          value: project.id,
          label: `${projectLabel(project)}${project.archivedAt ? '（已归档，不可新归属）' : ''}`,
        })),
      {
        value: PROJECT_SELECT_MANAGE,
        label: '管理项目和目录…',
        icon: <FolderKanban size={13} />,
      },
    ],
    [projects, value],
  );

  const handleChange = (nextValue: string) => {
    if (nextValue === PROJECT_SELECT_MANAGE) {
      navigate(value ? `/projects?projectId=${encodeURIComponent(value)}` : '/projects');
      return;
    }
    onChange(nextValue);
  };

  return (
    <Select
      value={value}
      onChange={handleChange}
      options={options}
      className={className}
      disabled={disabled}
      ariaLabel={ariaLabel}
      title={ariaLabel}
    />
  );
}

export function ProjectBadge({
  projectId,
  className = '',
}: {
  projectId?: string | null;
  className?: string;
}) {
  const project = useProjectStore((state) =>
    projectId ? state.projects.find((item) => item.id === projectId) : undefined,
  );
  if (!project) return null;
  return (
    <span
      className={`project-work-badge ${className}`}
      title={project.archivedAt ? `已归档项目：${project.name}` : `项目：${project.name}`}
    >
      <span aria-hidden="true">{project.icon || '📁'}</span>
      <span className="truncate">{project.name}</span>
    </span>
  );
}

export function ProjectMembershipDialog({
  workType,
  workId,
  workTitle,
  currentProjectId,
  onClose,
  onChanged,
}: {
  workType: ProjectWorkKind;
  workId: string;
  workTitle: string;
  currentProjectId?: string | null;
  onClose: () => void;
  onChanged: (projectId: string | null) => void | Promise<void>;
}) {
  const [projectId, setProjectId] = useState(currentProjectId ?? '');
  const [saving, setSaving] = useState(false);
  const [globalWorkspacePath, setGlobalWorkspacePath] = useState('');
  const [globalWorkspaceState, setGlobalWorkspaceState] = useState<'loading' | 'ready' | 'error'>('loading');
  const projects = useProjectStore((state) => state.projects);
  const selectedProject = projectId
    ? projects.find((project) => project.id === projectId)
    : undefined;
  const primaryDirectory = normalizeDisplayPath(projectPrimaryDirectory(selectedProject));
  const previewPath = projectId ? primaryDirectory : globalWorkspacePath;
  const directoryLabel = projectId ? '主目录预览' : '全局工作空间';
  const directoryPreview = projectId
    ? selectedProject
      ? primaryDirectory || '该项目尚未设置主目录'
      : '正在读取项目主目录…'
    : globalWorkspaceState === 'loading'
      ? '正在读取全局工作空间路径…'
      : globalWorkspacePath || '无法读取全局工作空间路径';

  useEffect(() => {
    let cancelled = false;
    void invoke<string>('resolve_default_workspace')
      .then((path) => {
        if (cancelled) return;
        setGlobalWorkspacePath(normalizeDisplayPath(path));
        setGlobalWorkspaceState('ready');
      })
      .catch(() => {
        if (cancelled) return;
        setGlobalWorkspaceState('error');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !saving) onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose, saving]);

  const submit = async () => {
    setSaving(true);
    try {
      await invoke('project_set_work_membership', {
        workType,
        workId,
        projectId: projectId || null,
      });
      await onChanged(projectId || null);
      showToast(projectId ? '已更新项目归属' : '已移出项目', 'success');
      onClose();
    } catch (error) {
      showToast(`更新项目归属失败：${errorMessage(error)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  return createPortal(
    <div className="project-membership-backdrop" role="presentation" onClick={onClose}>
      <div
        className="project-membership-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="设置项目归属"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="project-membership-head">
          <div>
            <span>项目归属</span>
            <strong>{workTitle || '未命名工作'}</strong>
          </div>
          <button type="button" onClick={onClose} disabled={saving} aria-label="关闭">
            <X size={14} />
          </button>
        </div>
        <div className="project-membership-body">
          <div className="project-membership-hint">
            <FolderKanban size={14} />
            <span>归属后默认使用项目主目录；主目录和关联目录自动进入安全路径，圆桌与工作流不再单独保存工作根。</span>
          </div>
          <label className="project-membership-field">
            <span>归属到</span>
            <ProjectSelect value={projectId} onChange={setProjectId} />
          </label>
          <div className="project-membership-directory" data-empty={!previewPath}>
            <FolderOpen size={14} aria-hidden="true" />
            <div>
              <span>{directoryLabel}</span>
              <code title={previewPath || undefined}>{directoryPreview}</code>
            </div>
          </div>
        </div>
        <div className="project-membership-footer">
          <button type="button" onClick={onClose} disabled={saving}>取消</button>
          <button type="button" data-primary="true" onClick={() => void submit()} disabled={saving}>
            {saving && <Loader2 size={12} className="animate-spin" />}
            保存
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
