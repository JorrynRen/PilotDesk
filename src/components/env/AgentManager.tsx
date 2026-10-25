import React, { useState, useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Plus, Trash2, Pencil, X, Loader2, Check, Download, Upload, Info, Terminal, Repeat, BookOpen, Store as StoreIcon, Play } from 'lucide-react';
import { save as saveDialog, open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { errorMessage } from '../../utils/errorMessage';

import type { AgentConfig, LifecycleSpec, PkgState } from '../../types';
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
  const [searchParams] = useSearchParams();
  const { agents, loading, fetchAgents } = useAgentRegistry();
  const [editingType, setEditingType] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<Partial<AgentConfig> | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [addForm, setAddForm] = useState<Partial<AgentConfig>>({});
  const [saving, setSaving] = useState(false);

  // ── 市场草稿的安装 ──
  const [installing, setInstalling] = useState<string | null>(null);
  const [uninstalling, setUninstalling] = useState<string | null>(null);
  const [updating, setUpdating] = useState<string | null>(null);
  const [installMsg, setInstallMsg] = useState<Record<string, string>>({});

  useEffect(() => {
    const un = listen<{ agent: string; message: string }>('install-progress', (e) => {
      const { agent, message } = e.payload;
      setInstallMsg((m) => ({ ...m, [agent]: message }));
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  /** 执行本机安装（npm/shell），完成后刷新状态；失败原因由后端落到 pkgState.lastError。 */
  const handleInstall = async (agentType: string) => {
    setInstalling(agentType);
    try {
      await invoke('agent_market_install', { agentType });
      showToast('安装完成，Agent 已就绪', 'success');
    } catch (err) {
      showToast(`安装失败: ${errorMessage(err)}`, 'error');
    }
    setInstalling(null);
    const fresh = await fetchAgents();
    // 编辑面板若正开着这个 Agent，同步包状态——否则用户会一直看到「待安装」。
    const updated = fresh.find((a) => a.agentType === agentType);
    setEditForm((prev) =>
      prev && prev.agentType === agentType
        ? { ...prev, pkgState: updated?.pkgState, lifecycle: updated?.lifecycle }
        : prev
    );
  };
  /** 卸载本机依赖（适配器 / CLI 包）：保留 Agent 配置，只把包状态退回「待安装」。 */
  const handleUninstall = async (agentType: string) => {
    const name = agents.find((a) => a.agentType === agentType)?.displayName || agentType;
    const target = transportMap[agentType] === 'acp' ? 'ACP 适配器' : '本地 CLI 包';
    const ok = await confirmDialog({
      title: '确认卸载',
      message: `将从本机卸载 ${name} 的${target}。Agent 配置会保留，卸载后需重新安装才能用于会话。`,
      confirmText: '卸载',
      danger: true,
    });
    if (!ok) return;
    setUninstalling(agentType);
    try {
      await invoke('uninstall_agent', { agentType });
      showToast('已卸载本机依赖', 'success');
    } catch (err) {
      showToast(`卸载失败: ${errorMessage(err)}`, 'error');
    }
    setUninstalling(null);
    const fresh = await fetchAgents();
    const updated = fresh.find((a) => a.agentType === agentType);
    setEditForm((prev) =>
      prev && prev.agentType === agentType
        ? { ...prev, pkgState: updated?.pkgState, lifecycle: updated?.lifecycle }
        : prev
    );
  };

  /** 更新：按市场最新规格覆盖本地配置后重装（仅市场提供版本信息的条目）。 */
  const handleUpdate = async (agentType: string) => {
    setUpdating(agentType);
    try {
      await invoke('agent_market_update', { agentType });
      showToast('已更新到市场最新版本', 'success');
    } catch (err) {
      showToast(`更新失败: ${errorMessage(err)}`, 'error');
    }
    setUpdating(null);
    const fresh = await fetchAgents();
    const updated = fresh.find((a) => a.agentType === agentType);
    setEditForm((prev) =>
      prev && prev.agentType === agentType
        ? {
            ...prev,
            pkgState: updated?.pkgState,
            lifecycle: updated?.lifecycle,
            distSource: updated?.distSource,
          }
        : prev
    );
  };

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 4, // 移动4px后才激活拖拽，防止点击误触
      },
    })
  );

  // 接入方式速查（agentType → transport）：用于把列表按「兼容模式 / ACP」分组。
  // 走独立的只读命令而不是把该列塞进 AgentConfig——后者要改它 4 处手工构造点（级联）。
  const [transportMap, setTransportMap] = useState<Record<string, string>>({});
  useEffect(() => {
    let alive = true;
    invoke<Array<{ agentType: string; transport: string }>>('list_agent_transports')
      .then((rows) => {
        if (!alive) return;
        const m: Record<string, string> = {};
        rows.forEach((r) => {
          m[r.agentType] = r.transport;
        });
        setTransportMap(m);
      })
      .catch(() => {
        // 读不到就都按兼容模式显示，不打断列表
      });
    return () => {
      alive = false;
    };
  }, [agents.length]);

  const transportOf = (agentType: string) => transportMap[agentType] ?? 'subprocess';
  // 分组只影响**渲染顺序与标题**：兼容模式在前、ACP 在后，组内保持原序。
  // 拖拽仍走同一个 SortableContext，故排序语义不变（组内拖动落在同一索引区间）。
  const groupedAgents = [...agents].sort(
    (a, b) =>
      (transportOf(a.agentType) === 'acp' ? 1 : 0) -
      (transportOf(b.agentType) === 'acp' ? 1 : 0)
  );

  const handleEdit = (agent: AgentConfig) => {
    setEditingType(agent.agentType);
    setEditForm({ ...agent });
  };

  // 深链 `/settings?tab=agents&agent=<agentType>`：市场「获取」后直接落到该 Agent 的编辑/安装态。
  // 用状态记录已打开的深链，避免用户关掉表单后又被 URL 反复拉回来。
  // 这是"由外部输入派生本地状态"的渲染期修正，放在渲染阶段而不是 effect（effect 内同步 setState 会级联渲染）。
  const [deepOpened, setDeepOpened] = useState<string | null>(null);
  const deepAgent = searchParams.get('agent');
  if (deepAgent && !loading && deepOpened !== deepAgent) {
    const target = agents.find((a) => a.agentType === deepAgent);
    if (target) {
      setDeepOpened(deepAgent);
      setEditingType(target.agentType);
      setEditForm({ ...target });
    }
  }

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
          runCmdTemplate: editForm.runCmdTemplate,
          outputParser: editForm.outputParser,
          outputFilterRegex: editForm.outputFilterRegex,
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
      // 接入方式独立落库（`update_agent` 不写这两列）。
      // **只在拿到回显值时才写**：编辑态若因故没回显成功，`editForm.transport` 为空，
      // 此时绝不能按默认值回写——那会把已配好的 ACP 设置覆盖成兼容模式（数据丢失）。
      if (editForm.transport) {
        await invoke('set_agent_transport', {
          agentType: editingType,
          transport: editForm.transport,
          connJson: editForm.conn ?? '',
        });
      }
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
    // ACP 用 conn 里的 command；兼容模式的命令写在「运行命令模板」里（{message} 占位）。
    const isAcp = (addForm.transport || 'subprocess') === 'acp';
    let acpCommand = '';
    if (isAcp) {
      try {
        acpCommand = (JSON.parse(addForm.conn || '{}') as { command?: string }).command || '';
      } catch {
        acpCommand = ''; // conn 非法就当作没填，由下面的提示拦住
      }
    }
    const missing = !addForm.agentType || !addForm.displayName || (isAcp && !acpCommand);
    if (missing) {
      showToast(
        isAcp ? '请填写 Agent 标识、名称和连接命令（command）' : '请填写 Agent 标识和名称',
        'error'
      );
      return;
    }
    setSaving(true);
    try {
      // transport/conn 不属于 CreateAgentPayload，先摘掉（后端只认接入方式之外的字段）
      const addPayload: Partial<AgentConfig> = { ...addForm };
      delete addPayload.transport;
      delete addPayload.conn;
      await invoke('add_agent', {
        payload: { ...addPayload, icon: addForm.icon || '', version: addForm.version || '' },
      });
      // 新建成功后再落库接入方式（添加态没有既存行，只能建完再设）
      if (addForm.agentType) {
        await invoke('set_agent_transport', {
          agentType: addForm.agentType,
          transport: addForm.transport ?? 'subprocess',
          connJson: addForm.conn ?? '',
        });
      }
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
            <SettingsButton
              variant="secondary"
              icon={<StoreIcon size={11} />}
              onClick={() => navigate('/market?tab=agents')}
            >
              从资源市集获取配置
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
            {/* 单一 SortableContext：分组只影响渲染顺序与标题，不改变拖拽语义 */}
            <SortableContext items={groupedAgents.map(a => a.agentType)} strategy={verticalListSortingStrategy}>
              {groupedAgents.map((agent, i) => {
                const t = transportOf(agent.agentType);
                const prevT = i > 0 ? transportOf(groupedAgents[i - 1].agentType) : null;
                return (
                  <React.Fragment key={agent.agentType}>
                    {/* 分组标题：接入方式切换处插一条（兼容模式 / ACP） */}
                    {t !== prevT && (
                      <div
                        className="flex items-center gap-2 pt-1 pb-0.5 text-[10px] font-semibold"
                        style={{ color: 'var(--text-secondary)' }}
                      >
                        <span>{t === 'acp' ? 'ACP（Agent Client Protocol）' : '兼容模式（本地 CLI）'}</span>
                        <span className="flex-1 h-px" style={{ backgroundColor: 'var(--border)' }} />
                      </div>
                    )}
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
                      onInstall={handleInstall}
                      onUninstall={handleUninstall}
                      onUpdate={handleUpdate}
                      installing={installing}
                      uninstalling={uninstalling}
                      updating={updating}
                      installMsg={installMsg[agent.agentType]}
                    />
                  </React.Fragment>
                );
              })}
            </SortableContext>
          </DndContext>
        </div>
      </SettingsSection>

    </div>
  );
}

// ──────────────────────────────────────────────
//  Sub-components
// ──────────────────────────────────────────────

function AgentForm({ form, onChange, onSubmit, onCancel, saving, mode, onInstall, onUninstall, onUpdate, installing, uninstalling, updating, installMsg }: {
  form: Partial<AgentConfig>;
  onChange: (f: Partial<AgentConfig>) => void;
  onSubmit: () => void;
  onCancel: () => void;
  saving: boolean;
  mode: 'add' | 'edit';
  onInstall?: (agentType: string) => void;
  onUninstall?: (agentType: string) => void;
  onUpdate?: (agentType: string) => void;
  installing?: string | null;
  uninstalling?: string | null;
  updating?: string | null;
  installMsg?: string;
}) {
  // ── 接入方式（设计文档 §4.2）──
  // state 就放在 `form` 里（TS 类型已加可选的 transport/conn），由**父组件**在创建/更新成功后
  // 统一落库（set_agent_transport）——添加态没有既存行，只能建完再设。
  // 编辑态打开时回显当前值；必须回显，否则"打开面板→保存"会把已配好的 ACP 设置重置掉。
  useEffect(() => {
    if (mode !== 'edit' || !form.agentType) return;
    let alive = true;
    invoke<{ transport: string; conn: string | null }>('get_agent_transport', {
      agentType: form.agentType,
    })
      .then((r) => {
        if (!alive) return;
        // 幂等守卫：避免 setState → 重渲染 → 再 setState 的循环
        if (form.transport === r.transport && (form.conn ?? '') === (r.conn ?? '')) return;
        onChange({ ...form, transport: r.transport, conn: r.conn ?? '' });
      })
      .catch(() => {
        // 读不到就按默认（兼容模式）显示，不打断表单
      });
    return () => {
      alive = false;
    };
    // 只在切换 Agent / 模式时重跑（form 不放进依赖，见上面的幂等守卫）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, form.agentType]);

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
  // ── conn 的界面拆字段 ──
  // 落库格式仍是 JSON 文本（`conn`），但界面上拆成独立字段：用户只填值，不需要面对 JSON。
  // 解析失败一律当空对象处理，不报错、不打断表单。
  const parseConn = (): Record<string, unknown> => {
    try {
      const o = JSON.parse(form.conn || '{}');
      return o && typeof o === 'object' && !Array.isArray(o) ? (o as Record<string, unknown>) : {};
    } catch {
      return {};
    }
  };
  const connGet = (k: string): string => {
    const v = parseConn()[k];
    return typeof v === 'string' ? v : '';
  };
  const connSet = (k: string, v: string) => {
    const o = parseConn();
    if (v.trim()) {
      o[k] = v;
    } else {
      delete o[k]; // 留空即不写入该字段（保持 conn 干净）
    }
    onChange({ ...form, conn: JSON.stringify(o) });
  };
  // args 是数组：界面用一个输入框（空格分隔），保存时拆成数组
  const connArgsText = (): string => {
    const a = parseConn().args;
    return Array.isArray(a) ? a.map(String).join(' ') : '';
  };
  const connSetArgs = (v: string) => {
    const o = parseConn();
    const parts = v.split(/\s+/).filter(Boolean);
    if (parts.length) {
      o.args = parts;
    } else {
      delete o.args;
    }
    onChange({ ...form, conn: JSON.stringify(o) });
  };

  // ACP 的命令/参数由市场下发（官方目录条目安装后会自动填入解析结果），
  // 集成配置页只保留「自定义填写」入口——预置不再硬编码在前端。

  return (
    <div className="w-full space-y-4">
      {/* 市场获取的实例：本机安装状态与后续动作（预置/官方目录的安装规格由市集下发，
          这里不再让用户手填安装命令）。 */}
      {mode === 'edit' && form.lifecycle && (
        <InstallStatusSection
          agentType={form.agentType!}
          transport={form.transport ?? 'subprocess'}
          pkgState={form.pkgState}
          lifecycle={form.lifecycle}
          canUpdate={!!form.distSource?.registryVersion}
          onInstall={onInstall}
          onUninstall={onUninstall}
          onUpdate={onUpdate}
          installing={!!installing && installing === form.agentType}
          uninstalling={!!uninstalling && uninstalling === form.agentType}
          updating={!!updating && updating === form.agentType}
          message={installMsg}
        />
      )}
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
        {/* 命令不再作为独立字段：兼容模式由「运行命令模板」决定实际执行，
            ACP 由「接入方式」区的 command + args 决定；安装/版本规格统一由市集下发。 */}
        {(form.transport || 'subprocess') === 'acp' && (
          <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
            ACP 接入方式的命令请在下方「接入方式」区填写（command + args），此处无需 CLI 命令。
          </div>
        )}
        <div className="grid grid-cols-2 gap-2 mt-2">
          <FormField label="主题色" value={form.color || '#6366F1'} onChange={(v) => onChange({ ...form, color: v })} placeholder="如 #6366F1" />
          <div className="flex gap-2 items-end">
            <div className="flex-1">
              <FormField label="图标（支持 Emoji / 图片 URL / 内置图标文件）" value={form.icon || ''} onChange={(v) => onChange({ ...form, icon: v })} placeholder="留空则使用名称首字符" />
            </div>
            <button
              onClick={handleUploadIcon}
              className="px-2 py-1.5 rounded-lg text-xs outline-none shrink-0"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              title="选择本地图片作为图标"
            >
              <Upload size={11} />
            </button>
          </div>
        </div>
      </div>

      {/* 接入方式：置于「排序 → 基础信息」之后，与兼容模式的字段板块顺序保持一致。
          添加与编辑都显示：ACP 的关键字段（command / args）只存在于本区块，
          若只在添加态渲染，编辑既有 ACP Agent 时这些字段就无处可看、无处可改。
          编辑态由上方 effect 经 `get_agent_transport` 预填当前值，因此打开即回显原配置。
          只留两种：兼容模式(subprocess) / ACP；http(在线API) 与 内嵌Web 各有专属入口，不列
          （列进来会变成"两个入口配同一件事"）。 */}
      {(mode === 'add' || mode === 'edit') && (
        <div>
          <div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2" style={{ color: 'var(--text-secondary)' }}>
            <Repeat size={11} style={{ color: 'var(--accent)' }} />
            <span>接入方式</span>
          </div>
          <SelectField
            label="接入方式"
            value={form.transport || 'subprocess'}
            onChange={(v) => onChange({ ...form, transport: v })}
            options={[
              { value: 'subprocess', label: '兼容模式（本地 CLI，默认）' },
              { value: 'acp', label: 'ACP（Agent Client Protocol）' },
            ]}
          />
          {(form.transport || 'subprocess') === 'acp' && (
            <div className="mt-2 space-y-2">
              {/* 工作目录不在此配置：运行时 cwd 取「会话/项目工作目录」（为空回退全局工作区），
                  见 lib.rs resolved_cwd。此处若再放一个 cwd 字段是无消费方的死配置。 */}
              {/* 预置与官方目录条目统一由「资源市集 › CLI Agent 配置」下发，本页只保留自定义填写。 */}
              <FormField
                label="命令（command，可直接修改）"
                value={connGet('command')}
                onChange={(v) => connSet('command', v)}
                placeholder="如 hermes / npx / claude-code-acp"
              />
              <FormField
                label="参数（args，空格分隔）"
                value={connArgsText()}
                onChange={connSetArgs}
                placeholder="如 acp"
              />
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary, var(--text-secondary))' }}>
                工作目录跟随「会话 / 项目工作目录」，无需在此配置。
              </div>
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary, var(--text-secondary))' }}>
                将保存为：{form.conn || '{}'}
              </div>
            </div>
          )}
        </div>
      )}

      {/* ── 以下整段是**本地 CLI（兼容模式）专属**：选了 ACP 就整体隐藏 ──
          ACP 自描述（连接靠 command/args 建立），包名/输出解析/session id/技能目录都不适用。 */}
      {(form.transport || 'subprocess') === 'subprocess' && (
        <>
      {/* 包名与安装/版本命令不再手填：全部由市集下发的 lifecycle 规格决定
          （见顶部「本机安装」区，安装按钮就在那里）。 */}

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
        </>
      )}

      <div className="flex gap-2 justify-end pt-1">
        <SettingsButton variant="secondary" onClick={onCancel}>取消</SettingsButton>
        <SettingsButton variant="primary" onClick={onSubmit} disabled={saving}>
          {saving ? <><Loader2 size={11} className="animate-spin" /> {mode === 'add' ? '添加中' : '保存中'}</> : (mode === 'add' ? '确认添加' : '保存')}
        </SettingsButton>
      </div>
    </div>
  );
}

/** 「本机安装」区：呈现包安装状态并提供安装/重试动作。
 *  安装规格（包名、安装/版本命令、运行参数）由市集下发，这里只呈现结果、不提供编辑。 */
function InstallStatusSection({ agentType, transport, pkgState, lifecycle, canUpdate, onInstall, onUninstall, onUpdate, installing, uninstalling, updating, message }: {
  agentType: string;
  transport: string;
  pkgState?: PkgState | null;
  lifecycle: LifecycleSpec;
  /** 市场是否提供可更新的版本信息（预置/手工配置没有，就不给「更新」按钮） */
  canUpdate?: boolean;
  onInstall?: (agentType: string) => void;
  onUninstall?: (agentType: string) => void;
  onUpdate?: (agentType: string) => void;
  installing?: boolean;
  uninstalling?: boolean;
  updating?: boolean;
  message?: string;
}) {
  const isAcp = transport === 'acp';
  // 卸载的对象按接入方式说清楚：ACP 卸的是适配器，CLI 卸的是它自己那个包
  const uninstallLabel = isAcp ? '卸载适配器' : '卸载 CLI 包';
  // P1 仅支持 npm / shell；uvx / binary 由市集显示「暂不支持」，这里兜底说明。
  const supported = lifecycle.kind === 'npm' || lifecycle.kind === 'shell';
  const status = pkgState?.status;
  const ready = status === 'ready';
  const failed = status === 'failed';
  const packages = (lifecycle.packages ?? []).map((p) => p.package).join(' + ');
  const runtime = lifecycle.runtime
    ? [lifecycle.runtime.command, ...(lifecycle.runtime.args ?? [])].join(' ')
    : '';
  const tone = ready
    ? 'var(--success, #16a34a)'
    : failed
      ? 'var(--danger, #dc2626)'
      : 'var(--warning, #d97706)';
  const statusText = ready
    ? `已就绪${pkgState?.installedVersion ? ` v${pkgState.installedVersion}` : ''}`
    : failed
      ? '安装失败'
      : status === 'pending_install'
        ? '待安装'
        : '未安装';

  return (
    <div
      className="p-2.5 rounded-lg"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-center gap-2">
        <span className="text-[10px] font-semibold" style={{ color: 'var(--text-secondary)' }}>本机安装</span>
        <span className="text-[10px]" style={{ color: tone }}>{statusText}</span>
        <span className="flex-1" />
        {!ready && (
          <SettingsButton
            onClick={() => onInstall?.(agentType)}
            variant="primary"
            disabled={!supported || installing || uninstalling || updating}
            icon={installing ? <Loader2 size={11} className="animate-spin" /> : <Play size={11} />}
            title={supported ? undefined : '当前版本暂不支持该安装方式'}
          >
            {installing ? '安装中' : failed ? '重试' : '安装'}
          </SettingsButton>
        )}
        {ready && canUpdate && (
          <SettingsButton
            onClick={() => onUpdate?.(agentType)}
            variant="secondary"
            disabled={installing || uninstalling || updating}
            icon={updating ? <Loader2 size={11} className="animate-spin" /> : <Repeat size={11} />}
            title="按市场最新规格重新安装（Agent 配置保留）"
          >
            {updating ? '更新中' : '更新'}
          </SettingsButton>
        )}
        {(ready || failed) && (
          <SettingsButton
            onClick={() => onUninstall?.(agentType)}
            variant="danger"
            disabled={installing || uninstalling || updating}
            icon={uninstalling ? <Loader2 size={11} className="animate-spin" /> : <Trash2 size={11} />}
            title={`${uninstallLabel}（保留 Agent 配置，卸载后需重新安装才能用于会话）`}
          >
            {uninstalling ? '卸载中' : uninstallLabel}
          </SettingsButton>
        )}
      </div>
      {(ready || failed) && (
        <div className="text-[10px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
          卸载只移除本机{isAcp ? '适配器' : 'CLI 包'}，Agent 配置与接入方式保留。
        </div>
      )}
      {!supported && (
        <div className="text-[10px] mt-1" style={{ color: 'var(--warning, #d97706)' }}>
          该条目为 {lifecycle.kind} 安装方式，当前版本暂不支持，后续版本提供。
        </div>
      )}
      {packages && (
        <div className="text-[10px] mt-1 font-mono truncate" style={{ color: 'var(--text-tertiary)' }}>
          {lifecycle.kind} · {packages}
        </div>
      )}
      {runtime && (
        <div className="text-[10px] mt-0.5 font-mono truncate" style={{ color: 'var(--text-tertiary)' }}>
          运行：{runtime}
        </div>
      )}
      {pkgState?.lastError && (
        <div className="text-[10px] mt-1 break-all" style={{ color: 'var(--danger, #dc2626)' }}>
          {pkgState.lastError}
        </div>
      )}
      {installing && message && (
        <div className="text-[10px] mt-1 break-all" style={{ color: 'var(--text-secondary)' }}>
          {message}
        </div>
      )}
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
          backgroundColor: readOnly ? 'var(--bg-field)' : 'var(--bg-primary)', 
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

/** 卡片副行的命令：优先取市集下发的运行规格，手工配置回退到 Agent 标识。 */
function runtimeLabel(agent: AgentConfig): string {
  const rt = agent.lifecycle?.runtime;
  if (rt?.command) return [rt.command, ...(rt.args ?? [])].join(' ');
  return agent.agentType;
}

/** 本地安装状态：`pkgState` 为空表示手工配置（视为就绪，不显示状态）。 */
function agentStatus(agent: AgentConfig): { text: string; tone: 'ok' | 'warn' | 'error' } | null {
  const s = agent.pkgState;
  if (!s) return null;
  if (s.status === 'ready') {
    return { text: `已就绪${s.installedVersion ? ` v${s.installedVersion}` : ''}`, tone: 'ok' };
  }
  if (s.status === 'failed') return { text: '安装失败', tone: 'error' };
  if (s.status === 'pending_install') return { text: '待安装', tone: 'warn' };
  return { text: s.status, tone: 'warn' };
}

// ──────────────────────────────────────────────
//  SortableAgentItem — 可拖拽排序的 Agent 卡片
// ──────────────────────────────────────────────
function SortableAgentItem({ agent, isEditing, editForm, setEditForm, handleSave, saving, setEditingType, handleToggleEnabled, handleEdit, requestDelete, onInstall, onUninstall, onUpdate, installing, uninstalling, updating, installMsg }: {
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
  onInstall: (agentType: string) => void;
  onUninstall: (agentType: string) => void;
  onUpdate: (agentType: string) => void;
  installing: string | null;
  uninstalling: string | null;
  updating: string | null;
  installMsg?: string;
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

  const status = agentStatus(agent);
  const busy = installing === agent.agentType;

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
            onInstall={onInstall}
            onUninstall={onUninstall}
            onUpdate={onUpdate}
            installing={installing}
            uninstalling={uninstalling}
            updating={updating}
            installMsg={installMsg}
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
                <div className="text-[10px] flex items-center gap-1.5 min-w-0" style={{ color: 'var(--text-secondary)' }}>
                  <span className="font-mono shrink-0">{runtimeLabel(agent)}</span>
                  {status && (
                    <span
                      className="shrink-0"
                      style={{
                        color:
                          status.tone === 'ok'
                            ? 'var(--success, #16a34a)'
                            : status.tone === 'error'
                              ? 'var(--danger, #dc2626)'
                              : 'var(--warning, #d97706)',
                      }}
                    >
                      {status.text}
                    </span>
                  )}
                  {agent.description && <span className="truncate">· {agent.description}</span>}
                </div>
              </div>
            </div>

            {/* Actions */}
            <div className="flex items-center gap-1 shrink-0">
              {/* 市场草稿：未就绪时把「安装」放在最前面，避免用户先进编辑页再找按钮 */}
              {status && status.tone !== 'ok' && (
                <SettingsButton
                  onClick={() => onInstall(agent.agentType)}
                  variant="primary"
                  disabled={busy}
                  icon={busy ? <Loader2 size={11} className="animate-spin" /> : <Play size={11} />}
                  title="在本机安装该 Agent 的依赖包"
                >
                  {busy ? '安装中' : status.tone === 'error' ? '重试' : '安装'}
                </SettingsButton>
              )}
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


