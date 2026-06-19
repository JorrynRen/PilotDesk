import { useState, useEffect, useCallback, useRef } from 'react';
import { Plus, Trash2, Pencil, Save, X, Loader2, Check, Palette, Download, Upload, Cpu, Search, FolderOpen, Bot, ChevronDown, ChevronRight } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import type { AgentConfig } from '../../types';
import { useSkillStore } from '../../stores/skillStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { AGENT_THEMES } from '../../types';
import type { SkillInfo } from '../../stores/skillStore';
import { SettingsSection, SettingsCard, SettingsButton } from '../settings';

// ──────────────────────────────────────────────
//  Agent Manager — Phase 4
// ──────────────────────────────────────────────

export function AgentManager() {
  const { agents, loading, fetchAgents, getTheme } = useAgentRegistry();
  const [editingType, setEditingType] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<Partial<AgentConfig> | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [addForm, setAddForm] = useState<Partial<AgentConfig>>({});
  const [saving, setSaving] = useState(false);
  const [marketAgents, setMarketAgents] = useState<AgentConfig[]>([]);
  const [marketLoading, setMarketLoading] = useState(false);
  const [showMarket, setShowMarket] = useState(false);

  // SkillBrowser state
  const { skillsByAgent, isLoading: skillsLoading, setAgentSkills, setLoading: setSkillsLoading } = useSkillStore();
  const { agents: allAgents, getDisplayName } = useAgentRegistry();
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

  // Agent display order from DB
  const agentOrder = allAgents
    .filter(a => a.isEnabled)
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .map(a => a.agentType);

  // Auto-fetch skills
  useEffect(() => {
    const fetchAll = async () => {
      setSkillsLoading(true);
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const installed = await invoke<string[]>('agent_detect_installed');
        for (const agent of installed) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      } catch {
        for (const agent of agentOrder) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      }
      setSkillsLoading(false);
    };
    fetchAll();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 按 DB 排序聚合所有 agent 的技能
  const allAgentTypes = Object.keys(skillsByAgent).sort(
    (a, b) => agentOrder.indexOf(a) - agentOrder.indexOf(b)
  );
  const hasAnySkills = allAgentTypes.length > 0;

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

  const handleEdit = (agent: AgentConfig) => {
    setEditingType(agent.agentType);
    setEditForm({ ...agent });
  };

  const handleSave = async () => {
    if (!editForm || !editingType) return;
    setSaving(true);
    try {
      await invoke('update_agent', {
        payload: {
          agentType: editingType,
          displayName: editForm.displayName,
          description: editForm.description,
          color: editForm.color,
          icon: editForm.icon,
          isEnabled: editForm.isEnabled,
          sortOrder: editForm.sortOrder,
          cliCommand: editForm.cliCommand,
          installCmd: editForm.installCmd,
          uninstallCmd: editForm.uninstallCmd,
          updateCmd: editForm.updateCmd,
          versionCmd: editForm.versionCmd,
          latestVersionCmd: editForm.latestVersionCmd,
          runCmdTemplate: editForm.runCmdTemplate,
          outputParser: editForm.outputParser,
          sessionIdSource: editForm.sessionIdSource,
          sessionIdField: editForm.sessionIdField,
          resumeArgTemplate: editForm.resumeArgTemplate,
          skillsDir: editForm.skillsDir,
          skillEntryFile: editForm.skillEntryFile,
          skillDisplayMode: editForm.skillDisplayMode,
        },
      });
      showToast('Agent 配置已更新', 'success');
      setEditingType(null);
      setEditForm(null);
      fetchAgents();
    } catch (err: any) {
      showToast(`更新失败: ${err}`, 'error');
    }
    setSaving(false);
  };

  const handleDelete = async (agentType: string) => {
    try {
      await invoke('delete_agent', { agentType });
      showToast('Agent 已删除', 'success');
      fetchAgents();
    } catch (err: any) {
      showToast(`删除失败: ${err}`, 'error');
    }
  };

  const handleAdd = async () => {
    if (!addForm.agentType || !addForm.displayName || !addForm.cliCommand) {
      showToast('请填写 Agent 类型、名称和 CLI 命令', 'error');
      return;
    }
    setSaving(true);
    try {
      await invoke('add_agent', { payload: addForm });
      showToast('Agent 已添加', 'success');
      setShowAddForm(false);
      setAddForm({});
      fetchAgents();
    } catch (err: any) {
      showToast(`添加失败: ${err}`, 'error');
    }
    setSaving(false);
  };

  const fetchMarket = async () => {
    setMarketLoading(true);
    try {
      const result = await invoke<AgentConfig[]>('list_agent_market');
      setMarketAgents(result);
    } catch (err: any) {
      showToast(`获取 Agent 市场失败: ${err}`, 'error');
    }
    setMarketLoading(false);
  };

  const handleInstallFromMarket = async (agent: AgentConfig) => {
    setSaving(true);
    try {
      await invoke('add_agent', { payload: agent });
      showToast(`已安装 ${agent.displayName}`, 'success');
      fetchAgents();
    } catch (err: any) {
      // Already exists, try update
      try {
        await invoke('update_agent', {
          payload: {
            agentType: agent.agentType,
            displayName: agent.displayName,
            description: agent.description,
            cliCommand: agent.cliCommand,
            installCmd: agent.installCmd,
            uninstallCmd: agent.uninstallCmd,
            updateCmd: agent.updateCmd,
            versionCmd: agent.versionCmd,
            latestVersionCmd: agent.latestVersionCmd,
            runCmdTemplate: agent.runCmdTemplate,
            outputParser: agent.outputParser,
            sessionIdSource: agent.sessionIdSource,
            sessionIdField: agent.sessionIdField,
            resumeArgTemplate: agent.resumeArgTemplate,
            skillsDir: agent.skillsDir,
            skillEntryFile: agent.skillEntryFile,
            skillDisplayMode: agent.skillDisplayMode,
            color: agent.color,
            icon: agent.icon,
            sortOrder: agent.sortOrder,
            isEnabled: agent.isEnabled,
          },
        });
        showToast(`已更新 ${agent.displayName}`, 'success');
        fetchAgents();
      } catch (updateErr: any) {
        showToast(`安装失败: ${updateErr}`, 'error');
      }
    }
    setSaving(false);
  };

  const handleToggleEnabled = async (agent: AgentConfig) => {
    try {
      await invoke('update_agent', {
        payload: { agentType: agent.agentType, isEnabled: !agent.isEnabled },
      });
      fetchAgents();
    } catch (err: any) {
      showToast(`操作失败: ${err}`, 'error');
    }
  };

  // ──────────────────────────────────────────────
  //  导入/导出 Agent 配置（JSON）
  // ──────────────────────────────────────────────

  const handleExport = async () => {
    try {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const filePath = await save({
        defaultPath: 'pilotdesk-agents.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (filePath) {
        await invoke('export_agents_json', { filePath });
        showToast(`已导出 ${agents.length} 个 Agent 配置`, 'success');
      }
    } catch (err: any) {
      showToast(`导出失败: ${err}`, 'error');
    }
  };

  const handleImport = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        filters: [{ name: 'JSON', extensions: ['json'] }],
        multiple: false,
      });
      if (!filePath) return;
      const result = await invoke<{ success: number; errors: string[] }>('import_agents_json', {
        filePath: filePath as string,
      });
      if (result.errors.length > 0) {
        showToast(`导入 ${result.success} 个成功，${result.errors.length} 个失败`, 'warning');
      } else {
        showToast(`成功导入 ${result.success} 个 Agent 配置`, 'success');
      }
      fetchAgents();
    } catch (err: any) {
      showToast(`导入失败: ${err}`, 'error');
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-8">
        <Loader2 size={16} className="animate-spin" style={{ color: 'var(--text-secondary)' }} />
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <SettingsSection title="已注册 Agent">
        <div className="flex gap-2 mb-3">
          <SettingsButton
            variant="secondary"
            icon={<Download size={11} />}
            onClick={handleExport}
          >
            导出配置
          </SettingsButton>
          <SettingsButton
            variant="secondary"
            icon={<Upload size={11} />}
            onClick={handleImport}
          >
            导入配置
          </SettingsButton>
        </div>
        <div className="space-y-2">
          {agents.map((agent) => {
            const theme = getTheme(agent.agentType);
            const isEditing = editingType === agent.agentType;
            return (
              <SettingsCard key={agent.agentType}>
                {isEditing ? (
                  <EditForm
                    form={editForm!}
                    onChange={setEditForm}
                    onSave={handleSave}
                    onCancel={() => { setEditingType(null); setEditForm(null); }}
                    saving={saving}
                  />
                ) : (
                  <div className="flex items-center gap-3 w-full">
                    {/* Color dot + name */}
                    <div className="flex items-center gap-2 min-w-0 flex-1">
                      <div
                        className="w-3 h-3 rounded-full shrink-0"
                        style={{ backgroundColor: agent.color }}
                      />
                      <div className="min-w-0">
                        <div className="flex items-center gap-1.5">
                          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                            {agent.displayName}
                          </span>
                          {agent.isBuiltin && (
                            <span className="text-[10px] px-1 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                              预置
                            </span>
                          )}
                        </div>
                        <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                          {agent.agentType} · {agent.cliCommand}
                        </div>
                      </div>
                    </div>

                    {/* Actions */}
                    <div className="flex items-center gap-1 shrink-0">
                      <SettingsButton
                        onClick={() => handleToggleEnabled(agent)}
                        variant="secondary"
                        icon={agent.isEnabled ? <Check size={11} /> : <X size={11} />}
                      >
                        {agent.isEnabled ? '已启用' : '已禁用'}
                      </SettingsButton>
                      <SettingsButton
                        onClick={() => handleEdit(agent)}
                        variant="secondary"
                        icon={<Pencil size={11} />}
                      />
                      {!agent.isBuiltin && (
                        <SettingsButton
                          onClick={() => handleDelete(agent.agentType)}
                          variant="danger"
                          icon={<Trash2 size={11} />}
                        />
                      )}
                    </div>
                  </div>
                )}
              </SettingsCard>
            );
          })}
        </div>
      </SettingsSection>

      {/* Agent 市场 */}
      <SettingsSection title="Agent 市场">
        <div className="space-y-2">
          {marketAgents.length === 0 ? (
            <div className="text-xs py-2" style={{ color: 'var(--text-secondary)' }}>
              {marketLoading ? '加载中...' : (
                <SettingsButton
                  variant="secondary"
                  icon={<Download size={11} />}
                  onClick={() => { fetchMarket(); setShowMarket(true); }}
                >
                  浏览 Agent 市场
                </SettingsButton>
              )}
            </div>
          ) : (
            marketAgents.map((agent) => {
              const isInstalled = agents.some(a => a.agentType === agent.agentType);
              return (
                <SettingsCard key={`market-${agent.agentType}`}>
                  <div className="flex items-center gap-3 w-full">
                    <div className="flex items-center gap-2 min-w-0 flex-1">
                      <div className="w-3 h-3 rounded-full shrink-0" style={{ backgroundColor: agent.color }} />
                      <div className="min-w-0">
                        <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                          {agent.displayName}
                        </div>
                        <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                          {agent.description}
                        </div>
                      </div>
                    </div>
                    <SettingsButton
                      variant={isInstalled ? 'secondary' : 'primary'}
                      onClick={() => handleInstallFromMarket(agent)}
                      disabled={saving}
                    >
                      {isInstalled ? '已安装' : '安装'}
                    </SettingsButton>
                  </div>
                </SettingsCard>
              );
            })
          )}
        </div>
      </SettingsSection>

      {/* ────────────────────────────── */}
      {/* 技能浏览 */}
      {/* ────────────────────────────── */}
      <SettingsSection title="技能浏览">
        <div className="relative mb-3">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索技能..."
            className="w-full pl-7 pr-3 py-1.5 rounded-lg text-xs outline-none"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
          />
        </div>

        {skillsLoading ? (
          <div className="flex items-center justify-center py-4">
            <Loader2 size={14} className="animate-spin" style={{ color: 'var(--text-secondary)' }} />
          </div>
        ) : !hasAnySkills ? (
          <div className="flex flex-col items-center justify-center py-4 gap-1">
            <FolderOpen size={20} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配技能' : '暂无已安装技能'}
            </span>
          </div>
        ) : (
          <div className="space-y-1">
            {allAgentTypes.map((agt) => {
              const filtered = getFiltered(agt);
              return (
                <div key={agt} className="rounded-lg overflow-hidden" style={{ border: '1px solid var(--border)' }}>
                  {/* Agent header */}
                  <div
                    className="flex items-center gap-2 px-3 py-2 cursor-pointer select-none"
                    style={{ backgroundColor: 'var(--bg-secondary)' }}
                    onClick={() => toggleAgent(agt)}
                  >
                    <Bot size={13} style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                    <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
                      {getDisplayName(agt)}
                    </span>
                    <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      ({filtered.length})
                    </span>
                    <span className="ml-auto" style={{ color: 'var(--text-tertiary)' }}>
                      {collapsedAgents.has(agt) ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                    </span>
                  </div>

                  {/* Skills */}
                  {!collapsedAgents.has(agt) && (
                    filtered.length > 0 ? filtered.map((skill) => (
                      <button
                        key={`${agt}:${skill.name}`}
                        onClick={() => {
                          setSelectedSkill({ agent: agt, ...skill });
                        }}
                        className="w-full flex items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-[var(--bg-tertiary)]"
                        style={{
                          backgroundColor: selectedSkill?.agent === agt && selectedSkill?.name === skill.name
                            ? 'var(--bg-tertiary)'
                            : 'transparent',
                          borderTop: '1px solid var(--border)',
                        }}
                      >
                        <Cpu size={12} className="shrink-0" style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                        <div className="flex-1 min-w-0 text-left">
                          <div className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
                            {skill.name}
                          </div>
                          <div className="text-[10px] truncate" style={{ color: 'var(--text-secondary)' }}>
                            {skill.description}
                          </div>
                        </div>
                      </button>
                    )) : (
                      <div className="px-3 py-2 text-[10px]" style={{ color: 'var(--text-tertiary)', borderTop: '1px solid var(--border)' }}>
                        暂无技能文件
                      </div>
                    )
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* Skill detail */}
        {selectedSkill && (
          <div className="mt-2 px-3 py-2 rounded-lg" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
            <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
              <span style={{ color: 'var(--text-tertiary)' }}>来源:</span> {getDisplayName(selectedSkill.agent)}
              {selectedSkill.category && (
                <span className="ml-2">
                  <span style={{ color: 'var(--text-tertiary)' }}>分类:</span> {selectedSkill.category}
                </span>
              )}
            </div>
          </div>
        )}
      </SettingsSection>

      {/* Add Agent button */}
      {!showAddForm ? (
        <SettingsButton
          onClick={() => setShowAddForm(true)}
          variant="primary"
          icon={<Plus size={12} />}
        >
          添加自定义 Agent
        </SettingsButton>
      ) : (
        <SettingsSection title="添加自定义 Agent">
          <div className="space-y-3">
            <div className="grid grid-cols-2 gap-3">
              <FormField label="Agent 类型" value={addForm.agentType || ''} onChange={(v) => setAddForm({ ...addForm, agentType: v })} placeholder="如 my-agent" />
              <FormField label="显示名称" value={addForm.displayName || ''} onChange={(v) => setAddForm({ ...addForm, displayName: v })} placeholder="如 My Agent" />
            </div>
            <FormField label="描述" value={addForm.description || ''} onChange={(v) => setAddForm({ ...addForm, description: v })} placeholder="简短描述" />
            <FormField label="CLI 命令" value={addForm.cliCommand || ''} onChange={(v) => setAddForm({ ...addForm, cliCommand: v })} placeholder="如 my-cli" />
            <FormField label="安装命令" value={addForm.installCmd || ''} onChange={(v) => setAddForm({ ...addForm, installCmd: v })} placeholder="如 npm install -g my-agent" />
            <FormField label="运行命令模板" value={addForm.runCmdTemplate || ''} onChange={(v) => setAddForm({ ...addForm, runCmdTemplate: v })} placeholder="如 my-cli {message}" />
            <div className="grid grid-cols-2 gap-3">
              <FormField label="主题色" value={addForm.color || '#6366F1'} onChange={(v) => setAddForm({ ...addForm, color: v })} placeholder="#6366F1" />
              <FormField label="图标" value={addForm.icon || '🤖'} onChange={(v) => setAddForm({ ...addForm, icon: v })} placeholder="🤖" />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <FormField label="技能目录路径" value={addForm.skillsDir || ''} onChange={(v) => setAddForm({ ...addForm, skillsDir: v })} placeholder="如 ~/.claude/skills" />
              <FormField label="技能入口文件名" value={addForm.skillEntryFile || 'SKILL.md'} onChange={(v) => setAddForm({ ...addForm, skillEntryFile: v })} />
            </div>
            <FormField label="技能显示模式" value={addForm.skillDisplayMode || 'recursive'} onChange={(v) => setAddForm({ ...addForm, skillDisplayMode: v })} placeholder="recursive 或 collection" />
            <div className="flex gap-2 justify-end">
              <SettingsButton variant="secondary" onClick={() => setShowAddForm(false)}>取消</SettingsButton>
              <SettingsButton variant="primary" onClick={handleAdd} disabled={saving}>
                {saving ? <><Loader2 size={11} className="animate-spin" /> 添加中</> : '确认添加'}
              </SettingsButton>
            </div>
          </div>
        </SettingsSection>
      )}
    </div>
  );
}

// ──────────────────────────────────────────────
//  Sub-components
// ──────────────────────────────────────────────

function EditForm({ form, onChange, onSave, onCancel, saving }: {
  form: Partial<AgentConfig>;
  onChange: (f: Partial<AgentConfig>) => void;
  onSave: () => void;
  onCancel: () => void;
  saving: boolean;
}) {
  return (
    <div className="w-full space-y-2">
      <div className="grid grid-cols-2 gap-2">
        <FormField label="显示名称" value={form.displayName || ''} onChange={(v) => onChange({ ...form, displayName: v })} />
        <FormField label="主题色" value={form.color || '#6366F1'} onChange={(v) => onChange({ ...form, color: v })} />
      </div>
      <FormField label="描述" value={form.description || ''} onChange={(v) => onChange({ ...form, description: v })} />
      <FormField label="CLI 命令" value={form.cliCommand || ''} onChange={(v) => onChange({ ...form, cliCommand: v })} />
      <FormField label="安装命令" value={form.installCmd || ''} onChange={(v) => onChange({ ...form, installCmd: v })} />
      <FormField label="运行命令模板" value={form.runCmdTemplate || ''} onChange={(v) => onChange({ ...form, runCmdTemplate: v })} />
      <div className="grid grid-cols-2 gap-2">
        <FormField label="输出解析器" value={form.outputParser || 'raw-text'} onChange={(v) => onChange({ ...form, outputParser: v })} />
        <FormField label="Session ID 来源" value={form.sessionIdSource || 'none'} onChange={(v) => onChange({ ...form, sessionIdSource: v })} />
      </div>
      <FormField label="恢复参数模板" value={form.resumeArgTemplate || ''} onChange={(v) => onChange({ ...form, resumeArgTemplate: v })} placeholder="如 --resume {session_id}" />
      <div className="grid grid-cols-2 gap-2">
        <FormField label="技能目录路径" value={form.skillsDir || ''} onChange={(v) => onChange({ ...form, skillsDir: v })} placeholder="如 ~/.claude/skills" />
        <FormField label="技能入口文件名" value={form.skillEntryFile || 'SKILL.md'} onChange={(v) => onChange({ ...form, skillEntryFile: v })} />
      </div>
      <FormField label="技能显示模式" value={form.skillDisplayMode || 'recursive'} onChange={(v) => onChange({ ...form, skillDisplayMode: v })} placeholder="recursive 或 collection" />
      <div className="flex gap-2 justify-end pt-1">
        <SettingsButton variant="secondary" onClick={onCancel}>取消</SettingsButton>
        <SettingsButton variant="primary" onClick={onSave} disabled={saving}>
          {saving ? <><Loader2 size={11} className="animate-spin" /> 保存中</> : '保存'}
        </SettingsButton>
      </div>
    </div>
  );
}

function FormField({ label, value, onChange, placeholder }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  return (
    <div>
      <label className="block text-[10px] mb-0.5" style={{ color: 'var(--text-secondary)' }}>{label}</label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="w-full px-2 py-1.5 rounded-lg text-xs outline-none"
        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
      />
    </div>
  );
}
