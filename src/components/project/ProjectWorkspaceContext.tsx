import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  ArrowUpRight,
  ChevronDown,
  Folder,
  FolderKanban,
  Link2,
  PackageOpen,
  ShieldCheck,
} from 'lucide-react';

import { useProjectStore } from '../../stores/projectStore';
import {
  projectLinkedDirectories,
  projectPrimaryDirectory,
  sameProjectPath,
} from '../../utils/project';
import {
  joinDisplayPath,
  normalizeDisplayPath,
  pathBasename,
} from '../../utils/path';

interface ProjectWorkspaceContextProps {
  projectId?: string | null;
  workRoot?: string | null;
  artifactRoot?: string | null;
  artifactRelativePath?: string;
  artifactHint?: string;
  /** 显式运行目录的来源文案，例如宿主恢复目录。 */
  rootSourceLabel?: string;
  /** 工具栏场景使用单行紧凑入口，完整目录信息仍保留在弹层中。 */
  compact?: boolean;
  /** 极窄工具栏只保留图标入口，完整信息通过提示和弹层查看。 */
  iconOnly?: boolean;
  className?: string;
}

/**
 * 项目工作区的统一消费入口。
 *
 * 只展示运行时已经采用的目录语义，不在前端重新推导授权范围：
 * 主目录是工作根，关联目录进入同一套安全路径，产物目录从当前工作根派生。
 */
export function ProjectWorkspaceContext({
  projectId,
  workRoot,
  artifactRoot,
  artifactRelativePath,
  artifactHint,
  rootSourceLabel,
  compact = false,
  iconOnly = false,
  className = '',
}: ProjectWorkspaceContextProps) {
  const navigate = useNavigate();
  const projects = useProjectStore((state) => state.projects);
  const loadProjects = useProjectStore((state) => state.loadProjects);
  const project = projects.find((item) => item.id === projectId);
  const [globalRoot, setGlobalRoot] = useState('');
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 8, top: 8 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  const primaryRoot = projectPrimaryDirectory(project);
  const linkedDirectories = projectLinkedDirectories(project);
  const resolvedWorkRoot = normalizeDisplayPath(workRoot?.trim() || primaryRoot || globalRoot);
  const effectiveArtifactRoot = normalizeDisplayPath(artifactRoot?.trim())
    || (artifactRelativePath && resolvedWorkRoot
      ? joinDisplayPath(resolvedWorkRoot, artifactRelativePath)
      : '');
  const rootSource = rootSourceLabel?.trim()
    || (project
      ? (workRoot?.trim() && primaryRoot && !sameProjectPath(workRoot, primaryRoot)
        ? '会话覆盖'
        : '项目主目录')
      : workRoot?.trim()
        ? '独立目录'
        : '全局工作空间');

  useEffect(() => {
    if (projects.length === 0) void loadProjects();
  }, [loadProjects, projects.length]);

  useEffect(() => {
    if (projectId || workRoot?.trim() || globalRoot) return;
    let cancelled = false;
    void invoke<string>('resolve_default_workspace')
      .then((value) => {
        if (!cancelled) setGlobalRoot(value);
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [globalRoot, projectId, workRoot]);

  const updatePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(360, window.innerWidth - 16);
    const estimatedHeight = 330;
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    const below = rect.bottom + 6;
    const top = below + estimatedHeight <= window.innerHeight
      ? below
      : Math.max(8, rect.top - estimatedHeight - 6);
    setPosition({ left, top });
  }, [setPosition]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    updatePosition();
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open, updatePosition]);

  const openProject = () => {
    if (!project) return;
    setOpen(false);
    navigate(`/projects?projectId=${encodeURIComponent(project.id)}`);
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={`project-workspace-trigger${compact ? ' is-compact' : ''}${iconOnly ? ' is-icon-only' : ''} ${className}`}
        onClick={() => {
          updatePosition();
          setOpen((value) => !value);
        }}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={resolvedWorkRoot ? `${rootSource}：${resolvedWorkRoot}` : '查看工作归属与目录'}
        title={resolvedWorkRoot
          ? `${rootSource}：${resolvedWorkRoot}${linkedDirectories.length > 0 ? `\n关联目录：${linkedDirectories.length} 个` : ''}`
          : '查看工作归属与目录'}
      >
        <span className="project-workspace-trigger-icon" aria-hidden="true">
          {project?.icon || <FolderKanban size={12} />}
        </span>
        {!iconOnly && (
          <>
            <span className="project-workspace-trigger-copy">
              <strong className="truncate">{project?.name || (compact ? rootSource : '全局工作空间')}</strong>
              {!compact && (
                <small className="truncate">
                  {resolvedWorkRoot ? pathBasename(resolvedWorkRoot) : rootSource}
                  {linkedDirectories.length > 0 ? ` · ${linkedDirectories.length} 关联` : ''}
                </small>
              )}
            </span>
            <ChevronDown
              size={10}
              className={open ? 'project-workspace-trigger-chevron is-open' : 'project-workspace-trigger-chevron'}
            />
          </>
        )}
      </button>

      {open && createPortal(
        <div
          ref={popoverRef}
          className="project-workspace-popover"
          role="dialog"
          aria-label="项目工作区"
          style={{ left: position.left, top: position.top }}
        >
          <div className="project-workspace-popover-head">
            <span className="project-workspace-popover-icon" aria-hidden="true">
              {project?.icon || <FolderKanban size={15} />}
            </span>
            <span className="project-workspace-popover-title">
              <strong>{project?.name || '未归属项目'}</strong>
              <small>{project?.archivedAt ? '项目已归档，仍沿用现有工作目录' : '工作归属与目录'}</small>
            </span>
            {project && (
              <button type="button" onClick={openProject}>
                管理项目
                <ArrowUpRight size={11} />
              </button>
            )}
          </div>

          <div className="project-workspace-popover-body">
            <div className="project-workspace-section">
              <div className="project-workspace-section-title">
                <Folder size={11} />
                工作根
                <span>{rootSource}</span>
              </div>
              <div className="project-workspace-path" title={resolvedWorkRoot || undefined}>
                <strong>{resolvedWorkRoot ? pathBasename(resolvedWorkRoot) : '运行时解析'}</strong>
                <code>{resolvedWorkRoot || '尚未读取到默认工作空间'}</code>
              </div>
            </div>

            <div className="project-workspace-section">
              <div className="project-workspace-section-title">
                <Link2 size={11} />
                关联目录
                <span>{linkedDirectories.length} 个</span>
              </div>
              {linkedDirectories.length > 0 ? (
                <div className="project-workspace-linked-list">
                  {linkedDirectories.map((directory) => (
                    <div
                      key={directory.id}
                      className="project-workspace-linked-row"
                      title={normalizeDisplayPath(directory.path)}
                    >
                      <Folder size={10} />
                      <span>{pathBasename(directory.path)}</span>
                      <code>{normalizeDisplayPath(directory.path)}</code>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="project-workspace-empty">未添加关联目录</div>
              )}
            </div>

            {(artifactRoot !== undefined || artifactRelativePath || artifactHint) && (
              <div className="project-workspace-section">
                <div className="project-workspace-section-title">
                  <PackageOpen size={11} />
                  产物目录
                  <span>由工作根派生</span>
                </div>
                <div className="project-workspace-path" title={effectiveArtifactRoot || artifactHint}>
                  <strong>{effectiveArtifactRoot ? pathBasename(effectiveArtifactRoot) : '按工作自动生成'}</strong>
                  <code>{effectiveArtifactRoot || artifactHint || '运行时解析'}</code>
                </div>
              </div>
            )}
          </div>

          <div className="project-workspace-popover-foot">
            <ShieldCheck size={11} />
            {artifactRoot !== undefined || artifactRelativePath
              ? '主目录、关联目录与产物目录统一进入当前安全路径矩阵'
              : '主目录与关联目录统一进入当前安全路径矩阵'}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
