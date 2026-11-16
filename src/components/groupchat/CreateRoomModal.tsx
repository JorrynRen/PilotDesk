import { useEffect, useMemo, useState } from 'react';
import { FolderOpen, Plus, Trash2, Users } from 'lucide-react';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { useGroupChatStore } from '../../stores/groupChatStore';
import { useProjectStore } from '../../stores/projectStore';
import { errorMessage } from '../../utils/errorMessage';
import { projectLinkedDirectories, projectPrimaryDirectory } from '../../utils/project';
import { joinDisplayPath } from '../../utils/path';
import type {
  CreateGroupChatRoomInput,
  GroupChatParticipantInput,
} from '../../types/groupchat';
import type { ProjectCreateContext } from '../../types/project';
import { Select } from '../common/Select';
import { ProjectSelect } from '../project/ProjectPicker';
import {
  WorkCreateDialogShell,
  type WorkCreateProjectSummary,
} from '../project/WorkCreateDialogShell';

interface CreateRoomModalProps {
  projectSeed?: ProjectCreateContext | null;
  onClose: () => void;
  onCreated?: (roomId: string) => void | Promise<void>;
}

interface DraftParticipant {
  type: 'api' | 'cli';
  displayName: string;
  systemRole: string;
  provider: string;
  model: string;
  agentType: string;
}

function draftId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export function CreateRoomModal({
  projectSeed,
  onClose,
  onCreated,
}: CreateRoomModalProps) {
  const createRoom = useGroupChatStore((state) => state.createRoom);
  const loadRooms = useGroupChatStore((state) => state.loadRooms);
  const selectRoom = useGroupChatStore((state) => state.selectRoom);
  const { providers, fetchProviders } = useApiProviderStore();
  const { agents, getTransport } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const projects = useProjectStore((state) => state.projects);

  const [title, setTitle] = useState(
    projectSeed ? `${projectSeed.projectName} · 圆桌` : '',
  );
  const [topic, setTopic] = useState('');
  const [directorProvider, setDirectorProvider] = useState('');
  const [directorModel, setDirectorModel] = useState('');
  const [drafts, setDrafts] = useState<DraftParticipant[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [allowAutoCli, setAllowAutoCli] = useState(true);
  const [projectId, setProjectId] = useState(projectSeed?.projectId ?? '');
  const [maxParallel, setMaxParallel] = useState(2);

  const selectedProjectInfo = useMemo<WorkCreateProjectSummary | null>(() => {
    const project = projects.find((item) => item.id === projectId);
    if (project) {
      return {
        name: project.name,
        icon: project.icon,
        primaryDirectory: projectPrimaryDirectory(project),
        linkedDirectoryCount: projectLinkedDirectories(project).length,
      };
    }
    if (projectSeed && projectSeed.projectId === projectId) {
      return {
        name: projectSeed.projectName,
        icon: '📁',
        primaryDirectory: projectSeed.primaryDirectory,
      };
    }
    return null;
  }, [projectId, projectSeed, projects]);

  const handleProjectChange = (nextProjectId: string) => {
    const nextProject = projects.find((item) => item.id === nextProjectId);
    const previousDefaultTitle = selectedProjectInfo
      ? `${selectedProjectInfo.name} · 圆桌`
      : '';
    setProjectId(nextProjectId);
    if (!title.trim() || title === previousDefaultTitle) {
      setTitle(nextProject ? `${nextProject.name} · 圆桌` : '');
    }
  };

  useEffect(() => {
    void fetchProviders();
  }, [fetchProviders]);

  const cliAgents = useMemo(
    () => agents.filter((agent) => {
      if (agent.agentType === 'api' || !agent.isEnabled) return false;
      return getTransport(agent.agentType) !== 'acp'
        || envInfo?.agentHealth?.[agent.agentType]?.available !== false;
    }),
    [agents, envInfo, getTransport],
  );

  const modelsOf = (providerId: string) =>
    providers.find((provider) => provider.id === providerId)?.models ?? [];

  const effectiveDirectorProvider = directorProvider || providers[0]?.id || '';
  const effectiveDirectorModel = directorModel
    || modelsOf(effectiveDirectorProvider)[0]
    || '';
  const artifactBaseLabel = selectedProjectInfo?.primaryDirectory
    ? joinDisplayPath(selectedProjectInfo.primaryDirectory, 'outputs')
    : '全局工作空间 › outputs';

  const addDraft = (type: 'api' | 'cli') => {
    setDrafts((rows) => [
      {
        type,
        displayName: '',
        systemRole: '',
        provider: '',
        model: '',
        agentType: '',
      },
      ...rows,
    ]);
  };

  const updateDraft = (index: number, patch: Partial<DraftParticipant>) => {
    setDrafts((rows) => rows.map((row, rowIndex) => (
      rowIndex === index ? { ...row, ...patch } : row
    )));
  };

  const removeDraft = (index: number) => {
    setDrafts((rows) => rows.filter((_, rowIndex) => rowIndex !== index));
  };

  const submit = async () => {
    setError(null);
    if (!title.trim()) {
      setError('请填写圆桌标题');
      return;
    }
    if (!effectiveDirectorProvider || !effectiveDirectorModel) {
      setError('请选择 Director 的模型（提供商 + 模型）');
      return;
    }

    const participants: GroupChatParticipantInput[] = [{
      id: 'director',
      participantType: 'director',
      agentConfig: JSON.stringify({
        provider: effectiveDirectorProvider,
        model: effectiveDirectorModel,
      }),
      displayName: '主持人',
      systemRole: '协调/裁决',
    }];

    for (const draft of drafts) {
      if (!draft.displayName.trim()) {
        setError('参与者显示名不能为空');
        return;
      }
      if (draft.type === 'api') {
        if (!draft.provider || !draft.model) {
          setError(`参与者「${draft.displayName}」需选择提供商与模型`);
          return;
        }
        participants.push({
          id: draftId('api'),
          participantType: 'api',
          agentConfig: JSON.stringify({ provider: draft.provider, model: draft.model }),
          displayName: draft.displayName.trim(),
          systemRole: draft.systemRole.trim() || '参与者',
        });
      } else {
        if (!draft.agentType) {
          setError(`参与者「${draft.displayName}」需选择 CLI Agent 类型`);
          return;
        }
        participants.push({
          id: draftId('cli'),
          participantType: 'cli',
          agentConfig: JSON.stringify({ agent_type: draft.agentType }),
          displayName: draft.displayName.trim(),
          systemRole: draft.systemRole.trim() || '参与者',
        });
      }
    }

    participants.push({
      id: 'user',
      participantType: 'user',
      agentConfig: '{}',
      displayName: '我',
      systemRole: '用户',
    });

    const input: CreateGroupChatRoomInput = {
      title: title.trim(),
      topic: topic.trim(),
      participants,
      directorId: 'director',
      allowAutoCli: allowAutoCli ? 1 : 0,
      projectId: projectId || undefined,
      maxParallel,
    };

    try {
      setSubmitting(true);
      const room = await createRoom(input);
      await loadRooms();
      if (!onCreated) await selectRoom(room.id);
      onClose();
      await onCreated?.(room.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <WorkCreateDialogShell
      icon={<Users size={14} />}
      kicker="圆桌"
      title="新建圆桌"
      project={selectedProjectInfo}
      submitting={submitting}
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
      submitLabel="创建并进入"
      error={error}
    >
      <label className="projects-field">
        <span>项目归属 <small>可选</small></span>
        <ProjectSelect value={projectId} onChange={handleProjectChange} />
      </label>

      <div className="work-create-section">
        <div className="work-create-section-head">
          <strong>讨论设置</strong>
          <span>标题用于识别房间，议题可留空并在进入后补充</span>
        </div>
        <label className="projects-field">
          <span>圆桌标题</span>
          <input
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="例如：PilotDesk 架构评审"
            maxLength={120}
            autoFocus
          />
        </label>
        <label className="projects-field">
          <span>讨论议题 <small>可选</small></span>
          <textarea
            value={topic}
            onChange={(event) => setTopic(event.target.value)}
            placeholder="留空时以进入房间后的第一条指令作为目标"
            rows={2}
            maxLength={2000}
          />
        </label>
      </div>

      <div className="work-create-section">
        <div className="work-create-section-head">
          <strong>主持模型</strong>
          <span>负责协调讨论、裁决分歧与编排任务</span>
        </div>
        <div className="projects-work-create-grid">
          <label className="projects-field">
            <span>提供商</span>
            <Select
              value={effectiveDirectorProvider}
              onChange={(value) => {
                setDirectorProvider(value);
                setDirectorModel(modelsOf(value)[0] || '');
              }}
              placeholder={providers.length > 0 ? '选择提供商' : '暂无可用提供商'}
              disabled={providers.length === 0}
              options={providers.map((provider) => ({
                value: provider.id,
                label: provider.name,
              }))}
            />
          </label>
          <label className="projects-field">
            <span>模型</span>
            <Select
              value={effectiveDirectorModel}
              onChange={setDirectorModel}
              placeholder={effectiveDirectorProvider ? '选择模型' : '请先选择提供商'}
              disabled={!effectiveDirectorProvider || modelsOf(effectiveDirectorProvider).length === 0}
              options={modelsOf(effectiveDirectorProvider).map((model) => ({
                value: model,
                label: model,
              }))}
            />
          </label>
        </div>
      </div>

      <div className="work-create-section">
        <div className="work-create-section-head work-create-section-head--actions">
          <div>
            <strong>参与者</strong>
            <span>主持人、你和追加的 API / CLI Agent 共同参与</span>
          </div>
          <div className="work-create-participant-actions">
            <label
              className="work-create-inline-check"
              title="关闭后仅影响主持人自动补充，手动添加不受限制"
            >
              <input
                type="checkbox"
                checked={allowAutoCli}
                onChange={(event) => setAllowAutoCli(event.target.checked)}
              />
              <span>允许主持人自动补充 CLI Agent</span>
            </label>
            <button
              type="button"
              className="projects-directory-add-button"
              onClick={() => addDraft('api')}
            >
              <Plus size={11} /> API
            </button>
            <button
              type="button"
              className="projects-directory-add-button"
              onClick={() => addDraft('cli')}
            >
              <Plus size={11} /> CLI
            </button>
          </div>
        </div>

        {drafts.length === 0 ? (
          <div className="work-create-empty">
            暂未添加其他参与者。Director 与用户已默认加入。
          </div>
        ) : (
          <div className="work-create-participant-list">
            {drafts.map((draft, index) => (
              <div key={`${draft.type}-${index}`} className="work-create-participant">
                <div className="flex items-center gap-2">
                  <span className="work-create-kind">{draft.type.toUpperCase()}</span>
                  <input
                    className="min-w-0 flex-1"
                    placeholder="显示名（如 架构师）"
                    value={draft.displayName}
                    onChange={(event) => updateDraft(index, {
                      displayName: event.target.value,
                    })}
                  />
                  <button
                    type="button"
                    className="projects-directory-row-action is-remove"
                    onClick={() => removeDraft(index)}
                    aria-label="移除参与者"
                    title="移除参与者"
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
                <input
                  placeholder="角色（如 架构师 / 质疑者 / 执行者）"
                  value={draft.systemRole}
                  onChange={(event) => updateDraft(index, {
                    systemRole: event.target.value,
                  })}
                />
                {draft.type === 'api' ? (
                  <div className="projects-work-create-grid">
                    <Select
                      value={draft.provider}
                      onChange={(value) => updateDraft(index, {
                        provider: value,
                        model: modelsOf(value)[0] || '',
                      })}
                      placeholder="选择提供商"
                      options={providers.map((provider) => ({
                        value: provider.id,
                        label: provider.name,
                      }))}
                    />
                    <Select
                      value={draft.model}
                      onChange={(value) => updateDraft(index, { model: value })}
                      placeholder="选择模型"
                      disabled={!draft.provider}
                      options={modelsOf(draft.provider).map((model) => ({
                        value: model,
                        label: model,
                      }))}
                    />
                  </div>
                ) : (
                  <Select
                    value={draft.agentType}
                    onChange={(value) => updateDraft(index, { agentType: value })}
                    placeholder="选择 CLI Agent 类型"
                    options={cliAgents.map((agent) => ({
                      value: agent.agentType,
                      label: agent.displayName || agent.agentType,
                    }))}
                  />
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="work-create-section">
        <div className="work-create-section-head">
          <strong>执行与产物</strong>
          <span>并行上限只影响同层任务，讨论仍按顺序进行</span>
        </div>
        <div className="projects-field">
          <span className="flex items-center justify-between">
            <span>并行任务数</span>
            <strong className="text-[11px]" style={{ color: 'var(--text-primary)' }}>
              {maxParallel} 路
            </strong>
          </span>
          <div className="work-create-range">
            <span>1</span>
            <input
              type="range"
              min={1}
              max={4}
              step={1}
              value={maxParallel}
              onChange={(event) => setMaxParallel(Number(event.target.value))}
              style={{ accentColor: 'var(--accent)' }}
              aria-label="并行任务数"
            />
            <span>4</span>
          </div>
        </div>
        <div className="projects-field">
          <span>产物目录 <small>运行时自动派生，不单独保存</small></span>
          <div
            className="projects-work-create-path items-center gap-2 px-3 py-2 text-xs"
            title={selectedProjectInfo?.primaryDirectory ? artifactBaseLabel : undefined}
          >
            <FolderOpen size={14} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
            <span className="truncate" style={{ color: 'var(--text-secondary)' }}>
              {artifactBaseLabel}
            </span>
          </div>
        </div>
      </div>
    </WorkCreateDialogShell>
  );
}
