import { useState, useEffect, useCallback, useRef } from 'react';
import { Plus, Trash2, Pencil, Save, X, Loader2, Check, Palette, Download, Upload, Info, Package, Terminal, Repeat, Activity, BookOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import type { AgentConfig } from '../../types';
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
  const [deleteConfirm, setDeleteConfirm] = useState<string | null>(null);

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
          npmPackage: editForm.npmPackage,
          pipPackage: editForm.pipPackage,
          installCmd: editForm.installCmd,
          uninstallCmd: editForm.uninstallCmd,
          updateCmd: editForm.updateCmd,
          versionCmd: editForm.versionCmd,
          latestVersionCmd: editForm.latestVersionCmd,
          runCmdTemplate: editForm.runCmdTemplate,
          outputParser: editForm.outputParser,
          outputFilterRegex: editForm.outputFilterRegex,
          versionPattern: editForm.versionPattern,
          sessionIdSource: editForm.sessionIdSource,
          sessionIdEventType: editForm.sessionIdEventType,
          sessionIdField: editForm.sessionIdField,
          resumeArgTemplate: editForm.resumeArgTemplate,
          skillsDir: editForm.skillsDir,
          skillEntryFile: editForm.skillEntryFile,
          skillDisplayMode: editForm.skillDisplayMode,
          version: editForm.version,
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
      showToast('请填写 Agent 标识、名称和 CLI 命令', 'error');
      return;
    }
    setSaving(true);
    try {
      await invoke('add_agent', { payload: { ...addForm, icon: addForm.icon || '', version: addForm.version || '' } });
      showToast('Agent 已添加', 'success');
      setShowAddForm(false);
      setAddForm({});
      fetchAgents();
    } catch (err: any) {
      showToast(`添加失败: ${err}`, 'error');
    }
    setSaving(false);
  };

  const MARKET_URLS = [
    'https://raw.githubusercontent.com/JorrynRen/PilotDesk/refs/heads/main/server/market/agents-config/agents-config.json',
    'https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/agents-config/agents-config.json',
  ];

  /** 带超时的 fetch，timeoutMs 默认 8 秒 */
  const fetchWithTimeout = (url: string, timeoutMs = 8000): Promise<Response> => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    return fetch(url, { signal: controller.signal }).finally(() => clearTimeout(timer));
  };

  const fetchMarket = async () => {
    setMarketLoading(true);
    let lastError: string | null = null;
    for (const url of MARKET_URLS) {
      try {
        const response = await fetchWithTimeout(url);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const data = await response.json();
        setMarketAgents(data.agents || []);
        setMarketLoading(false);
        return;
      } catch (err: any) {
        lastError = err.name === 'AbortError' ? '连接超时' : String(err.message || err);
      }
    }
    showToast(`无法连接 Agent 市场，请检查网络连接`, 'warning');
    setMarketLoading(false);
  };

  /** 比较两个版本号（支持 v1, v1.1, v1.1.1 格式），返回 true 表示 a > b */
  const isNewerVersion = (a: string, b: string): boolean => {
    const normalize = (v: string) => {
      const parts = v.replace(/^v/i, '').split('.').map(Number);
      // 补零对齐：v1 → [1,0,0], v1.1 → [1,1,0]
      while (parts.length < 3) parts.push(0);
      return parts;
    };
    const pa = normalize(a), pb = normalize(b);
    for (let i = 0; i < 3; i++) {
      if (pa[i] > pb[i]) return true;
      if (pa[i] < pb[i]) return false;
    }
    return false; // equal
  };

  const handleInstallFromMarket = async (agent: AgentConfig) => {
    setSaving(true);
    try {
      const local = agents.find(a => a.agentType === agent.agentType);
      if (local) {
        // 已存在 → 检查版本
        if (!isNewerVersion(agent.version, local.version)) {
          showToast(`${agent.displayName} 已是最新版本 (${local.version})`, 'info');
          setSaving(false);
          return;
        }
        // 更新
        await invoke('update_agent', {
          payload: {
            agentType: agent.agentType,
            displayName: agent.displayName,
            description: agent.description,
            cliCommand: agent.cliCommand,
            npmPackage: agent.npmPackage,
            pipPackage: agent.pipPackage,
            installCmd: agent.installCmd,
            uninstallCmd: agent.uninstallCmd,
            updateCmd: agent.updateCmd,
            versionCmd: agent.versionCmd,
            latestVersionCmd: agent.latestVersionCmd,
            runCmdTemplate: agent.runCmdTemplate,
            outputParser: agent.outputParser,
            outputFilterRegex: agent.outputFilterRegex,
            versionPattern: agent.versionPattern,
            sessionIdSource: agent.sessionIdSource,
            sessionIdEventType: agent.sessionIdEventType,
            sessionIdField: agent.sessionIdField,
            resumeArgTemplate: agent.resumeArgTemplate,
            skillsDir: agent.skillsDir,
            skillEntryFile: agent.skillEntryFile,
            skillDisplayMode: agent.skillDisplayMode,
            color: agent.color,
            icon: agent.icon,
            sortOrder: agent.sortOrder,
            isEnabled: agent.isEnabled,
            version: agent.version,
          },
        });
        showToast(`已更新 ${agent.displayName} (${local.version} → ${agent.version})`, 'success');
      } else {
        // 新安装
        await invoke('add_agent', { payload: { ...agent, version: agent.version } });
        showToast(`已安装 ${agent.displayName} (${agent.version})`, 'success');
      }
      fetchAgents();
    } catch (err: any) {
      showToast(`操作失败: ${err}`, 'error');
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
        <div className="flex items-center justify-between mb-3">
          <div className="flex gap-2">
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
          {!showAddForm ? (
            <SettingsButton
              onClick={() => setShowAddForm(true)}
              variant="primary"
              icon={<Plus size={12} />}
            >
              添加自定义 Agent
            </SettingsButton>
          ) : (
            <SettingsButton
              variant="secondary"
              icon={<X size={11} />}
              onClick={() => { setShowAddForm(false); setAddForm({}); }}
            >
              取消添加
            </SettingsButton>
          )}
        </div>

        {/* 添加自定义 Agent 表单 */}
        {showAddForm && (
          <div className="mb-4 p-3 rounded-lg" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
            <div className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>添加自定义 Agent</div>
            <AgentForm
              form={addForm}
              onChange={setAddForm}
              onSubmit={handleAdd}
              onCancel={() => { setShowAddForm(false); setAddForm({}); }}
              saving={saving}
              mode="add"
            />
          </div>
        )}

        <div className="space-y-2">
          {agents.map((agent) => {
            const theme = getTheme(agent.agentType);
            const isEditing = editingType === agent.agentType;
            return (
              <SettingsCard key={agent.agentType}>
                {isEditing ? (
                  <AgentForm
                    form={editForm!}
                    onChange={setEditForm}
                    onSubmit={handleSave}
                    onCancel={() => { setEditingType(null); setEditForm(null); }}
                    saving={saving}
                    mode="edit"
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
                          {agent.cliCommand} · {agent.description}
                        </div>
                      </div>
                    </div>

                    {/* Actions */}
                    <div className="flex items-center gap-1 shrink-0">
                      <SettingsButton
                        onClick={() => handleToggleEnabled(agent)}
                        variant={agent.isEnabled ? 'primary' : 'secondary'}
                        icon={agent.isEnabled ? <Check size={11} /> : <X size={11} />}
                        title={agent.isEnabled ? '点击禁用此 Agent' : '点击启用此 Agent'}
                      >
                        {agent.isEnabled ? '已启用' : '已禁用'}
                      </SettingsButton>
                      <SettingsButton
                        onClick={() => handleEdit(agent)}
                        variant="secondary"
                        icon={<Pencil size={11} />}
                        title="编辑此 Agent 配置"
                      />
                      {!agent.isBuiltin && (
                        <SettingsButton
                          onClick={() => setDeleteConfirm(agent.agentType)}
                          variant="danger"
                          icon={<Trash2 size={11} />}
                          title="删除此 Agent 配置"
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
      <SettingsSection
        title="Agent 市场"
        actions={
          <>
            <SettingsButton
              variant="secondary"
              icon={<Download size={11} />}
              onClick={() => { fetchMarket(); setShowMarket(true); }}
              disabled={marketLoading}
            >
              {marketLoading ? '加载中...' : '刷新'}
            </SettingsButton>
            {marketAgents.length > 0 && (
              <SettingsButton
                variant="secondary"
                icon={<X size={11} />}
                onClick={() => setMarketAgents([])}
              >
                关闭
              </SettingsButton>
            )}
          </>
        }
      >
        <div className="space-y-2">
          {marketAgents.length === 0 ? (
            <div className="text-xs py-2" style={{ color: 'var(--text-secondary)' }}>
              {marketLoading ? '加载中...' : '点击"刷新"浏览 Agent 市场'}
            </div>
          ) : (
            marketAgents.map((agent) => {
              const local = agents.find(a => a.agentType === agent.agentType);
              const isInstalled = !!local;
              const hasUpdate = isInstalled && isNewerVersion(agent.version, local.version);
              return (
                <SettingsCard key={`market-${agent.agentType}`}>
                  <div className="flex items-center gap-3 w-full">
                    <div className="flex items-center gap-2 min-w-0 flex-1">
                      <div className="w-3 h-3 rounded-full shrink-0" style={{ backgroundColor: agent.color }} />
                      <div className="min-w-0">
                        <div className="flex items-center gap-1.5">
                          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                            {agent.displayName}
                          </span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            v{agent.version}
                          </span>
                        </div>
                        <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                          {agent.description}
                        </div>
                      </div>
                    </div>
                    <SettingsButton
                      variant={hasUpdate ? 'primary' : isInstalled ? 'secondary' : 'primary'}
                      onClick={() => handleInstallFromMarket(agent)}
                      disabled={saving}
                    >
                      {hasUpdate ? `更新 (v${local.version} → v${agent.version})` : isInstalled ? '已安装' : '安装'}
                    </SettingsButton>
                  </div>
                </SettingsCard>
              );
            })
          )}
        </div>
      </SettingsSection>


      {/* 删除确认弹窗 */}
      {deleteConfirm && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => setDeleteConfirm(null)}
        >
          <div
            className="rounded-xl p-5 shadow-xl max-w-sm w-full mx-4"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
              确认删除
            </div>
            <div className="text-xs mb-4" style={{ color: 'var(--text-secondary)' }}>
              确定要删除 <strong style={{ color: 'var(--text-primary)' }}>{deleteConfirm}</strong> 的配置吗？此操作不可撤销。
            </div>
            <div className="flex justify-end gap-2">
              <SettingsButton variant="secondary" onClick={() => setDeleteConfirm(null)}>
                取消
              </SettingsButton>
              <SettingsButton
                variant="danger"
                onClick={() => {
                  const agentType = deleteConfirm;
                  setDeleteConfirm(null);
                  handleDelete(agentType);
                }}
              >
                确认删除
              </SettingsButton>
            </div>
          </div>
        </div>
      )}

    </div>
  );
}

// ──────────────────────────────────────────────
//  Sub-components
// ──────────────────────────────────────────────

function AgentForm({ form, onChange, onSubmit, onCancel, saving, mode }: {
  form: Partial<AgentConfig>;
  onChange: (f: Partial<AgentConfig>) => void;
  onSubmit: () => void;
  onCancel: () => void;
  saving: boolean;
  mode: 'add' | 'edit';
}) {
  const handleUploadIcon = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        filters: [{ name: '图片文件', extensions: ['png', 'jpg', 'jpeg', 'gif', 'ico', 'svg', 'webp'] }],
        multiple: false,
      });
      if (!filePath) return;

      const agentType = form.agentType;
      if (!agentType) {
        showToast('请先填写 Agent 标识', 'error');
        return;
      }

      const result = await invoke<string>('upload_agent_icon', {
        agentType,
        sourcePath: filePath as string,
      });

      onChange({ ...form, icon: result });
      showToast('图标已上传', 'success');
    } catch (err: any) {
      showToast(`上传失败: ${err}`, 'error');
    }
  };
  return (
    <div className="w-full space-y-4">
      {/* 基础信息 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <Info size={11} style={{ color: 'var(--accent)' }} />
          <span>基础信息</span>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <FormField
            label="Agent标识（不可重复）"
            value={form.agentType || ''}
            onChange={(v) => onChange({ ...form, agentType: v })}
            placeholder="如 claude-code"
            readOnly={mode === 'edit'}
          />
          <FormField label="显示名称" value={form.displayName || ''} onChange={(v) => onChange({ ...form, displayName: v })} placeholder="如 Claude Code" />
        </div>
        <FormField label="描述" value={form.description || ''} onChange={(v) => onChange({ ...form, description: v })} placeholder="如 Anthropic 官方 CLI Agent，支持多文件编辑" />
        <FormField label="CLI 命令" value={form.cliCommand || ''} onChange={(v) => onChange({ ...form, cliCommand: v })} placeholder="如 claude" />
        <div className="grid grid-cols-2 gap-2 mt-2">
          <FormField label="主题色" value={form.color || '#6366F1'} onChange={(v) => onChange({ ...form, color: v })} placeholder="如 #6366F1" />
          <div className="flex gap-2 items-end">
            <div className="flex-1">
              <FormField label="图标（支持 Emoji / 图片 URL / 内置图标文件）" value={form.icon || ''} onChange={(v) => onChange({ ...form, icon: v })} placeholder="留空则使用名称首字符" />
            </div>
            <button
              onClick={handleUploadIcon}
              className="px-2 py-1.5 rounded-lg text-xs outline-none shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              title="选择本地图片作为图标"
            >
              <Upload size={11} />
            </button>
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <FormField label="排序序号" value={String(form.sortOrder ?? 0)} onChange={(v) => onChange({ ...form, sortOrder: parseInt(v) || 0 })} placeholder="如 0" />
          <FormField label="配置版本号（非Agent版本号）" value={form.version || ''} onChange={(v) => {
            // 只允许数字和点
            const cleaned = v.replace(/[^\d.]/g, '');
            onChange({ ...form, version: cleaned });
          }} placeholder="如 1.0（留空则默认空）" />
        </div>
      </div>

      {/* 包参数配置 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <Package size={11} style={{ color: 'var(--accent)' }} />
          <span>包参数配置</span>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <FormField label="npm 包名" value={form.npmPackage || ''} onChange={(v) => onChange({ ...form, npmPackage: v || null })} placeholder="如 @anthropic-ai/claude-code" />
          <FormField label="pip 包名" value={form.pipPackage || ''} onChange={(v) => onChange({ ...form, pipPackage: v || null })} placeholder="如 hermes-agent" />
        </div>
      </div>

      {/* 会话参数配置 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <Terminal size={11} style={{ color: 'var(--accent)' }} />
          <span>会话参数配置</span>
        </div>
        <FormField label="运行命令模板" value={form.runCmdTemplate || ''} onChange={(v) => onChange({ ...form, runCmdTemplate: v })} placeholder="如 claude --print {message}" />
        <div className="grid grid-cols-2 gap-2 mt-2">
          <SelectField label="输出解析器" value={form.outputParser || 'raw-text'} onChange={(v) => onChange({ ...form, outputParser: v })} options={[
            { value: 'raw-text', label: 'raw-text（原始文本）' },
            { value: 'json-stream', label: 'json-stream（JSON 流）' },
            { value: 'ansi-text', label: 'ansi-text（ANSI 文本）' },
          ]} />
          <FormField label="输出过滤正则" value={form.outputFilterRegex || ''} onChange={(v) => onChange({ ...form, outputFilterRegex: v })} placeholder="如 ^(?:\\[info\\]|DEBUG)" />
        </div>
      </div>

      {/* 延续会话配置 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <Repeat size={11} style={{ color: 'var(--accent)' }} />
          <span>延续会话配置</span>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <SelectField label="Session ID 来源" value={form.sessionIdSource || 'none'} onChange={(v) => onChange({ ...form, sessionIdSource: v })} options={[
            { value: 'none', label: 'none（不支持会话延续）' },
            { value: 'stdout-json', label: 'stdout-json（标准输出 JSON）' },
            { value: 'stderr-text', label: 'stderr-text（标准错误文本行）' },
          ]} />
          <FormField label="Session ID 事件类型" value={form.sessionIdEventType || ''} onChange={(v) => onChange({ ...form, sessionIdEventType: v })} placeholder="如 system/init" />
        </div>
        <div className="grid grid-cols-2 gap-2 mt-2">
          <FormField label="Session ID 字段名" value={form.sessionIdField || ''} onChange={(v) => onChange({ ...form, sessionIdField: v })} placeholder="如 session_id" />
          <FormField label="恢复参数模板" value={form.resumeArgTemplate || ''} onChange={(v) => onChange({ ...form, resumeArgTemplate: v })} placeholder="如 --resume {session_id}" />
        </div>
      </div>

      {/* 生命周期命令配置 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <Activity size={11} style={{ color: 'var(--accent)' }} />
          <span>生命周期命令配置</span>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <FormField label="安装命令" value={form.installCmd || ''} onChange={(v) => onChange({ ...form, installCmd: v })} placeholder="如 npm install -g @anthropic-ai/claude-code" />
          <FormField label="卸载命令" value={form.uninstallCmd || ''} onChange={(v) => onChange({ ...form, uninstallCmd: v })} placeholder="如 npm uninstall -g @anthropic-ai/claude-code" />
        </div>
        <div className="grid grid-cols-2 gap-2 mt-2">
          <FormField label="更新命令" value={form.updateCmd || ''} onChange={(v) => onChange({ ...form, updateCmd: v })} placeholder="如 npm update -g @anthropic-ai/claude-code" />
          <FormField label="版本检测命令" value={form.versionCmd || ''} onChange={(v) => onChange({ ...form, versionCmd: v })} placeholder="如 claude --version" />
        </div>
        <div className="grid grid-cols-2 gap-2 mt-2">
          <FormField label="最新版本查询命令" value={form.latestVersionCmd || ''} onChange={(v) => onChange({ ...form, latestVersionCmd: v })} placeholder="如 npm view @anthropic-ai/claude-code version" />
          <FormField label="版本号提取正则" value={form.versionPattern || ''} onChange={(v) => onChange({ ...form, versionPattern: v })} placeholder="如 (\\d+\\.\\d+\\.\\d+)" />
        </div>
      </div>

      {/* 技能引用配置 */}
      <div>
        <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
          <BookOpen size={11} style={{ color: 'var(--accent)' }} />
          <span>技能引用配置</span>
        </div>
        <div className="grid grid-cols-3 gap-2">
          <FormField label="技能目录路径" value={form.skillsDir || ''} onChange={(v) => onChange({ ...form, skillsDir: v })} placeholder="如 ~/.claude/skills" />
          <FormField label="技能入口文件名" value={form.skillEntryFile || 'SKILL.md'} onChange={(v) => onChange({ ...form, skillEntryFile: v })} />
          <SelectField label="技能显示模式" value={form.skillDisplayMode || 'recursive'} onChange={(v) => onChange({ ...form, skillDisplayMode: v })} options={[
            { value: 'recursive', label: 'recursive（递归显示全部）' },
            { value: 'collection', label: 'collection（只显示集合名）' },
          ]} />
        </div>
      </div>

      <div className="flex gap-2 justify-end pt-1">
        <SettingsButton variant="secondary" onClick={onCancel}>取消</SettingsButton>
        <SettingsButton variant="primary" onClick={onSubmit} disabled={saving}>
          {saving ? <><Loader2 size={11} className="animate-spin" /> {mode === 'add' ? '添加中' : '保存中'}</> : (mode === 'add' ? '确认添加' : '保存')}
        </SettingsButton>
      </div>
    </div>
  );
}

function FormField({ label, value, onChange, placeholder, readOnly }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  readOnly?: boolean;
}) {
  return (
    <div>
      <label className="block text-[10px] mb-0.5" style={{ color: 'var(--text-secondary)' }}>{label}</label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        readOnly={readOnly}
        className="w-full px-2 py-1.5 rounded-lg text-xs outline-none"
        style={{ 
          backgroundColor: readOnly ? 'var(--bg-tertiary)' : 'var(--bg-primary)', 
          color: readOnly ? 'var(--text-tertiary)' : 'var(--text-primary)', 
          border: '1px solid var(--border)' 
        }}
      />
    </div>
  );
}
function SelectField({ label, value, onChange, options }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <div>
      <label className="block text-[10px] mb-0.5" style={{ color: 'var(--text-secondary)' }}>{label}</label>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full px-2 py-1.5 rounded-lg text-xs outline-none"
        style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>{opt.label}</option>
        ))}
      </select>
    </div>
  );
}


