import { useState, useEffect, useCallback } from 'react';
import { Cpu, Search, FolderOpen, Bot, ChevronDown, ChevronRight, FileText, Trash2, FileArchive } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { useSkillStore, type SkillInfo } from '../../stores/skillStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { SkillEntryEditor } from './SkillEntryEditor';

/**
 * SkillBrowser — 全部技能管理（按 agent 分组）。
 *
 * 归属：**设置 › 技能管理**。技能目录配置（路径/入口文件/展示模式）在 Agent 集成配置里；
 * 会话中"取用技能"由输入框的技能选择器（Ctrl+K）承担，这里只做浏览 + 安装/卸载/改主文件。
 *
 * 目录归属提醒：技能落在**该 Agent 自己的技能目录**里（可能是 CLI Agent 的全局目录，
 * 如 ~/.claude/skills）—— 卸载/改主文件会直接影响该 Agent，故卸载前二次确认并显示路径。
 */
export function SkillBrowser() {
  const { skillsByAgent, isLoading, setAgentSkills, setLoading } = useSkillStore();
  const { agents, getDisplayName, getTheme } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedSkill, setSelectedSkill] = useState<{ agent: string; name: string; description: string; category?: string } | null>(null);
  const [collapsedAgents, setCollapsedAgents] = useState<Set<string>>(new Set());

  const toggleAgent = (agent: string) => {
    setCollapsedAgents((prev) => {
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
        // Derive installed agents from detect_env (version non-null = installed)
        const agentVersions = envInfo?.agentVersions ?? {};
        const installed = Object.entries(agentVersions)
          .filter(([, version]) => version !== null && version !== undefined)
          .map(([agentType]) => agentType);
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
    <div className="flex flex-col">
      {/* Header —— 左右内边距交给外层（设置页内容区），这样标题/搜索框与列表、与其它设置页左右对齐 */}
      <div className="pb-3 mb-1" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
          全部技能
        </h3>
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索技能..."
            className="search-input"
          />
        </div>
      </div>

      {/* Skills list by agent —— 不设内部滚动：交给设置页整页滚动 */}
      <div>
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : !hasAny ? (
          <div className="flex flex-col items-center justify-center py-8 gap-2">
            <FolderOpen size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配技能' : '暂无已安装技能'}
            </span>
          </div>
        ) : (
          allAgentTypes.map((agt) => {
            const filtered = getFiltered(agt);
            return (
              <div key={agt}>
                {/* Agent group header — click to toggle
                    这是带背景色的横条（不是普通行），保留自身左右内边距，避免文字贴边 */}
                <div
                  className="flex items-center gap-2 px-3 py-2 cursor-pointer select-none"
                  style={{ backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
                  onClick={() => toggleAgent(agt)}
                >
                  <Bot size={14} style={{ color: getTheme(agt).color }} />
                  <span className="text-xs font-medium uppercase tracking-wide" style={{ color: 'var(--text-secondary)' }}>
                    {getDisplayName(agt)}
                  </span>
                  <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                    ({filtered.length})
                  </span>
                  {/* 安装到「该 Agent 自己的技能目录」：目录或 .zip 包两种来源 */}
                  <div className="ml-auto shrink-0 flex items-center gap-0.5">
                    <button
                      onClick={(e) => { e.stopPropagation(); void handleInstall(agt, 'dir'); }}
                      disabled={busy}
                      className="p-1 rounded transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-tertiary)', cursor: busy ? 'not-allowed' : 'pointer' }}
                      title={`从本地目录安装技能到 ${getDisplayName(agt)} 的技能目录`}
                    >
                      <FolderOpen size={13} />
                    </button>
                    <button
                      onClick={(e) => { e.stopPropagation(); void handleInstall(agt, 'zip'); }}
                      disabled={busy}
                      className="p-1 rounded transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-tertiary)', cursor: busy ? 'not-allowed' : 'pointer' }}
                      title={`从 .zip 包安装技能到 ${getDisplayName(agt)} 的技能目录`}
                    >
                      <FileArchive size={13} />
                    </button>
                    <span className="flex items-center" style={{ color: 'var(--text-tertiary)' }}>
                      {collapsedAgents.has(agt) ? <ChevronRight size={16} /> : <ChevronDown size={16} />}
                    </span>
                  </div>
                </div>

                {/* Skills */}
                {!collapsedAgents.has(agt) && (
                  filtered.length > 0 ? filtered.map((skill) => {
                    const isSelected = selectedSkill?.agent === agt && selectedSkill?.name === skill.name;
                    const noPath = !skill.entryPath;
                    return (
                      <div
                        key={`${agt}:${skill.name}`}
                        className="flex items-center transition-colors"
                        style={{
                          backgroundColor: isSelected ? 'var(--bg-tertiary)' : 'transparent',
                          borderBottom: '1px solid var(--border)',
                        }}
                      >
                        <button
                          onClick={() => setSelectedSkill({ agent: agt, ...skill })}
                          className="flex-1 min-w-0 flex items-center gap-2 py-2.5 text-left transition-colors hover:bg-[var(--bg-tertiary)]"
                        >
                          <div className="flex-1 min-w-0">
                            <div className="text-xs flex items-center gap-1.5" style={{ color: 'var(--text-primary)' }}>
                              <Cpu size={12} className="shrink-0" style={{ color: getTheme(agt).color }} />
                              <span className="truncate">{skill.name}</span>
                            </div>
                            <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                              {skill.description}
                            </div>
                          </div>
                        </button>
                        {/* 主文件编辑 / 卸载：都按落盘路径操作，取不到路径时禁用并说明原因 */}
                        <div className="flex items-center gap-0.5 shrink-0">
                          <button
                            onClick={() => {
                              if (skill.entryPath) setEditTarget({ agent: agt, skillName: skill.name, entryPath: skill.entryPath });
                            }}
                            disabled={busy || noPath}
                            className="p-1 rounded transition-colors hover:opacity-80"
                            style={{ color: 'var(--text-tertiary)', cursor: busy || noPath ? 'not-allowed' : 'pointer' }}
                            title={noPath ? '未取到技能主文件路径，无法编辑' : '编辑技能主文件（SKILL.md）'}
                          >
                            <FileText size={13} />
                          </button>
                          <button
                            onClick={() => void handleUninstall(agt, skill)}
                            disabled={busy || !skill.dirPath}
                            className="p-1 rounded transition-colors hover:opacity-80"
                            style={{ color: 'var(--text-tertiary)', cursor: busy || !skill.dirPath ? 'not-allowed' : 'pointer' }}
                            title={skill.dirPath ? '卸载技能（删除技能目录）' : '未取到技能目录，无法卸载'}
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
                      </div>
                    );
                  }) : (
                    <div className="py-2 text-[10px]" style={{ color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border)' }}>
                      暂无技能文件（可用右上角「目录 / 压缩包」按钮安装）
                    </div>
                  )
                )}
              </div>
            );
          })
        )}
      </div>

      {/* Skill detail */}
      {selectedSkill && (
        <div className="pt-3 mt-1" style={{ borderTop: '1px solid var(--border)' }}>
          <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            <span style={{ color: 'var(--text-tertiary)' }}>来源:</span> {getDisplayName(selectedSkill.agent)}
            {selectedSkill.category && (
              <span className="ml-2">
                <span style={{ color: 'var(--text-tertiary)' }}>分类:</span> {selectedSkill.category}
              </span>
            )}
          </div>
        </div>
      )}

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
  );
}
