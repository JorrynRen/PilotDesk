import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { FolderOpen, Key, MessageSquare, Terminal, X } from 'lucide-react';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { isAgentAvailable, isAgentInstalled, useEnvInfo } from '../../hooks/useEnvInfo';
import { useI18n } from '../../hooks/useI18n';
import { getApiKey, useApiProviderStore } from '../../stores/apiProviderStore';
import { useProjectStore } from '../../stores/projectStore';
import { useSessionStore } from '../../stores/sessionStore';
import { errorMessage } from '../../utils/errorMessage';
import { projectLinkedDirectories, projectPrimaryDirectory } from '../../utils/project';
import { showToast } from '../../utils/toast';
import type { ProjectCreateContext } from '../../types/project';
import { Select } from '../common/Select';
import { ProjectSelect } from '../project/ProjectPicker';
import {
  WorkCreateDialogShell,
  type WorkCreateProjectSummary,
} from '../project/WorkCreateDialogShell';

interface NewSessionDialogProps {
  projectSeed?: ProjectCreateContext | null;
  onClose: () => void;
  onCreated: (sessionId: string) => void | Promise<void>;
  onOpenHostSessions?: (agentType: string, cwd?: string, projectId?: string) => void;
}

function projectSeedTitle(projectName: string): string {
  return `${projectName} · 会话`;
}

export function NewSessionDialog({
  projectSeed,
  onClose,
  onCreated,
  onOpenHostSessions,
}: NewSessionDialogProps) {
  const { t } = useI18n();
  const projects = useProjectStore((state) => state.projects);
  const createSession = useSessionStore((state) => state.createSession);
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();
  const { getDisplayName, getEnabledAgentTypes, getTransport } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const { createAgentSession: wsCreateSession } = useAgentEvent();

  const [newSessionType, setNewSessionType] = useState('');
  const [selectedApiProvider, setSelectedApiProvider] = useState('');
  const [selectedApiModel, setSelectedApiModel] = useState('');
  const [customModel, setCustomModel] = useState('');
  const [useCustomModel, setUseCustomModel] = useState(false);
  const [customTitle, setCustomTitle] = useState(
    projectSeed ? projectSeedTitle(projectSeed.projectName) : '',
  );
  const [projectId, setProjectId] = useState<string | null>(projectSeed?.projectId ?? null);
  const [sessionTemperature, setSessionTemperature] = useState(0.7);
  const [sessionMaxTokens, setSessionMaxTokens] = useState<number | undefined>(undefined);
  const [creating, setCreating] = useState(false);

  const selectedProject = projects.find((project) => project.id === projectId);
  const projectSummary = useMemo<WorkCreateProjectSummary | null>(() => {
    if (selectedProject) {
      return {
        name: selectedProject.name,
        icon: selectedProject.icon,
        primaryDirectory: projectPrimaryDirectory(selectedProject),
        linkedDirectoryCount: projectLinkedDirectories(selectedProject).length,
      };
    }
    if (projectSeed && projectId === projectSeed.projectId) {
      return {
        name: projectSeed.projectName,
        icon: '📁',
        primaryDirectory: projectSeed.primaryDirectory,
      };
    }
    return null;
  }, [projectId, projectSeed, selectedProject]);

  const cliTypes = useMemo(
    () => getEnabledAgentTypes().filter((agentType) => {
      if (agentType === 'api') return false;
      if (getTransport(agentType) === 'acp') {
        return envInfo?.agentHealth?.[agentType]?.available !== false;
      }
      return isAgentInstalled(envInfo, agentType);
    }),
    [envInfo, getEnabledAgentTypes, getTransport],
  );

  const hasApi = apiProviders.length > 0;
  const sessionTypeUsable = newSessionType === 'api'
    ? hasApi
    : cliTypes.includes(newSessionType);
  if (!sessionTypeUsable) {
    const nextSessionType = hasApi ? 'api' : cliTypes[0] ?? '';
    if (nextSessionType !== newSessionType) setNewSessionType(nextSessionType);
  }

  const currentApiProvider = apiProviders.find(
    (provider) => provider.id === selectedApiProvider,
  ) ?? apiProviders[0];
  const effectiveModel = useCustomModel
    ? customModel.trim()
    : selectedApiModel || currentApiProvider?.models[0] || '';

  useEffect(() => {
    void fetchProviders();
  }, [fetchProviders]);

  const handleProjectChange = (nextProjectId: string) => {
    const nextProject = projects.find((project) => project.id === nextProjectId);
    const previousTitle = projectSummary ? projectSeedTitle(projectSummary.name) : '';
    setProjectId(nextProjectId || null);
    if (!customTitle.trim() || customTitle === previousTitle) {
      setCustomTitle(nextProject ? projectSeedTitle(nextProject.name) : '');
    }
  };

  const handleCreate = async () => {
    if (!newSessionType) {
      showToast('暂无可用会话方式', 'error');
      return;
    }

    setCreating(true);
    try {
      let sessionId: string;
      if (newSessionType === 'api') {
        if (!currentApiProvider) {
          showToast('请选择 API 提供商', 'error');
          return;
        }
        const apiKey = await getApiKey(currentApiProvider.id);
        if (!apiKey) {
          showToast('请先在「设置 - API 配置」中添加 API Key', 'error');
          return;
        }
        if (!effectiveModel) {
          showToast('请选择或输入模型名称', 'error');
          return;
        }
        const session = await createSession(
          'api',
          customTitle.trim() || null,
          currentApiProvider.id,
          effectiveModel,
          sessionTemperature,
          sessionMaxTokens,
          projectId ?? undefined,
        );
        sessionId = session.id;
      } else {
        const session = await createSession(
          newSessionType,
          customTitle.trim() || null,
          undefined,
          undefined,
          undefined,
          undefined,
          projectId ?? undefined,
        );
        sessionId = session.id;
        void wsCreateSession(session.id, newSessionType)
          .catch((err) => showToast(`启动 Agent 会话失败: ${errorMessage(err)}`, 'error'));
      }

      await onCreated(sessionId);
    } catch (err) {
      showToast(`创建会话失败: ${errorMessage(err)}`, 'error');
    } finally {
      setCreating(false);
    }
  };

  const hostImportAvailable = Boolean(onOpenHostSessions)
    && newSessionType !== 'api'
    && getTransport(newSessionType) === 'acp'
    && isAgentAvailable(envInfo, newSessionType);

  return (
    <WorkCreateDialogShell
      icon={<MessageSquare size={14} />}
      kicker="会话"
      title={t('sessionList.dialog.title', '新建会话')}
      project={projectSummary}
      submitting={creating}
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault();
        void handleCreate();
      }}
      submitLabel={t('sessionList.dialog.create', '创建')}
      submittingLabel={t('sessionList.dialog.creating', '创建中…')}
      footerLeading={hostImportAvailable ? (
        <button
          type="button"
          className="projects-dialog-button is-secondary"
          disabled={creating}
          onClick={() => {
            onClose();
            onOpenHostSessions?.(
              newSessionType,
              projectSummary?.primaryDirectory
                || projectSeed?.primaryDirectory
                || undefined,
              projectId ?? undefined,
            );
          }}
        >
          {t('sessionList.import.entry', '从宿主导入会话…')}
        </button>
      ) : undefined}
    >
      <label className="projects-field">
        <span>项目归属 <small>可选</small></span>
        <ProjectSelect value={projectId ?? ''} onChange={handleProjectChange} />
      </label>

      <div className="work-create-section">
        <div className="work-create-section-head">
          <strong>会话方式</strong>
          <span>选择本地 Agent 或直接使用 API 模型</span>
        </div>
        {cliTypes.length === 0 && !hasApi ? (
          <div className="work-create-empty">
            {t(
              'sessionList.dialog.noAgent',
              '暂无可用会话方式。请先在「设置」中配置 API 提供商或安装 CLI Agent。',
            )}
          </div>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-2">
              {cliTypes.length > 0 && (
                <SessionKindButton
                  active={newSessionType !== 'api'}
                  icon={<Terminal size={12} />}
                  label="CLI Agent"
                  onClick={() => setNewSessionType(
                    cliTypes.includes(newSessionType) ? newSessionType : cliTypes[0],
                  )}
                />
              )}
              {hasApi && (
                <SessionKindButton
                  active={newSessionType === 'api'}
                  icon={<Key size={12} />}
                  label="API Agent"
                  onClick={() => setNewSessionType('api')}
                />
              )}
            </div>
            {newSessionType !== 'api' && cliTypes.length > 0 && (
              <label className="projects-field">
                <span>{t('sessionList.dialog.cliLabel', '选择 CLI Agent')}</span>
                <Select
                  value={newSessionType}
                  onChange={setNewSessionType}
                  options={cliTypes.map((agentType) => ({
                    value: agentType,
                    label: getDisplayName(agentType),
                  }))}
                />
              </label>
            )}
            {newSessionType === 'api' && (
              !hasApi ? (
                <div className="work-create-empty">
                  {t(
                    'sessionList.dialog.noProvider',
                    '暂无配置的 API 提供商，请先在「设置 - API集成配置」中添加',
                  )}
                </div>
              ) : (
                <div className="projects-work-create-grid">
                  <label className="projects-field">
                    <span className="work-create-field-label">
                      {t('sessionList.dialog.providerLabel', 'API 提供商')}
                    </span>
                    <Select
                      value={currentApiProvider?.id ?? ''}
                      onChange={(providerId) => {
                        const provider = apiProviders.find((item) => item.id === providerId);
                        setSelectedApiProvider(providerId);
                        setSelectedApiModel(provider?.models[0] || '');
                        setUseCustomModel(false);
                        setCustomModel('');
                      }}
                      options={apiProviders.map((provider) => ({
                        value: provider.id,
                        label: `${provider.name}${provider.apiKeySet
                          ? ` ${t('sessionList.dialog.configured', '(已配置)')}`
                          : ` ${t('sessionList.dialog.notConfigured', '(未配置Key)')}`}`,
                      }))}
                    />
                  </label>

                  {currentApiProvider && (
                    <div className="projects-field">
                      <span className="work-create-field-label justify-between gap-2">
                        <span>{t('sessionList.dialog.modelLabel', '模型')}</span>
                        <button
                          type="button"
                          onClick={() => setUseCustomModel((value) => !value)}
                          className="flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] transition-colors"
                          style={{
                            color: useCustomModel ? 'var(--accent)' : 'var(--text-tertiary)',
                            backgroundColor: useCustomModel ? 'var(--bg-tertiary)' : 'transparent',
                          }}
                        >
                          {useCustomModel ? <X size={10} /> : null}
                          {t('sessionList.dialog.custom', '自定义')}
                        </button>
                      </span>
                      {useCustomModel ? (
                        <input
                          type="text"
                          value={customModel}
                          onChange={(event) => setCustomModel(event.target.value)}
                          placeholder={t(
                            'sessionList.dialog.modelPlaceholder',
                            '输入模型名称，如: gpt-4o-2024-08-06',
                          )}
                          autoFocus
                        />
                      ) : currentApiProvider.models.length > 0 ? (
                        <Select
                          value={effectiveModel}
                          onChange={setSelectedApiModel}
                          options={currentApiProvider.models.map((model) => ({
                            value: model,
                            label: model,
                          }))}
                        />
                      ) : (
                        <div className="work-create-empty">
                          {t(
                            'sessionList.dialog.noModel',
                            '该提供商暂无预定义模型，请使用"自定义"输入模型名称',
                          )}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )
            )}
          </>
        )}
      </div>

      <div className="work-create-section">
        <div className="work-create-section-head">
          <strong>会话信息</strong>
          <span>标题用于识别会话，工作目录由项目归属动态决定</span>
        </div>
        <label className="projects-field">
          <span>{t('sessionList.dialog.titleLabel', '会话标题')} <small>可选</small></span>
          <input
            type="text"
            value={customTitle}
            onChange={(event) => setCustomTitle(event.target.value)}
            placeholder={
              newSessionType === 'api'
                ? `API: ${currentApiProvider?.name || ''} - ${useCustomModel
                  ? customModel || t('sessionList.dialog.custom', '自定义')
                  : effectiveModel || t('sessionList.dialog.modelFallback', '模型')}`
                : t(
                  'sessionList.dialog.titlePlaceholder',
                  `${getDisplayName(newSessionType)} 新会话`,
                  { name: getDisplayName(newSessionType) },
                )
            }
          />
        </label>

        <div className="projects-field">
          <span>
            {t('sessionList.dialog.cwdLabel', '工作目录')}
            {' '}
            <small>{projectId ? '跟随项目主目录' : '使用全局工作空间'}</small>
          </span>
          <div
            className="projects-work-create-path items-center gap-2 px-3 py-2 text-xs"
            title={projectSummary?.primaryDirectory || undefined}
          >
            <FolderOpen size={14} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
            <span className="truncate" style={{ color: 'var(--text-secondary)' }}>
              {projectSummary?.primaryDirectory
                || (projectId ? '项目尚未设置主目录' : '由全局工作空间动态解析')}
            </span>
          </div>
          <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            项目目录变更后，后续运行会自动使用新主目录；主目录与关联目录统一进入安全路径。
          </span>
        </div>
      </div>

      {newSessionType === 'api' && hasApi && (
        <div className="work-create-section">
          <div className="work-create-section-head">
            <strong>生成参数</strong>
            <span>未设置最大 Token 时使用模型默认策略</span>
          </div>
          <div className="projects-work-create-grid">
            <label className="projects-field">
              <span>{t('sessionList.dialog.temperature', '温度')}</span>
              <input
                type="number"
                min={0}
                max={2}
                step={0.1}
                value={sessionTemperature}
                onChange={(event) => {
                  setSessionTemperature(Number.parseFloat(event.target.value) || 0.7);
                }}
              />
            </label>
            <label className="projects-field">
              <span>{t('sessionList.dialog.maxTokens', '最大 Token')} <small>可选</small></span>
              <input
                type="number"
                min={1}
                step={1}
                value={sessionMaxTokens ?? ''}
                onChange={(event) => {
                  setSessionMaxTokens(
                    event.target.value ? Number.parseInt(event.target.value, 10) : undefined,
                  );
                }}
                placeholder={t('sessionList.dialog.maxTokens.placeholder', '不限制')}
              />
            </label>
          </div>
        </div>
      )}
    </WorkCreateDialogShell>
  );
}

function SessionKindButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex-1 py-2 rounded-lg text-xs transition-colors flex items-center justify-center gap-1.5"
      style={{
        backgroundColor: active ? 'var(--accent-light)' : 'var(--bg-field)',
        color: active ? 'var(--accent)' : 'var(--text-secondary)',
        border: `1px solid ${active ? 'var(--accent)' : 'var(--border)'}`,
      }}
    >
      {icon}
      {label}
    </button>
  );
}
