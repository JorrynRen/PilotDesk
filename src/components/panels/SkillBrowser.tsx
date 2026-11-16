import { useState, useEffect, useCallback } from 'react';
import { Cpu, Search, FolderOpen, Bot, ChevronDown, ChevronRight, FileText, Trash2, FileArchive } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { useSkillStore, type SkillInfo } from '../../stores/skillStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo, isAgentInstalled } from '../../hooks/useEnvInfo';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { SkillEntryEditor } from './SkillEntryEditor';
import { SettingsButton, SettingsSection } from '../settings';

interface SkillBrowserProps {
  onConfigureSkills?: () => void;
}

/**
 * SkillBrowser — 全部技能管理（按 agent 分组）。
 *
 * 归属：**设置 › 技能管理**。技能目录配置（路径/入口文件/展示模式）在 Agent 集成配置里；
 * 会话中"取用技能"由输入框的技能选择器（Ctrl+K）承担，这里只做浏览 + 安装/卸载/改主文件。
 *
 * 目录归属提醒：技能落在**该 Agent 自己的技能目录**里（可能是 CLI Agent 的全局目录，
 * 如 ~/.claude/skills）—— 卸载/改主文件会直接影响该 Agent，故卸载前二次确认并显示路径。
 */
export function SkillBrowser({ onConfigureSkills }: SkillBrowserProps) {
  const { skillsByAgent, isLoading, setAgentSkills, setLoading } = useSkillStore();
  const { agents, getDisplayName, getTheme } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedSkill, setSelectedSkill] = useState<{ agent: string; name: string; description: string; category?: string } | null>(null);
  const [expandedAgents, setExpandedAgents] = useState<Set<string>>(new Set());

  const toggleAgent = (agent: string) => {
    setExpandedAgents((prev) => {
      const next = new Set(prev);
      if (next.has(agent)) {
        next.delete(agent);
      } else {
        next.add(agent);
      }
      return next;
    });
  };

  const { requestSkills } = useAgentEvent({
    onSkills: (agentType, skills) => {
      setAgentSkills(agentType, skills);
    },
  });

  /** 安装/卸载进行中（都会改磁盘，防重复点击） */
  const [busy, setBusy] = useState(false);
  /** 正在编辑主文件的技能（null = 关闭） */
  const [editTarget, setEditTarget] = useState<{ agent: string; skillName: string; entryPath: string } | null>(null);

  /** 安装技能：来源可以是技能目录或 .zip 包，装到该 Agent 自己的技能目录 */
  const handleInstall = useCallback(
    async (agentType: string, kind: 'dir' | 'zip') => {
      if (busy) return;
      const selected = await openDialog(
        kind === 'dir'
          ? { directory: true, multiple: false, title: '选择技能目录（需含 SKILL.md 与 frontmatter）' }
          : { multiple: false, filters: [{ name: '技能压缩包', extensions: ['zip'] }], title: '选择技能 .zip 包' },
      );
      if (!selected || typeof selected !== 'string') return;
      setBusy(true);
      try {
        const info = await invoke<SkillInfo>('skill_install', { agentType, sourcePath: selected });
        showToast(`已安装技能「${info.name}」`, 'success');
        await requestSkills(agentType);
      } catch (e) {
        showToast(`安装失败: ${errorMessage(e)}`, 'error');
      } finally {
        setBusy(false);
      }
    },
    [busy, requestSkills],
  );

  /** 卸载技能：删除该技能目录（破坏性，先二次确认并把路径摊开给用户看） */
  const handleUninstall = useCallback(
    async (agentType: string, skill: SkillInfo) => {
      if (busy || !skill.dirPath) return;
      const ok = await confirmDialog({
        title: '卸载技能',
        message:
          `确定卸载「${skill.name}」？该操作会删除技能目录，且不可撤销。\n\n` +
          `目录：${skill.dirPath}\n\n` +
          `该目录由 ${getDisplayName(agentType)} 直接读取，删除后它会失去这个技能。`,
        confirmText: '卸载',
      });
      if (!ok) return;
      setBusy(true);
      try {
        await invoke('skill_uninstall', { agentType, dirPath: skill.dirPath });
        showToast(`已卸载「${skill.name}」`, 'success');
        await requestSkills(agentType);
      } catch (e) {
        showToast(`卸载失败: ${errorMessage(e)}`, 'error');
      } finally {
        setBusy(false);
      }
    },
    [busy, requestSkills, getDisplayName],
  );

  // Agent display order from DB
  const agentOrder = agents
    .filter(a => a.isEnabled)
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .map(a => a.agentType);

  // Auto-fetch skills: use detect_env results to determine installed agents, then fetch skills
  useEffect(() => {
    const fetchAll = async () => {
      setLoading(true);
      try {
        // 已装 Agent 取自 detect_env 的 health（版本号取不到不代表未安装）
        const types = new Set([
          ...Object.keys(envInfo?.agentVersions ?? {}),
          ...Object.keys(envInfo?.agentHealth ?? {}),
        ]);
        const installed = [...types].filter((agentType) => isAgentInstalled(envInfo, agentType));
        // API Agent 独立于 DB/detect_env，需显式纳入技能请求
        const targets = Array.from(new Set([...installed, 'api']));
        for (const agent of targets) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      } catch {
        // Fallback: try enabled agents from DB（并补上 API Agent）
        const targets = Array.from(new Set([...agentOrder, 'api']));
        for (const agent of targets) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      }
      setLoading(false);
    };
    fetchAll();
  }, [envInfo]); // eslint-disable-line react-hooks/exhaustive-deps

  // 按 DB 排序聚合所有 agent 的技能
  const allAgentTypes = Object.keys(skillsByAgent).sort(
    (a, b) => agentOrder.indexOf(a) - agentOrder.indexOf(b)
  );
  const hasAny = allAgentTypes.length > 0;

  // 搜索过滤
  const getFiltered = (agentType: string) => {
    const skills = skillsByAgent[agentType] || [];
    if (!searchQuery) return skills;
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        s.description.toLowerCase().includes(searchQuery.toLowerCase())
    );
  };

  return (
    <SettingsSection
      title="全部技能"
      description="按 Agent 浏览本机技能。技能目录由各 Agent 独立读取，安装、编辑主文件或卸载会直接影响对应 Agent。"
      actions={
        onConfigureSkills ? (
          <SettingsButton onClick={onConfigureSkills} icon={<FolderOpen size={12} />}>
            配置技能读取目录
          </SettingsButton>
        ) : undefined
      }
    >
      <div className="flex flex-col">
        <div className="pd-settings-search">
          <Search
            size={12}
            aria-hidden
            className="pd-settings-search-icon"
          />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索技能..."
            className="pd-settings-input pd-settings-search-input w-full"
          />
        </div>

        {/* Skills list by agent —— 不设内部滚动：交给设置页整页滚动 */}
        <div className="pd-settings-skill-groups">
        {isLoading ? (
          <div className="pd-settings-empty">
            <FolderOpen size={20} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : !hasAny ? (
          <div className="pd-settings-empty">
            <FolderOpen size={20} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配技能' : '暂无已安装技能'}
            </span>
          </div>
        ) : (
          allAgentTypes.map((agt) => {
            const filtered = getFiltered(agt);
            return (
              <section
                key={agt}
                className="pd-settings-skill-group"
                data-collapsed={!expandedAgents.has(agt)}
              >
                {/* 一级：Agent 分组标题。整条可展开/收起，操作按钮浮于命中区之上。 */}
                <div className="pd-settings-skill-group-header">
                  <button
                    type="button"
                    className="pd-settings-skill-group-toggle"
                    onClick={() => toggleAgent(agt)}
                    aria-expanded={expandedAgents.has(agt)}
                    aria-label={`${expandedAgents.has(agt) ? '收起' : '展开'} ${getDisplayName(agt)} 的技能`}
                  />
                  <div className="pd-settings-skill-group-heading">
                    <Bot size={14} style={{ color: getTheme(agt).color }} />
                    <span className="pd-settings-skill-group-name">
                      {getDisplayName(agt)}
                    </span>
                    <span className="pd-settings-skill-group-count">
                      {filtered.length}
                    </span>
                  </div>
                  <div className="pd-settings-skill-group-actions">
                    <button
                      type="button"
                      onClick={(e) => { e.stopPropagation(); void handleInstall(agt, 'dir'); }}
                      disabled={busy}
                      className="pd-settings-skill-icon-button"
                      style={{ color: 'var(--text-tertiary)', cursor: busy ? 'not-allowed' : 'pointer' }}
                      title={`从本地目录安装技能到 ${getDisplayName(agt)} 的技能目录`}
                    >
                      <FolderOpen size={13} />
                    </button>
                    <button
                      type="button"
                      onClick={(e) => { e.stopPropagation(); void handleInstall(agt, 'zip'); }}
                      disabled={busy}
                      className="pd-settings-skill-icon-button"
                      style={{ color: 'var(--text-tertiary)', cursor: busy ? 'not-allowed' : 'pointer' }}
                      title={`从 .zip 包安装技能到 ${getDisplayName(agt)} 的技能目录`}
                    >
                      <FileArchive size={13} />
                    </button>
                    <span className="pd-settings-skill-chevron" aria-hidden>
                      {expandedAgents.has(agt) ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                    </span>
                  </div>
                </div>

                {/* 二级：技能条目缩进在 Agent 分组内，整行负责选中，操作按钮独立。 */}
                {expandedAgents.has(agt) && (
                  <div className="pd-settings-skill-group-body">
                    {filtered.length > 0 ? (
                      <div className="pd-settings-skill-list">
                        {filtered.map((skill) => {
                          const isSelected = selectedSkill?.agent === agt && selectedSkill?.name === skill.name;
                          const noPath = !skill.entryPath;
                          return (
                            <div
                              key={`${agt}:${skill.name}`}
                              className="pd-settings-skill-row"
                              data-selected={isSelected}
                            >
                              <button
                                type="button"
                                onClick={() => setSelectedSkill({ agent: agt, ...skill })}
                                className="pd-settings-skill-row-select"
                                aria-label={`选择技能 ${skill.name}`}
                                aria-pressed={isSelected}
                              />
                              <div className="pd-settings-skill-row-content">
                                <div className="pd-settings-skill-name">
                                  <Cpu size={12} className="shrink-0" style={{ color: getTheme(agt).color }} />
                                  <span className="truncate">{skill.name}</span>
                                </div>
                                <div className="pd-settings-skill-description pd-settings-note truncate">
                                  {skill.description}
                                </div>
                              </div>
                              {/* 主文件编辑 / 卸载：都按落盘路径操作，取不到路径时禁用并说明原因 */}
                              <div className="pd-settings-skill-row-actions">
                                <button
                                  type="button"
                                  onClick={() => {
                                    if (skill.entryPath) setEditTarget({ agent: agt, skillName: skill.name, entryPath: skill.entryPath });
                                  }}
                                  disabled={busy || noPath}
                                  className="pd-settings-skill-icon-button"
                                  style={{ color: 'var(--text-tertiary)', cursor: busy || noPath ? 'not-allowed' : 'pointer' }}
                                  title={noPath ? '未取到技能主文件路径，无法编辑' : '编辑技能主文件（SKILL.md）'}
                                >
                                  <FileText size={13} />
                                </button>
                                <button
                                  type="button"
                                  onClick={() => void handleUninstall(agt, skill)}
                                  disabled={busy || !skill.dirPath}
                                  className="pd-settings-skill-icon-button"
                                  style={{ color: 'var(--text-tertiary)', cursor: busy || !skill.dirPath ? 'not-allowed' : 'pointer' }}
                                  title={skill.dirPath ? '卸载技能（删除技能目录）' : '未取到技能目录，无法卸载'}
                                >
                                  <Trash2 size={13} />
                                </button>
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    ) : (
                      <div className="pd-settings-skill-empty">
                      暂无技能文件（可用右上角「目录 / 压缩包」按钮安装）
                      </div>
                    )}
                  </div>
                )}
              </section>
            );
          })
        )}
        </div>

        {/* 主文件编辑器：保存成功后刷新该 Agent 的技能列表（name/description 可能已改） */}
        {editTarget && (
          <SkillEntryEditor
            agentType={editTarget.agent}
            skillName={editTarget.skillName}
            entryPath={editTarget.entryPath}
            onClose={() => setEditTarget(null)}
            onSaved={() => {
              const agent = editTarget.agent;
              setEditTarget(null);
              void requestSkills(agent);
            }}
          />
        )}
      </div>
    </SettingsSection>
  );
}
