import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { Play, Plus, Trash2, Clock, CheckCircle, XCircle, AlertCircle, Upload, Download, Settings, GitBranch, Activity, Layout, FileText, Tag, Layers, Zap } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { showToast } from '../utils/toast';
import { TitleBar, StatusBar } from '../components/layout';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import { WorkflowPropertyDialog } from '../components/workflow/WorkflowPropertyDialog';
import type { WorkflowDefinition, WorkflowInstance } from '../types/workflow';

interface WorkflowPageProps {
  onBack?: () => void;
}

/** 将 Cron 表达式转换为用户友好描述 */
function describeCron(expr: string): string {
  if (!expr) return '定时';
  const parts = expr.trim().split(/\s+/);
  if (parts.length < 6) return expr;

  const [sec, min, hour, day, month, week] = parts;

  // 星期映射
  const weekMap: Record<string, string> = {
    '0': '日', '1': '一', '2': '二', '3': '三', '4': '四', '5': '五', '6': '六', '7': '日',
    'SUN': '日', 'MON': '一', 'TUE': '二', 'WED': '三', 'THU': '四', 'FRI': '五', 'SAT': '六',
  };

  // 解析小时和分钟
  const fmtTime = (h: string, m: string) => {
    const hh = h.padStart(2, '0');
    const mm = m.padStart(2, '0');
    return `${hh}:${mm}`;
  };

  // 解析星期范围
  const describeWeek = (w: string): string | null => {
    if (w === '*') return null;
    // 1-5 → 周一至周五
    const rangeMatch = w.match(/^(\d+)-(\d+)$/);
    if (rangeMatch) {
      const from = weekMap[rangeMatch[1]] || rangeMatch[1];
      const to = weekMap[rangeMatch[2]] || rangeMatch[2];
      return `周${from}至周${to}`;
    }
    // 1,3,5 → 周一、三、五
    if (w.includes(',')) {
      const days = w.split(',').map(d => weekMap[d.trim()] || d.trim()).filter(Boolean);
      return `周${days.join('、')}`;
    }
    // 单个数字
    if (weekMap[w]) return `周${weekMap[w]}`;
    return null;
  };

  // 解析日
  const describeDay = (d: string): string | null => {
    if (d === '*' || d === '?') return null;
    if (d === 'L') return '最后一天';
    if (d.includes(',')) {
      const days = d.split(',').map(x => x.trim());
      return `每月${days.join('、')}日`;
    }
    const rangeMatch = d.match(/^(\d+)-(\d+)$/);
    if (rangeMatch) return `每月${rangeMatch[1]}-${rangeMatch[2]}日`;
    return `每月${d}日`;
  };

  // 解析步进分钟
  const stepMinMatch = min.match(/^\*\/(\d+)$/);
  if (stepMinMatch && hour === '*' && day === '*' && month === '*' && week === '*') {
    return `每 ${stepMinMatch[1]} 分钟`;
  }

  // 解析步进小时
  const stepHourMatch = hour.match(/^\*\/(\d+)$/);
  if (stepHourMatch && min === '0' && day === '*' && month === '*' && week === '*') {
    return `每 ${stepHourMatch[1]} 小时`;
  }

  // 小时范围 9-17
  const hourRangeMatch = hour.match(/^(\d+)-(\d+)$/);
  if (hourRangeMatch && min === '0') {
    const weekDesc = describeWeek(week);
    const base = `每天 ${fmtTime(hourRangeMatch[1], '0')}-${fmtTime(hourRangeMatch[2], '0')} 每小时`;
    return weekDesc ? `${weekDesc} ${fmtTime(hourRangeMatch[1], '0')}-${fmtTime(hourRangeMatch[2], '0')} 每小时` : base;
  }

  // 常规：解析具体时间
  const weekDesc = describeWeek(week);
  const dayDesc = describeDay(day);

  if (weekDesc) {
    // 按星期调度
    return `${weekDesc} ${fmtTime(hour, min)}`;
  }
  if (dayDesc) {
    // 按日期调度
    return `${dayDesc} ${fmtTime(hour, min)}`;
  }
  if (hour === '*' && min === '0') {
    return '每小时整点';
  }
  if (hour === '*' && min !== '0') {
    return `每小时 ${min.padStart(2, '0')} 分`;
  }

  return `${fmtTime(hour, min)}`;
}

export function WorkflowPage({ onBack }: WorkflowPageProps) {
  const navigate = useNavigate();
  const { definitions, instances, loading, error, loadDefinitions, loadInstances, createDefinition, updateDefinition, deleteDefinition, deleteExecution, selectDefinition } = useWorkflowStore();
  const [activeTab, setActiveTab] = useState<'definitions' | 'instances'>('definitions');
  const [showPropertyDialog, setShowPropertyDialog] = useState<'create' | 'edit' | null>(null);
  const [editingDef, setEditingDef] = useState<WorkflowDefinition | null>(null);

  useEffect(() => {
    loadDefinitions();
    loadInstances();
  }, []);

  const handleExportSingle = async (e: React.MouseEvent, id: string, name: string) => {
    e.stopPropagation();
    try {
      const filePath = await saveDialog({
        defaultPath: `${name || '工作流'}.json`,
        filters: [{ name: '工作流文件', extensions: ['json'] }],
      });
      if (filePath) {
        await invoke('export_workflow_to_file', { id, filePath });
      }
    } catch (err: any) {
      console.error('导出工作流失败:', err);
    }
  };

  const handleImportWorkflow = async () => {
    try {
      const filePaths = await openDialog({
        filters: [{ name: '工作流文件', extensions: ['json'] }],
        multiple: true,
      });
      if (!filePaths || (filePaths as string[]).length === 0) return;
      const paths = filePaths as string[];
      let successCount = 0;
      for (const filePath of paths) {
        try {
          await invoke('import_workflow_from_file', { filePath });
          successCount++;
        } catch (innerErr: any) {
          const fileName = filePath.split(/[\/]/).pop();
          showToast(`导入工作流「${fileName}」失败: ${innerErr}`, 'error');
        }
      }
      if (successCount > 0) {
        showToast(`成功导入 ${successCount} 个工作流`, 'success');
      }
      await loadDefinitions();
    } catch (err: any) {
      console.error('导入工作流失败:', err);
    }
  };

  const handleCreateAndEdit = () => {
    setEditingDef(null);
    setShowPropertyDialog('create');
  };

  const handleEditProperties = (e: React.MouseEvent, def: WorkflowDefinition) => {
    e.stopPropagation();
    setEditingDef(def);
    setShowPropertyDialog('edit');
  };

  const handlePropertyConfirm = async (data: {
    name: string;
    description: string;
    version: string;
    trigger: { triggerType: 'manual' | 'cron' | 'event'; cron?: string };
    enabled: boolean;
  }) => {
    setShowPropertyDialog(null);
    if (editingDef) {
      // Edit mode: update existing definition
      try {
        await updateDefinition(editingDef.id, {
          name: data.name,
          description: data.description,
          version: data.version,
          trigger: data.trigger,
          enabled: data.enabled,
        });
      } catch (err) {
        console.error('更新工作流属性失败:', err);
      }
    } else {
      // Create mode: create and navigate to editor
      const def = createDefaultWorkflow(data.name);
      def.description = data.description;
      def.version = data.version;
      def.trigger = data.trigger;
      def.enabled = data.enabled;
      try {
        const id = await createDefinition(def);
        selectDefinition(id);
        navigate(`/workflow/editor?id=${id}`);
      } catch (err) {
        console.error('创建工作流失败:', err);
      }
    }
  };

  const handleStart = async (e: React.MouseEvent, id: string, name: string) => {
    e.stopPropagation();
    try {
      setRunningIds(prev => new Set(prev).add(id));
      await useWorkflowStore.getState().safeStartWorkflow(id);
    } catch (err) {
      setRunningIds(prev => { const next = new Set(prev); next.delete(id); return next; });
      console.error(`启动工作流「${name}」失败:`, err);
      showToast(`启动工作流「${name}」失败: ${err}`, 'error');
    }
  };

  // ── 删除二次确认弹窗 ──
  // ── 执行中状态追踪 + 结果通知 ──
  const [runningIds, setRunningIds] = useState<Set<string>>(new Set());


  // 监听执行状态事件（全局，组件挂载时注册）
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<{ execution_id: string; definition_id: string; definition_name: string; status: string; error?: string }>('workflow:execution-status', (event) => {
      const { status, definition_id: defId, definition_name: defName } = event.payload;
      // 直接使用事件 payload 中的 definition_id，不再依赖 store 查找（消除竞态）
      setRunningIds(prev => {
        const next = new Set(prev);
        if (status === 'running') {
          next.add(defId);
        } else {
          next.delete(defId);
        }
        return next;
      });
      if (status === 'completed' || status === 'failed' || status === 'cancelled') {
        if (status === 'completed') {
          showToast(`${defName} 执行成功`, 'success');
        } else if (status === 'failed') {
          showToast(`${defName} 执行失败${event.payload.error ? ': ' + event.payload.error : ''}`, 'error');
        } else {
          showToast(`${defName} 已取消`, 'warning');
        }
        useWorkflowStore.getState().loadInstances();
      }
    }).then(fn => { unlisten = fn; });

    // 监听阶段门控失败事件
    listen<{ execution_id: string; stage_id: string; stage_name: string; status: string; reason?: string; error?: string }>('workflow:stage-status', (event) => {
      const { status, stage_name: stageName, reason, error } = event.payload;
      if (status === 'gate_failed') {
        const detail = reason || error || '';
        showToast(`${stageName} 门控策略未通过${detail ? ': ' + detail : ''}`, 'error');
      }
    });

    return () => { unlisten?.(); };
  }, []);

  const [confirmDelete, setConfirmDelete] = useState<{
    type: 'definition' | 'execution';
    id: string;
    name: string;
  } | null>(null);

  const handleDelete = async (id: string, name: string) => {
    setConfirmDelete({ type: 'definition', id, name });
  };

  const handleDeleteExecution = async (executionId: string, name: string) => {
    setConfirmDelete({ type: 'execution', id: executionId, name });
  };

  const confirmDeleteAction = async () => {
    if (!confirmDelete) return;
    try {
      if (confirmDelete.type === 'definition') {
        await deleteDefinition(confirmDelete.id);
      } else {
        await deleteExecution(confirmDelete.id);
      }
    } catch (err) {
      console.error('删除失败:', err);
    }
    setConfirmDelete(null);
  };

  const statusIcon = (status: string) => {
    switch (status) {
      case 'success': return <CheckCircle size={14} className="text-green-500" />;
      case 'failed': return <XCircle size={14} className="text-red-500" />;
      case 'running': return <AlertCircle size={14} className="text-blue-500" />;
      case 'pending': return <Clock size={14} className="text-yellow-500" />;
      default: return <Clock size={14} className="text-gray-400" />;
    }
  };

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      <TitleBar
        showBackButton={true}
        titleText="工作流管理"
        onBack={() => navigate('/')}
        onOpenSettings={() => navigate('/settings')}
      />
      {/* Tab navigation — 与设置页同一套UI */}
      <div className="shrink-0 px-4 pt-1 flex items-center gap-0.5 overflow-x-clip" style={{ borderBottom: '1px solid var(--border)' }}>
        <button
          onClick={() => setActiveTab('definitions')}
          className={"pd-tab" + (activeTab === 'definitions' ? " pd-tab-active" : "")}
        >
          <GitBranch size={12} />
          工作流定义 ({definitions.length})
        </button>
        <button
          onClick={() => setActiveTab('instances')}
          className={"pd-tab" + (activeTab === 'instances' ? " pd-tab-active" : "")}
        >
          <Activity size={12} />
          执行记录 ({instances.length})
        </button>
        <button
          className="pd-tab pd-tab-disabled"
        >
          <Layout size={12} />
          模板市场
        </button>

        <div className="flex-1" />

        <div className="flex items-center gap-2 pb-2">
          <button
            onClick={handleCreateAndEdit}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={14} /> 新建工作流
          </button>
          <button
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', cursor: 'not-allowed', opacity: 0.6 }}
            title="即将推出"
            disabled
          >
            从本地模板建立
          </button>
          <button
            onClick={handleImportWorkflow}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <Upload size={14} /> 从文件导入
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {loading && (
          <div className="flex items-center justify-center h-32 text-xs" style={{ color: 'var(--text-tertiary)' }}>
            加载中...
          </div>
        )}

        {error && (
          <div className="p-3 mb-3 rounded text-xs" style={{ backgroundColor: 'var(--status-danger-bg, rgba(239,68,68,0.1))', color: 'var(--status-danger)' }}>
            {error}
          </div>
        )}

        {!loading && activeTab === 'definitions' && (
          <>
            {definitions.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-48 gap-2">
                <GitBranch size={32} style={{ opacity: 0.25, color: 'var(--text-tertiary)' }} />
                <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无工作流定义</div>
              </div>
            ) : (
              <div className="grid gap-3">
                {definitions.map((def) => (
                  <div
                    key={def.id}
                    className="p-3 rounded-lg transition-colors cursor-pointer hover:opacity-90"
                    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                    onClick={() => navigate(`/workflow/editor?id=${def.id}`)}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        <GitBranch size={14} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>{def.name}</span>
                        <span className="text-[10px] px-1.5 py-0.5 rounded" style={{
                          backgroundColor: def.enabled ? 'rgba(34,197,94,0.15)' : 'rgba(107,114,128,0.15)',
                          color: def.enabled ? '#22c55e' : 'var(--text-tertiary)',
                        }}>
                          {def.enabled ? '已启用' : '已禁用'}
                        </span>
                      </div>
                      <div className="flex items-center gap-1">
                        <button
                          onClick={(e) => { handleStart(e, def.id, def.name); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: runningIds.has(def.id) ? '#3fb950' : 'var(--text-secondary)' }}
                          title={runningIds.has(def.id) ? '执行中...' : '执行'}
                          disabled={runningIds.has(def.id)}
                        >
                          {runningIds.has(def.id) ? (
                            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" className="pd-animate-spin">
                              <circle cx="7" cy="7" r="5.5" stroke="currentColor" strokeWidth="1.5" strokeDasharray="8 8" />
                            </svg>
                          ) : (
                            <Play size={14} />
                          )}
                        </button>
                        <button
                          onClick={(e) => { handleEditProperties(e, def); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: 'var(--text-secondary)' }}
                          title="编辑属性"
                        >
                          <Settings size={14} />
                        </button>
                        <button
                          onClick={(e) => { handleExportSingle(e, def.id, def.name); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: 'var(--text-secondary)' }}
                          title="导出"
                        >
                          <Download size={14} />
                        </button>
                        <button
                          onClick={(e) => { e.stopPropagation(); handleDelete(def.id, def.name); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: 'var(--text-secondary)' }}
                          title="删除"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                    {def.description && (
                      <div className="mt-1 flex items-start gap-1.5 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                        <FileText size={11} style={{ flexShrink: 0, marginTop: 1 }} />
                        <span>{def.description}</span>
                      </div>
                    )}
                    <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      <span className="flex items-center gap-1">
                        <Tag size={10} />
                        v{def.version}
                      </span>
                      <span className="flex items-center gap-1">
                        <Layers size={10} />
                        {def.stages?.length || 0} 阶段
                      </span>
                      <span className="flex items-center gap-1">
                        <Zap size={10} />
                        {(() => {
                          const t = def.trigger;
                          if (!t || t.triggerType === 'manual') return '手动';
                          if (t.triggerType === 'cron') return `定时（${describeCron(t.cron || '')}）`;
                          if (t.triggerType === 'event') return `事件${t.eventName ? ' - ' + t.eventName : ''}`;
                          return t.triggerType;
                        })()}
                      </span>
                      <span className="flex items-center gap-1">
                        <Clock size={10} />
                        {new Date(Number(def.createdAt) * 1000).toLocaleString()}
                      </span>
                      <span className="flex items-center gap-1">
                        <Download size={10} />
                        {new Date(Number(def.updatedAt) * 1000).toLocaleString()}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}

        {!loading && activeTab === 'instances' && (
          <>
            {instances.length === 0 ? (
              <div className="flex items-center justify-center h-48 text-xs" style={{ color: 'var(--text-tertiary)' }}>
                暂无执行记录
              </div>
            ) : (
              <div className="grid gap-2">
                {instances.map((inst) => (
                  <div
                    key={inst.id}
                    className="p-3 rounded-lg"
                    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        {statusIcon(inst.status)}
                        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>{inst.definitionName}</span>
                      </div>
                      <div className="flex items-center gap-2">
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          {new Date(Number(inst.createdAt) * 1000).toLocaleString()}
                        </span>
                        <button
                          onClick={(e) => { e.stopPropagation(); handleDeleteExecution(inst.id, inst.definitionName); }}
                          className="p-1 rounded hover:bg-red-500/10 transition-colors"
                          title="删除执行记录"
                        >
                          <Trash2 size={12} style={{ color: 'var(--text-tertiary)' }} className="hover:text-red-500" />
                        </button>
                      </div>
                    </div>
                    <div className="mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      ID: {inst.id.slice(0, 12)}... | 触发器: {inst.trigger}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      {/* 工作流属性对话框 */}
      {showPropertyDialog && (
        <WorkflowPropertyDialog
          mode={showPropertyDialog}
          initial={editingDef || undefined}
          onConfirm={handlePropertyConfirm}
          onClose={() => { setShowPropertyDialog(null); setEditingDef(null); }}
        />
      )}

      {/* 删除二次确认弹窗 */}
      {confirmDelete && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => setConfirmDelete(null)}
        >
          <div
            className="rounded-xl p-5 shadow-xl max-w-sm w-full mx-4"
            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
              确认删除
            </div>
            <div className="text-xs mb-4" style={{ color: 'var(--text-secondary)' }}>
              {confirmDelete.type === 'definition'
                ? `确定删除工作流「${confirmDelete.name}」？此操作不可撤销，关联的执行记录也将被删除。`
                : `确定删除执行记录「${confirmDelete.name}」？此操作不可撤销。`
              }
            </div>
            <div className="flex justify-end gap-2">
              <button
                onClick={() => setConfirmDelete(null)}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
              >
                取消
              </button>
              <button
                onClick={confirmDeleteAction}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ backgroundColor: '#ef4444', color: '#fff' }}
              >
                确认删除
              </button>
            </div>
          </div>
        </div>
      )}

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
