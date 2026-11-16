import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import {
  Archive,
  ArchiveRestore,
  ArrowRight,
  Check,
  Folder,
  FolderKanban,
  FolderOpen,
  FolderPlus,
  Loader2,
  MessageSquare,
  Pencil,
  Plus,
  Search,
  Trash2,
  Users,
  Workflow,
  X,
} from 'lucide-react';
import { WorkbenchAssetPage } from '../components/layout';
import { PageToolbar, type ToolbarAction } from '../components/common/PageToolbar';
import { ProjectMembershipDialog } from '../components/project/ProjectPicker';
import { NewSessionDialog } from '../components/session/NewSessionDialog';
import { CreateRoomModal } from '../components/groupchat/CreateRoomModal';
import { WorkflowPropertyDialog } from '../components/workflow/WorkflowPropertyDialog';
import { useProjectStore } from '../stores/projectStore';
import { useSessionStore } from '../stores/sessionStore';
import { useGroupChatStore } from '../stores/groupChatStore';
import { useWorkflowStore } from '../stores/workflowStore';
import { useTerminal } from '../TerminalManager';
import { confirmDialog } from '../stores/confirmStore';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import { projectPrimaryDirectory } from '../utils/project';
import { pathBasename } from '../utils/path';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import type {
  Project,
  ProjectCreateKind,
  ProjectDirectoryInput,
  ProjectWorkItem,
  ProjectWorkKind,
} from '../types/project';

type ProjectScope = 'active' | 'archived';

interface ProjectEditorState {
  mode: 'create' | 'edit';
  project?: Project;
  section?: 'details' | 'directories';
}

const PROJECT_EMOJI_GROUPS: Array<{ label: string; emojis: string[] }> = [
  { label: '项目', emojis: ['📁', '💼', '🎯', '🚀', '🌐'] },
  { label: '产品/设计', emojis: ['🧩', '🎨', '📱', '💡', '📝'] },
  { label: '研发/数据', emojis: ['💻', '⚙️', '📊', '🧪', '🔬'] },
  { label: '内容/知识', emojis: ['📚', '📰', '🧠', '🎬', '🔗'] },
];

const WORK_KIND_LABEL: Record<ProjectWorkKind, string> = {
  session: '会话',
  groupchat: '圆桌',
  workflow: '工作流',
};

const WORK_KIND_ICON: Record<ProjectWorkKind, ReactNode> = {
  session: <MessageSquare size={13} />,
  groupchat: <Users size={13} />,
  workflow: <Workflow size={13} />,
};

function projectIcon(project?: Pick<Project, 'icon'> | null): string {
  return project?.icon || '📁';
}

function workStatusLabel(item: ProjectWorkItem): string {
  if (item.kind === 'workflow') {
    return item.status === 'enabled' ? '已启用' : '已禁用';
  }
  if (item.kind === 'groupchat') {
    return ({
      idle: '待开始',
      running: '讨论中',
      paused: '已暂停',
      finished: '已结束',
      aborted: '已中止',
    } as Record<string, string>)[item.status] || item.status;
  }
  return item.status === 'archived' ? '已归档' : '会话';
}

function formatActivity(timestamp: number): string {
  if (!timestamp) return '暂无活动';
  const delta = Math.max(0, Math.floor(Date.now() / 1000) - timestamp);
  if (delta < 60) return '刚刚活跃';
  if (delta < 3600) return `${Math.floor(delta / 60)} 分钟前`;
  if (delta < 86400) return `${Math.floor(delta / 3600)} 小时前`;
  if (delta < 86400 * 7) return `${Math.floor(delta / 86400)} 天前`;
  return new Date(timestamp * 1000).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

function projectMatches(project: Project, query: string): boolean {
  const keyword = query.trim().toLowerCase();
  if (!keyword) return true;
  return [
    project.name,
    project.description,
    ...project.directories.map((directory) => directory.path),
  ].some((value) => value.toLowerCase().includes(keyword));
}

export function ProjectsPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const projectIdParam = searchParams.get('projectId');
  const { setMode } = useTerminal();
  const projects = useProjectStore((state) => state.projects);
  const loading = useProjectStore((state) => state.loading);
  const error = useProjectStore((state) => state.error);
  const loadProjects = useProjectStore((state) => state.loadProjects);
  const archiveProject = useProjectStore((state) => state.archiveProject);
  const restoreProject = useProjectStore((state) => state.restoreProject);
  const loadWorkItems = useProjectStore((state) => state.loadWorkItems);
  const selectSession = useSessionStore((state) => state.selectSession);
  const selectRoom = useGroupChatStore((state) => state.selectRoom);
  const createDefinition = useWorkflowStore((state) => state.createDefinition);
  const selectDefinition = useWorkflowStore((state) => state.selectDefinition);

  const [scope, setScope] = useState<ProjectScope>('active');
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(projectIdParam);
  const [syncedProjectParam, setSyncedProjectParam] = useState<string | null>(null);
  const [editor, setEditor] = useState<ProjectEditorState | null>(null);
  const [archivingId, setArchivingId] = useState<string | null>(null);
  const [workItems, setWorkItems] = useState<ProjectWorkItem[]>([]);
  const [workItemsLoading, setWorkItemsLoading] = useState(false);
  const [workItemsSourceId, setWorkItemsSourceId] = useState<string | null | undefined>(undefined);
  const [workFilter, setWorkFilter] = useState<'all' | ProjectWorkKind>('all');
  const [membershipItem, setMembershipItem] = useState<ProjectWorkItem | null>(null);
  const [creation, setCreation] = useState<{
    kind: ProjectCreateKind;
    project: Project;
  } | null>(null);

  useEffect(() => {
    void loadProjects();
  }, [loadProjects]);

  if (projectIdParam && syncedProjectParam !== projectIdParam) {
    const project = projects.find((item) => item.id === projectIdParam);
    if (project) {
      setSyncedProjectParam(projectIdParam);
      setScope(project.archivedAt ? 'archived' : 'active');
      setSelectedId(project.id);
    }
  }

  const activeProjects = useMemo(
    () => projects.filter((project) => !project.archivedAt),
    [projects],
  );
  const archivedProjects = useMemo(
    () => projects.filter((project) => Boolean(project.archivedAt)),
    [projects],
  );
  const visibleProjects = useMemo(() => {
    const base = scope === 'active' ? activeProjects : archivedProjects;
    return base.filter((project) => projectMatches(project, query));
  }, [activeProjects, archivedProjects, query, scope]);

  const selected = useMemo(() => {
    return visibleProjects.find((project) => project.id === selectedId)
      ?? visibleProjects[0]
      ?? null;
  }, [selectedId, visibleProjects]);

  const selectedProjectId = selected?.id ?? null;
  if (workItemsSourceId !== selectedProjectId) {
    setWorkItemsSourceId(selectedProjectId);
    setWorkItems([]);
    setWorkItemsLoading(Boolean(selectedProjectId));
    setWorkFilter('all');
  }

  useEffect(() => {
    if (!selectedProjectId) return;
    let cancelled = false;
    void loadWorkItems(selectedProjectId)
      .then((items) => {
        if (!cancelled) setWorkItems(items);
      })
      .catch((error) => {
        if (!cancelled) {
          setWorkItems([]);
          showToast(`加载项目工作失败：${errorMessage(error)}`, 'error');
        }
      })
      .finally(() => {
        if (!cancelled) setWorkItemsLoading(false);
      });
    return () => { cancelled = true; };
  }, [loadWorkItems, selectedProjectId]);

  const visibleWorkItems = useMemo(
    () => workFilter === 'all' ? workItems : workItems.filter((item) => item.kind === workFilter),
    [workFilter, workItems],
  );

  const openWorkItem = async (item: ProjectWorkItem) => {
    if (item.kind === 'session') {
      await selectSession(item.id);
      setMode('session');
      navigate('/');
      return;
    }
    if (item.kind === 'groupchat') {
      await selectRoom(item.id);
      setMode('groupchat');
      navigate('/');
      return;
    }
    navigate(`/workflow/editor?id=${item.id}`);
  };

  const reloadSelectedWork = async () => {
    if (!selected?.id) return;
    await useProjectStore.getState().loadProjects();
    const items = await loadWorkItems(selected.id);
    setWorkItems(items);
  };

  const launchCreation = (kind: ProjectCreateKind, project: Project) => {
    setCreation({ kind, project });
  };

  const handleCreated = async (kind: ProjectCreateKind, id: string) => {
    await useProjectStore.getState().loadProjects();
    setCreation(null);
    if (kind === 'session') {
      await selectSession(id);
      setMode('session');
      navigate('/');
      return;
    }
    if (kind === 'groupchat') {
      await selectRoom(id);
      setMode('groupchat');
      navigate('/');
      return;
    }
    selectDefinition(id);
    setMode('workflow');
    navigate(`/workflow/editor?id=${id}`);
  };

  const handleWorkflowPropertyConfirm = async (data: {
    name: string;
    description: string;
    version: string;
    trigger: { triggerType: 'manual' | 'cron' | 'event'; cron?: string; eventName?: string };
    enabled: boolean;
    icon?: string | null;
    projectId?: string;
    inputSchema?: Record<string, { type: string; description?: string; required?: boolean; default?: import('../types/plugin').JsonValue }>;
    outputSchema?: Record<string, { type: string; description?: string }>;
  }) => {
    const definition = createDefaultWorkflow(data.name);
    definition.description = data.description;
    definition.version = data.version;
    definition.trigger = data.trigger;
    definition.enabled = data.enabled;
    if (data.icon) definition.icon = data.icon;
    definition.projectId = data.projectId;
    if (data.inputSchema) definition.inputSchema = data.inputSchema;
    if (data.outputSchema) definition.outputSchema = data.outputSchema;

    try {
      const id = await createDefinition(definition);
      await handleCreated('workflow', id);
    } catch (err) {
      showToast(`创建工作流失败：${errorMessage(err)}`, 'error');
      throw err;
    }
  };

  const handleArchive = async (project: Project) => {
    const confirmed = await confirmDialog({
      title: `归档项目「${project.name}」`,
      message: '项目归档后不再接收新的会话、圆桌和工作流；已有工作仍保留归属，可随时恢复项目。',
      confirmText: '归档',
    });
    if (!confirmed) return;
    setArchivingId(project.id);
    try {
      await archiveProject(project.id);
      showToast(`已归档项目「${project.name}」`, 'success');
    } catch (err) {
      showToast(`归档失败：${errorMessage(err)}`, 'error');
    } finally {
      setArchivingId(null);
    }
  };

  const handleRestore = async (project: Project) => {
    setArchivingId(project.id);
    try {
      await restoreProject(project.id);
      setScope('active');
      setSelectedId(project.id);
      showToast(`已恢复项目「${project.name}」`, 'success');
    } catch (err) {
      showToast(`恢复失败：${errorMessage(err)}`, 'error');
    } finally {
      setArchivingId(null);
    }
  };

  const openDirectory = async (path: string) => {
    try {
      await invoke('open_path', { path });
    } catch (err) {
      showToast(`打开目录失败：${errorMessage(err)}`, 'error');
    }
  };

  return (
    <WorkbenchAssetPage contextLabel="项目" activeAsset="projects" frameless>
      <div className="projects-page h-full flex flex-col gap-2 overflow-hidden px-2 pb-2">
        <header
          className="enclave-surface projects-page-head shrink-0"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <div className="projects-scope-switch shrink-0" role="tablist" aria-label="项目范围">
            <button
              type="button"
              role="tab"
              aria-selected={scope === 'active'}
              data-active={scope === 'active'}
              onClick={() => setScope('active')}
            >
              <FolderKanban size={12} />
              进行中
              <span className="projects-scope-count">{activeProjects.length}</span>
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={scope === 'archived'}
              data-active={scope === 'archived'}
              onClick={() => setScope('archived')}
            >
              <Archive size={12} />
              已归档
              <span className="projects-scope-count">{archivedProjects.length}</span>
            </button>
          </div>

          <div className="flex-1" />

          <label className="projects-search">
            <Search size={12} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索项目或目录"
              aria-label="搜索项目或目录"
            />
            {query && (
              <button type="button" onClick={() => setQuery('')} title="清除搜索" aria-label="清除搜索">
                <X size={11} />
              </button>
            )}
          </label>

          <button
            type="button"
            className="projects-new-button"
            onClick={() => setEditor({ mode: 'create' })}
          >
            <Plus size={12} />
            新建项目
          </button>
        </header>

        <div className="projects-page-body flex-1 min-h-0 flex gap-2">
          <aside className="projects-list-pane shrink-0 flex flex-col overflow-hidden rounded-lg">
            <div className="projects-list-heading">
              <span>{scope === 'active' ? '项目' : '归档项目'}</span>
              <span>{visibleProjects.length}</span>
            </div>

            <div className="flex-1 min-h-0 overflow-y-auto pd-scroll-stable">
              {loading && projects.length === 0 ? (
                <div className="projects-list-state">
                  <Loader2 size={16} className="animate-spin" />
                  <span>加载项目</span>
                </div>
              ) : visibleProjects.length === 0 ? (
                <div className="projects-list-state projects-list-state-empty">
                  <FolderKanban size={20} />
                  <strong>{query ? '没有匹配的项目' : scope === 'active' ? '还没有项目' : '没有归档项目'}</strong>
                  <span>
                    {query
                      ? '换个关键词，或搜索项目目录。'
                      : scope === 'active'
                        ? '项目把会话、圆桌和工作流收在一起。'
                        : '归档项目会保留全部工作归属。'}
                  </span>
                  {!query && scope === 'active' && (
                    <button type="button" onClick={() => setEditor({ mode: 'create' })}>
                      <Plus size={11} />
                      新建项目
                    </button>
                  )}
                </div>
              ) : (
                <div className="projects-list">
                  {visibleProjects.map((project) => {
                    const active = selected?.id === project.id;
                    const workCount = project.sessionCount + project.roomCount + project.workflowCount;
                    return (
                      <button
                        key={project.id}
                        type="button"
                        className="projects-list-item"
                        data-active={active || undefined}
                        onClick={() => setSelectedId(project.id)}
                      >
                        <span className="projects-list-icon">
                          <span aria-hidden="true">{projectIcon(project)}</span>
                        </span>
                        <span className="projects-list-copy">
                          <strong>{project.name}</strong>
                          <span>{project.description || '未填写项目说明'}</span>
                          <small>
                            {workCount} 项工作
                            <span>·</span>
                            {formatActivity(project.lastActivityAt)}
                          </small>
                        </span>
                        <ArrowRight size={11} className="projects-list-arrow" />
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </aside>

          <main className="projects-detail-pane flex-1 min-w-0 flex flex-col overflow-hidden rounded-lg">
            {selected ? (
              <>
                <div className="projects-detail-head shrink-0">
                  <div className="projects-detail-title">
                    <span className="projects-detail-icon">
                      <span aria-hidden="true">{projectIcon(selected)}</span>
                    </span>
                    <div className="min-w-0">
                      <div className="flex items-center gap-2 min-w-0">
                        <h1>{selected.name}</h1>
                        {selected.archivedAt && <span className="projects-state-badge">已归档</span>}
                      </div>
                      <p>{selected.description || '未填写项目说明'}</p>
                    </div>
                  </div>

                  <PageToolbar
                    collapseAll
                    actions={[
                      {
                        key: 'edit',
                        label: '编辑项目',
                        icon: <Pencil size={11} />,
                        priority: 2,
                        onClick: () => setEditor({ mode: 'edit', project: selected, section: 'details' }),
                      },
                      {
                        key: 'archive',
                        label: selected.archivedAt ? '恢复项目' : '归档项目',
                        icon: selected.archivedAt ? <ArchiveRestore size={11} /> : <Archive size={11} />,
                        priority: 2,
                        onClick: () => selected.archivedAt
                          ? void handleRestore(selected)
                          : void handleArchive(selected),
                      },
                    ] satisfies ToolbarAction[]}
                  />
                </div>

                <div className="projects-detail-scroll flex-1 min-h-0 overflow-y-auto pd-scroll-stable">
                  <section className="projects-overview">
                    <div className="projects-section-label">工作概览</div>
                    <div className="projects-stat-grid">
                      <ProjectStat
                        icon={<MessageSquare size={14} />}
                        value={selected.sessionCount}
                        label="会话"
                        active={workFilter === 'session'}
                        onClick={() => setWorkFilter((value) => value === 'session' ? 'all' : 'session')}
                        createLabel="新建会话"
                        createDisabled={Boolean(selected.archivedAt)}
                        onCreate={() => launchCreation('session', selected)}
                      />
                      <ProjectStat
                        icon={<Users size={14} />}
                        value={selected.roomCount}
                        label="圆桌"
                        active={workFilter === 'groupchat'}
                        onClick={() => setWorkFilter((value) => value === 'groupchat' ? 'all' : 'groupchat')}
                        createLabel="新建圆桌"
                        createDisabled={Boolean(selected.archivedAt)}
                        onCreate={() => launchCreation('groupchat', selected)}
                      />
                      <ProjectStat
                        icon={<Workflow size={14} />}
                        value={selected.workflowCount}
                        label="工作流"
                        active={workFilter === 'workflow'}
                        onClick={() => setWorkFilter((value) => value === 'workflow' ? 'all' : 'workflow')}
                        createLabel="新建工作流"
                        createDisabled={Boolean(selected.archivedAt)}
                        onCreate={() => launchCreation('workflow', selected)}
                      />
                    </div>
                    {selected.archivedAt && (
                      <div className="projects-archived-note">
                        <Archive size={14} />
                        恢复项目后可继续创建会话、圆桌和工作流。
                      </div>
                    )}
                    <div className="projects-activity-line">
                      <span>最近活动</span>
                      <strong>{formatActivity(selected.lastActivityAt)}</strong>
                    </div>
                  </section>

                  <section className="projects-section">
                    <div className="projects-section-head">
                      <div>
                        <div className="projects-section-label">项目工作</div>
                        <p>直接打开归入本项目的会话、圆桌和工作流；工作数量随实际归属实时统计。</p>
                      </div>
                      {workItems.length > 0 && (
                        <div className="projects-work-total">
                          {workFilter === 'all'
                            ? `${workItems.length} 项`
                            : `${WORK_KIND_LABEL[workFilter]} · ${visibleWorkItems.length} 项`}
                        </div>
                      )}
                    </div>
                    {workItemsLoading ? (
                      <div className="projects-work-state">
                        <Loader2 size={14} className="animate-spin" />
                        加载项目工作
                      </div>
                    ) : visibleWorkItems.length === 0 ? (
                      <div className="projects-work-state">
                        <FolderKanban size={15} />
                        <span>
                          {workFilter === 'all'
                            ? '这个项目还没有会话、圆桌或工作流。'
                            : `这个项目还没有${WORK_KIND_LABEL[workFilter]}。`}
                        </span>
                        {workFilter !== 'all' && (
                          <button type="button" onClick={() => setWorkFilter('all')}>显示全部工作</button>
                        )}
                      </div>
                    ) : (
                      <div className="projects-work-list">
                        {visibleWorkItems.map((item) => (
                          <div
                            key={`${item.kind}:${item.id}`}
                            className="projects-work-item"
                          >
                            <button
                              type="button"
                              className="projects-work-open"
                              onClick={() => void openWorkItem(item)}
                            >
                              <span className="projects-work-kind-icon" data-kind={item.kind}>
                                {WORK_KIND_ICON[item.kind]}
                              </span>
                              <span className="projects-work-copy">
                                <strong>{item.title || `未命名${WORK_KIND_LABEL[item.kind]}`}</strong>
                                <small>{item.subtitle || '暂无补充信息'}</small>
                              </span>
                              <span className="projects-work-meta">
                                <small>{WORK_KIND_LABEL[item.kind]} · {workStatusLabel(item)}</small>
                                <span>{formatActivity(item.updatedAt)}</span>
                              </span>
                              <ArrowRight size={12} />
                            </button>
                            <button
                              type="button"
                              className="projects-work-membership-action"
                              title="调整项目归属"
                              aria-label={`调整「${item.title || WORK_KIND_LABEL[item.kind]}」的项目归属`}
                              onClick={() => setMembershipItem(item)}
                            >
                              <FolderKanban size={12} />
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                  </section>

                  <section className="projects-section">
                    <div className="projects-section-head">
                      <div>
                        <div className="projects-section-label">关联目录</div>
                        <p>主目录是项目默认工作根；关联目录与主目录共同进入安全路径。跟随项目的工作会使用最新主目录，产物目录也从当前工作根派生。</p>
                      </div>
                      <button
                        type="button"
                        className="projects-section-action"
                        onClick={() => setEditor({ mode: 'edit', project: selected, section: 'directories' })}
                      >
                        <FolderPlus size={11} />
                        管理目录
                      </button>
                    </div>
                    <div className="projects-directory-list">
                      {selected.directories.map((directory) => (
                        <div key={directory.id} className="projects-directory-row">
                          <span className="projects-directory-icon">
                            <Folder size={14} />
                          </span>
                          <span className="projects-directory-copy">
                            <strong>{pathBasename(directory.path)}</strong>
                            <span title={directory.path}>{directory.path}</span>
                          </span>
                          {directory.isPrimary && (
                            <span className="projects-primary-badge">
                              <Check size={9} />
                              主目录
                            </span>
                          )}
                          <button
                            type="button"
                            className="pd-btn p-1 rounded"
                            title="在文件管理器中打开"
                            aria-label={`打开目录 ${directory.path}`}
                            onClick={() => void openDirectory(directory.path)}
                          >
                            <FolderOpen size={13} />
                          </button>
                        </div>
                      ))}
                    </div>
                  </section>

                  {archivingId === selected.id && (
                    <div className="projects-busy-line">
                      <Loader2 size={12} className="animate-spin" />
                      正在更新项目状态
                    </div>
                  )}
                </div>
              </>
            ) : (
              <div className="projects-detail-empty">
                <span><FolderKanban size={26} /></span>
                <h2>{loading ? '正在载入项目' : '选择一个项目'}</h2>
                <p>在一个项目里集中查看相关会话、圆桌、工作流与常用目录。</p>
              </div>
            )}

            {error && (
              <div className="projects-error" role="alert">
                {error}
                <button type="button" onClick={() => void loadProjects()}>重试</button>
              </div>
            )}
          </main>
        </div>
      </div>

      {editor && (
        <ProjectEditorDialog
          mode={editor.mode}
          project={editor.project}
          section={editor.section}
          onClose={() => setEditor(null)}
          onSaved={(project) => {
            setScope(project.archivedAt ? 'archived' : 'active');
            setSelectedId(project.id);
            setEditor(null);
          }}
        />
      )}
      {membershipItem && (
        <ProjectMembershipDialog
          workType={membershipItem.kind}
          workId={membershipItem.id}
          workTitle={membershipItem.title}
          currentProjectId={selected?.id}
          onClose={() => setMembershipItem(null)}
          onChanged={() => reloadSelectedWork()}
        />
      )}
      {creation?.kind === 'session' && (
        <NewSessionDialog
          projectSeed={{
            kind: 'session',
            projectId: creation.project.id,
            projectName: creation.project.name,
            primaryDirectory: projectPrimaryDirectory(creation.project),
          }}
          onClose={() => setCreation(null)}
          onCreated={(id) => handleCreated('session', id)}
        />
      )}
      {creation?.kind === 'groupchat' && (
        <CreateRoomModal
          projectSeed={{
            kind: 'groupchat',
            projectId: creation.project.id,
            projectName: creation.project.name,
            primaryDirectory: projectPrimaryDirectory(creation.project),
          }}
          onClose={() => setCreation(null)}
          onCreated={(id) => handleCreated('groupchat', id)}
        />
      )}
      {creation?.kind === 'workflow' && (
        <WorkflowPropertyDialog
          mode="create"
          initial={{
            name: `${creation.project.name} · 工作流`,
            description: `由项目「${creation.project.name}」创建`,
            version: '1.0.0',
            trigger: { triggerType: 'manual' },
            enabled: true,
            projectId: creation.project.id,
          }}
          onClose={() => setCreation(null)}
          onConfirm={handleWorkflowPropertyConfirm}
        />
      )}
    </WorkbenchAssetPage>
  );
}

function ProjectStat({
  icon,
  value,
  label,
  active,
  onClick,
  onCreate,
  createLabel,
  createDisabled,
  creating,
}: {
  icon: ReactNode;
  value: number;
  label: string;
  active?: boolean;
  onClick?: () => void;
  onCreate?: () => void;
  createLabel?: string;
  createDisabled?: boolean;
  creating?: boolean;
}) {
  return (
    <div
      className="projects-stat"
      data-active={active || undefined}
    >
      <button
        type="button"
        className="projects-stat-filter"
        aria-pressed={active}
        onClick={onClick}
      >
        <span>{icon}</span>
        <strong>{value}</strong>
        <small>{label}</small>
      </button>
      {onCreate && (
        <button
          type="button"
          className="projects-stat-create"
          title={createDisabled ? '恢复项目后可继续新建' : createLabel}
          aria-label={createLabel}
          disabled={createDisabled || creating}
          onClick={onCreate}
        >
          {creating ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
        </button>
      )}
    </div>
  );
}

interface DirectoryDraft extends ProjectDirectoryInput {
  key: string;
}

function createDirectoryDraft(path = '', isPrimary = false): DirectoryDraft {
  return {
    key: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    path,
    isPrimary,
  };
}

function ProjectEditorDialog({
  mode,
  project,
  section,
  onClose,
  onSaved,
}: {
  mode: 'create' | 'edit';
  project?: Project;
  section?: 'details' | 'directories';
  onClose: () => void;
  onSaved: (project: Project) => void;
}) {
  const createProject = useProjectStore((state) => state.createProject);
  const updateProject = useProjectStore((state) => state.updateProject);
  const [name, setName] = useState(project?.name ?? '');
  const [description, setDescription] = useState(project?.description ?? '');
  const [icon, setIcon] = useState(project?.icon || '📁');
  const [directories, setDirectories] = useState<DirectoryDraft[]>(() => {
    if (project?.directories.length) {
      return project.directories.map((directory) => createDirectoryDraft(
        directory.path,
        directory.isPrimary,
      ));
    }
    return [createDirectoryDraft('', true)];
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const showDetails = mode === 'create' || section !== 'directories';
  const showDirectories = mode === 'create' || section !== 'details';
  const dialogKicker = mode === 'create'
    ? '新建'
    : section === 'directories'
      ? '目录设置'
      : '项目设置';
  const dialogTitle = mode === 'create'
    ? '创建项目'
    : section === 'directories'
      ? '管理项目目录'
      : `编辑「${project?.name}」`;

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !saving) onClose();
    };
    document.addEventListener('keydown', closeOnEscape);
    return () => document.removeEventListener('keydown', closeOnEscape);
  }, [onClose, saving]);

  const updateDirectory = (key: string, patch: Partial<DirectoryDraft>) => {
    setDirectories((rows) => rows.map((row) => row.key === key ? { ...row, ...patch } : row));
  };

  const chooseDirectory = async (key: string) => {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: '选择项目目录',
      });
      if (selected && typeof selected === 'string') {
        updateDirectory(key, { path: selected });
      }
    } catch (err) {
      setError(`选择目录失败：${errorMessage(err)}`);
    }
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    const normalized = directories
      .map((directory) => ({ path: directory.path.trim(), isPrimary: directory.isPrimary }))
      .filter((directory) => directory.path);
    if (showDirectories && !normalized.length) {
      setError('至少添加一个项目目录');
      return;
    }
    if (!normalized.some((directory) => directory.isPrimary)) {
      normalized[0].isPrimary = true;
    }

    setSaving(true);
    try {
      const input = { name: name.trim(), description: description.trim(), icon, directories: normalized };
      const saved = mode === 'create'
        ? await createProject(input)
        : await updateProject(project!.id, input);
      showToast(mode === 'create' ? `已创建项目「${saved.name}」` : `已保存项目「${saved.name}」`, 'success');
      onSaved(saved);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  return createPortal(
    <div className="projects-dialog-backdrop" role="presentation">
      <form className="projects-dialog" onSubmit={submit}>
        <div className="projects-dialog-head">
          <div>
            <span className="projects-dialog-kicker">{dialogKicker}</span>
            <h2>{dialogTitle}</h2>
          </div>
          <button type="button" onClick={onClose} disabled={saving} aria-label="关闭">
            <X size={15} />
          </button>
        </div>

        <div className="projects-dialog-body pd-scroll-stable">
          {showDetails && (
            <>
              <label className="projects-field">
                <span>项目名称</span>
                <input
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="例如：PilotDesk 桌面端"
                  maxLength={80}
                  autoFocus
                  required
                />
              </label>

              <label className="projects-field">
                <span>项目说明 <small>可选</small></span>
                <textarea
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  placeholder="这个项目负责什么、当前目标或边界。"
                  rows={3}
                  maxLength={2000}
                />
              </label>

              <div className="projects-icon-field">
                <div className="projects-field-title">
                  <div>
                    <span>项目图标</span>
                    <small>用于项目列表、工作徽标和门户入口识别。</small>
                  </div>
                  <span className="projects-icon-preview" aria-hidden="true">{icon}</span>
                </div>
                <div className="projects-icon-grid">
                  {PROJECT_EMOJI_GROUPS.map((group) => (
                    <div key={group.label} className="projects-icon-group">
                      <span>{group.label}</span>
                      <div>
                        {group.emojis.map((emoji) => (
                          <button
                            key={emoji}
                            type="button"
                            className="projects-icon-option"
                            data-active={icon === emoji || undefined}
                            aria-label={`选择图标 ${emoji}`}
                            aria-pressed={icon === emoji}
                            onClick={() => setIcon(emoji)}
                          >
                            {emoji}
                          </button>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}

          {showDirectories && (
            <div className="projects-directory-editor">
              <div className="projects-field-title">
                <div>
                  <span>项目目录</span>
                  <small>一个主目录，其余作为关联目录。</small>
                </div>
                <button
                  type="button"
                  className="projects-directory-add-button"
                  onClick={() => setDirectories((rows) => [
                    ...rows,
                    createDirectoryDraft('', rows.length === 0),
                  ])}
                >
                  <Plus size={11} />
                  添加目录
                </button>
              </div>

              <div className="projects-directory-editor-list">
                {directories.map((directory) => (
                  <div key={directory.key} className="projects-directory-editor-row">
                    <button
                      type="button"
                      className="projects-primary-picker"
                      data-active={directory.isPrimary}
                      aria-label={directory.isPrimary ? '当前主目录' : '设为主目录'}
                      title={directory.isPrimary ? '主目录' : '设为主目录'}
                      onClick={() => setDirectories((rows) => rows.map((row) => ({
                        ...row,
                        isPrimary: row.key === directory.key,
                      })))}
                    >
                      <span>{directory.isPrimary && <Check size={9} />}</span>
                    </button>
                    <input
                      value={directory.path}
                      onChange={(event) => updateDirectory(directory.key, { path: event.target.value })}
                      placeholder="输入目录路径，或从右侧选择"
                      aria-label="项目目录路径"
                    />
                    <button
                      type="button"
                      className="projects-directory-row-action"
                      title="选择目录"
                      aria-label="选择目录"
                      onClick={() => void chooseDirectory(directory.key)}
                    >
                      <FolderOpen size={13} />
                    </button>
                    <button
                      type="button"
                      className="projects-directory-row-action is-remove"
                      title="移除目录"
                      aria-label="移除目录"
                      disabled={directories.length <= 1}
                      onClick={() => {
                        setDirectories((rows) => {
                          const removed = rows.find((row) => row.key === directory.key);
                          const next = rows.filter((row) => row.key !== directory.key);
                          if (removed?.isPrimary && next.length) next[0] = { ...next[0], isPrimary: true };
                          return next;
                        });
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </div>
                ))}
              </div>
              <p className="projects-directory-hint">
                主目录是项目内会话、圆桌与工作流的默认工作根；关联目录与主目录统一进入安全路径，产物写入各工作对应的产物目录，均从当前工作根派生。
              </p>
            </div>
          )}

          {error && <div className="projects-dialog-error" role="alert">{error}</div>}
        </div>

        <div className="projects-dialog-footer">
          <button
            type="button"
            className="projects-dialog-button is-secondary"
            onClick={onClose}
            disabled={saving}
          >
            取消
          </button>
          <button
            type="submit"
            className="projects-dialog-button is-primary"
            disabled={saving || !name.trim()}
          >
            {saving && <Loader2 size={12} className="animate-spin" />}
            {mode === 'create'
              ? '创建项目'
              : section === 'directories'
                ? '保存目录'
                : '保存设置'}
          </button>
        </div>
      </form>
    </div>,
    document.body,
  );
}
