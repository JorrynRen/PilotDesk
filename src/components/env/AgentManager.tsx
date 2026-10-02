import React, { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Plus, Trash2, Pencil, X, Loader2, Check, Download, Upload, Info, Package, Terminal, Repeat, Activity, BookOpen, Store as StoreIcon } from 'lucide-react';
import { save as saveDialog, open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { errorMessage } from '../../utils/errorMessage';

import type { AgentConfig } from '../../types';
import { SettingsSection, SettingsCard, SettingsButton } from '../settings';
import { Select } from '../common/Select';
import {
  DndContext,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
  arrayMove,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';

// ──────────────────────────────────────────────
//  Agent Manager — Phase 4
// ──────────────────────────────────────────────

export function AgentManager() {
  const navigate = useNavigate();
  const { agents, loading, fetchAgents } = useAgentRegistry();
  const [editingType, setEditingType] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<Partial<AgentConfig> | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [addForm, setAddForm] = useState<Partial<AgentConfig>>({});
  const [saving, setSaving] = useState(false);
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 4, // 移动4px后才激活拖拽，防止点击误触
      },
    })
  );

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
    } catch (err) {
      showToast(`更新失败: ${errorMessage(err)}`, 'error');
    }
    setSaving(false);
  };

  const handleDelete = async (agentType: string) => {
    try {
      await invoke('delete_agent', { agentType });
      showToast('Agent 已删除', 'success');
      fetchAgents();
    } catch (err) {
      showToast(`删除失败: ${errorMessage(err)}`, 'error');
    }
  };

  /** 删除前二次确认（统一走全局确认弹窗） */
  const requestDelete = async (agentType: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: `确定要删除「${agentType}」的配置吗？此操作不可撤销。`,
      confirmText: '删除',
    });
    if (ok) await handleDelete(agentType);
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
    } catch (err) {
      showToast(`添加失败: ${errorMessage(err)}`, 'error');
    }
    setSaving(false);
  };

  const handleDragEnd = async (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const ids = agents.map(a => a.agentType);
    const oldIndex = ids.indexOf(active.id as string);
    const newIndex = ids.indexOf(over.id as string);
    if (oldIndex === -1 || newIndex === -1) return;

    const newOrder = arrayMove(ids, oldIndex, newIndex);
    try {
      await invoke('reorder_agents', { agentTypes: newOrder });
      fetchAgents();
    } catch (err) {
      showToast(`排序失败: ${errorMessage(err)}`, 'error');
      fetchAgents();
    }
  };

  const handleToggleEnabled = async (agent: AgentConfig) => {
    try {
      await invoke('update_agent', {
        payload: { agentType: agent.agentType, isEnabled: !agent.isEnabled },
      });
      fetchAgents();
    } catch (err) {
      showToast(`操作失败: ${errorMessage(err)}`, 'error');
    }
  };

  // ──────────────────────────────────────────────
  //  导入/导出 Agent 配置（JSON）
  // ──────────────────────────────────────────────

  const handleExport = async () => {
    try {
      // 使用静态导入 saveDialog
      const filePath = await saveDialog({
        defaultPath: 'pilotdesk-agents.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (filePath) {
        await invoke('export_agents_json', { filePath });
        showToast(`已导出 ${agents.length} 个 Agent 配置`, 'success');
      }
    } catch (err) {
      showToast(`导出失败: ${errorMessage(err)}`, 'error');
    }
  };

  const handleImport = async () => {
    try {
      
      const filePath = await openDialog({
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
    } catch (err) {
      showToast(`导入失败: ${errorMessage(err)}`, 'error');
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
              onChange={(f) => setAddForm(f)}
              onSubmit={handleAdd}
              onCancel={() => { setShowAddForm(false); setAddForm({}); }}
              saving={saving}
              mode="add"
            />
          </div>
        )}

        <div className="space-y-2">
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
            <SortableContext items={agents.map(a => a.agentType)} strategy={verticalListSortingStrategy}>
              {agents.map((agent) => (
                <SortableAgentItem
                  key={agent.agentType}
                  agent={agent}
                  isEditing={editingType === agent.agentType}
                  editForm={editForm}
                  setEditForm={setEditForm}
                  handleSave={handleSave}
                  saving={saving}
                  setEditingType={setEditingType}
                  handleToggleEnabled={handleToggleEnabled}
                  handleEdit={handleEdit}
                  requestDelete={requestDelete}
                />
              ))}
            </SortableContext>
          </DndContext>
        </div>
      </SettingsSection>

      {/* Agent 配置市场已迁至「资源市集 › CLI Agent 配置」：设置页只保留本地 Registry 的增删改与排序 */}
      <SettingsSection title="Agent 配置市场" description="在线浏览与拉取各 CLI Agent 的集成配置">
        <SettingsCard>
          <div className="flex items-center gap-3 w-full">
            <StoreIcon size={14} style={{ color: 'var(--accent)', flexShrink: 0 }} />
            <div className="min-w-0 flex-1">
              <div className="text-xs" style={{ color: 'var(--text-primary)' }}>去资源市集获取 Agent 配置</div>
            </div>
            <SettingsButton
              variant="primary"
              onClick={() => navigate('/market?tab=agents')}
            >
              前往资源市集
            </SettingsButton>
          </div>
        </SettingsCard>
      </SettingsSection>

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
      
      const filePath = await openDialog({
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
    } catch (err) {
      showToast(`上传失败: ${errorMessage(err)}`, 'error');
    }
  };
  return (
    <div className="w-full space-y-4">
      <div className="grid grid-cols-2 gap-2 mb-4">
        <FormField label="排序序号" value={String(form.sortOrder ?? 0)} onChange={(v) => onChange({ ...form, sortOrder: parseInt(v) || 0 })} placeholder="如 0" />
        <FormField label="配置版本号（非Agent版本号）" value={form.version || ''} onChange={(v) => {
          // 只允许数字和点
          const cleaned = v.replace(/[^\d.]/g, '');
          onChange({ ...form, version: cleaned });
        }} placeholder="如 1.0（留空则默认空）" />
      </div>

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
            { value: 'stdout-text', label: 'stdout-text（标准输出文本行）' },
            { value: 'stderr-text', label: 'stderr-text（标准错误文本行）' },
            { value: 'stdout-json', label: 'stdout-json（标准输出 JSON）' },
            { value: 'stderr-json', label: 'stderr-json（标准错误 JSON）' },
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
          <SelectField label="技能显示模式" value={form.skillDisplayMode || 'collection'} onChange={(v) => onChange({ ...form, skillDisplayMode: v })} options={[
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
      <Select
        value={value}
        onChange={(v) => onChange(v)}
        options={options}
        className="w-full"
      />
    </div>
  );
}

// ──────────────────────────────────────────────
//  SortableAgentItem — 可拖拽排序的 Agent 卡片
// ──────────────────────────────────────────────
function SortableAgentItem({ agent, isEditing, editForm, setEditForm, handleSave, saving, setEditingType, handleToggleEnabled, handleEdit, requestDelete }: {
  agent: AgentConfig;
  isEditing: boolean;
  editForm: Partial<AgentConfig> | null;
  setEditForm: (f: Partial<AgentConfig>) => void;
  handleSave: () => void;
  saving: boolean;
  setEditingType: (t: string | null) => void;
  handleToggleEnabled: (agent: AgentConfig) => void;
  handleEdit: (agent: AgentConfig) => void;
  requestDelete: (agentType: string) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: agent.agentType });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
    position: 'relative',
    zIndex: isDragging ? 10 : 'auto',
  };

  return (
    <div ref={setNodeRef} style={style}>
      {isEditing ? (
        <SettingsCard>
          <AgentForm
            form={editForm || {}}
            onChange={(f) => setEditForm(f)}
            onSubmit={handleSave}
            onCancel={() => { setEditingType(null); setEditForm(null as unknown as Partial<AgentConfig>); }}
            saving={saving}
            mode="edit"
          />
        </SettingsCard>
      ) : (
        <SettingsCard>
          <div className="flex items-center gap-3 w-full">
            {/* Drag handle — 直接绑定 attributes/listeners */}
            <div
              className="shrink-0 flex items-center justify-center cursor-grab active:cursor-grabbing"
              style={{ color: 'var(--text-tertiary)', padding: '6px 4px', margin: '-6px -4px' }}
              {...attributes}
              {...listeners}
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor" style={{ display: 'block' }}>
                <circle cx="4" cy="2" r="1.2" />
                <circle cx="8" cy="2" r="1.2" />
                <circle cx="4" cy="6" r="1.2" />
                <circle cx="8" cy="6" r="1.2" />
                <circle cx="4" cy="10" r="1.2" />
                <circle cx="8" cy="10" r="1.2" />
              </svg>
            </div>
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
                  {agent.version && (
                    <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      v{agent.version}
                    </span>
                  )}
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
                title="编辑此 Agent 配置"
                icon={<Pencil size={11} />}
              />
              {!agent.isBuiltin && (
                <SettingsButton
                  onClick={() => requestDelete(agent.agentType)}
                  variant="danger"
                  title="删除此 Agent 配置"
                  icon={<Trash2 size={11} />}
                />
              )}
            </div>
          </div>
        </SettingsCard>
      )}
    </div>
  );
}


