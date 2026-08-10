import { useState, useEffect, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { Play, Plus, Trash2, UserCheck, Clock, CheckCircle, XCircle, AlertCircle, Upload, Download, Settings, GitBranch, Activity, BarChart3, Layout, FileText, Tag, Layers, Zap, Copy, Slash, AlertTriangle, Search, Filter, X, ArrowUpDown, Calendar, Sparkles } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { showToast } from '../utils/toast';
import { TitleBar, StatusBar } from '../components/layout';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import { WorkflowPropertyDialog } from '../components/workflow/WorkflowPropertyDialog';
import { WorkflowMonitor } from '../components/workflow/WorkflowMonitor';
import { ExecutionStats } from '../components/workflow/ExecutionStats';
import { WorkflowTemplateMarket } from '../components/workflow/WorkflowTemplateMarket';
import type { WorkflowDefinition, WorkflowInstance, PendingHumanInput } from '../types/workflow';

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
  const { definitions, instances, pendingInputs, loading, error, loadDefinitions, loadInstances, loadPendingInputs, respondHumanInput, createDefinition, updateDefinition, deleteDefinition, deleteExecution, selectDefinition } = useWorkflowStore();
  const [activeTab, setActiveTab] = useState<'definitions' | 'instances' | 'stats' | 'templates'>('definitions');
  const [showPropertyDialog, setShowPropertyDialog] = useState<'create' | 'edit' | null>(null);
  const [editingDef, setEditingDef] = useState<WorkflowDefinition | null>(null);
  const DEF_PAGE_SIZE = 8;
  const [defPage, setDefPage] = useState(1);

  // ---- 工作流定义筛选状态 ----
  const [defFilterKeyword, setDefFilterKeyword] = useState<string>('');
  const [defFilterEnabled, setDefFilterEnabled] = useState<'all' | 'enabled' | 'disabled'>('all');
  const [defFilterTrigger, setDefFilterTrigger] = useState<'all' | 'manual' | 'cron' | 'event'>('all');
  const [defSortKey, setDefSortKey] = useState<'updatedAt' | 'createdAt' | 'name' | 'stages'>('updatedAt');
  const [defSortAsc, setDefSortAsc] = useState<boolean>(false);

  const DEFINITION_TRIGGER_OPTIONS: Array<{ key: typeof defFilterTrigger; label: string }> = [
    { key: 'all', label: '全部触发' },
    { key: 'manual', label: '手动' },
    { key: 'cron', label: '定时' },
    { key: 'event', label: '事件' },
  ];

  const DEFINITION_ENABLED_OPTIONS: Array<{ key: typeof defFilterEnabled; label: string; color: string }> = [
    { key: 'all', label: '全部', color: 'var(--text-secondary)' },
    { key: 'enabled', label: '已启用', color: '#22c55e' },
    { key: 'disabled', label: '已禁用', color: '#6B7280' },
  ];

  const DEFINITION_SORT_OPTIONS = [
    { key: 'updatedAt', label: '更新时间' },
    { key: 'createdAt', label: '创建时间' },
    { key: 'name', label: '名称 A-Z' },
    { key: 'stages', label: '阶段数量' },
  ] as const;

  const hasDefFilter =
    defFilterKeyword.trim() !== '' ||
    defFilterEnabled !== 'all' ||
    defFilterTrigger !== 'all' ||
    defSortKey !== 'updatedAt' ||
    defSortAsc !== false;

  const clearDefFilters = () => {
    setDefFilterKeyword('');
    setDefFilterEnabled('all');
    setDefFilterTrigger('all');
    setDefSortKey('updatedAt');
    setDefSortAsc(false);
  };

  // 每个 def 的实例数快速汇总（从 instances 计算）
  const defInstanceCounts = useMemo(() => {
    const map = new Map<string, number>();
    instances.forEach(i => {
      if (!i.definitionId) return;
      map.set(i.definitionId, (map.get(i.definitionId) || 0) + 1);
    });
    return map;
  }, [instances]);

  // 筛选 + 排序（结果为 filteredDefs，分页基于它计算）
  const filteredDefs = useMemo(() => {
    const kw = defFilterKeyword.trim().toLowerCase();
    const result = definitions.filter(def => {
      // 启用状态
      if (defFilterEnabled === 'enabled' && !def.enabled) return false;
      if (defFilterEnabled === 'disabled' && def.enabled) return false;
      // 触发方式
      if (defFilterTrigger !== 'all') {
        const tt = def.trigger?.triggerType || 'manual';
        if (tt !== defFilterTrigger) return false;
      }
      // 关键字（匹配名称、描述、版本号、定义ID）
      if (kw) {
        const haystack = [
          def.name,
          def.description || '',
          'v' + String(def.version || ''),
          def.id,
        ].join(' ').toLowerCase();
        if (!haystack.includes(kw)) return false;
      }
      return true;
    });

    // 排序
    const sign = defSortAsc ? 1 : -1;
    result.sort((a, b) => {
      switch (defSortKey) {
        case 'name':
          return sign * (a.name || '').localeCompare(b.name || '', 'zh-CN');
        case 'stages':
          return sign * ((a.stages?.length || 0) - (b.stages?.length || 0));
        case 'createdAt':
          return sign * ((a.createdAt || 0) - (b.createdAt || 0));
        case 'updatedAt':
        default:
          return sign * ((a.updatedAt || 0) - (b.updatedAt || 0));
      }
    });

    return result;
  }, [definitions, defFilterKeyword, defFilterEnabled, defFilterTrigger, defSortKey, defSortAsc]);

  // 筛选条件/排序变化时回到第 1 页
  useEffect(() => {
    setDefPage(1);
  }, [defFilterKeyword, defFilterEnabled, defFilterTrigger, defSortKey, defSortAsc]);

  useEffect(() => {
    loadDefinitions();
    loadInstances();
    loadPendingInputs();
    const pendingInterval = setInterval(loadPendingInputs, 5000);
    // 检测崩溃后残留的 running/paused 实例
    invoke<Array<{ executionId: string; definitionName: string; status: string }>>('list_recoverable_executions')
      .then(recoverable => {
        if (recoverable.length > 0) {
          showToast('检测到 ' + recoverable.length + ' 个未完成的执行记录（已自动标记为失败）', 'warning');
        }
      })
      .catch(() => {});
    return () => {
      clearInterval(pendingInterval);
    };
  }, []);

  const handleExportSingle = async (e: React.MouseEvent, id: string, name: string) => {
    e.stopPropagation();
    try {
      const dirPath = await openDialog({
        directory: true,
        title: '选择导出目录',
        defaultPath: name || '工作流',
      });
      if (dirPath) {
        await invoke('export_workflow_to_file', { id, dirPath });
        showToast('工作流导出成功', 'success');
      }
    } catch (err: any) {
      console.error('导出工作流失败:', err);
      showToast(`导出工作流失败: ${err}`, 'error');
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
    icon?: string;
    inputSchema?: Record<string, { type: string; description?: string; default?: any }>;
    outputSchema?: Record<string, { type: string; description?: string }>;
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
          ...(data.icon !== undefined ? { icon: data.icon } : {}),
          ...(data.inputSchema !== undefined ? { inputSchema: data.inputSchema } : {}),
          ...(data.outputSchema !== undefined ? { outputSchema: data.outputSchema } : {}),
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
      if (data.icon) def.icon = data.icon;
      if (data.inputSchema) def.inputSchema = data.inputSchema;
      if (data.outputSchema) def.outputSchema = data.outputSchema;
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
        useWorkflowStore.getState().loadPendingInputs();
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

  const handleDuplicate = async (id: string, name: string) => {
    try {
      await useWorkflowStore.getState().duplicateDefinition(id, name + ' (副本)');
    } catch (err) {
      console.error('复制失败:', err);
    }
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
      case 'cancelled': return <Slash size={14} className="text-amber-500" />;
      case 'timeout': return <AlertTriangle size={14} className="text-red-500" />;
      default: return <Clock size={14} className="text-gray-400" />;
    }
  };

  // Computed pagination (moved outside JSX for oxc parser compatibility)
  const totalDefPages = Math.max(1, Math.ceil(filteredDefs.length / DEF_PAGE_SIZE));
  const safeDefPage = Math.min(defPage, totalDefPages);
  const paginatedDefs = filteredDefs.slice((safeDefPage - 1) * DEF_PAGE_SIZE, safeDefPage * DEF_PAGE_SIZE);

  // Auto-correct page when data changes shrink the list beyond current page
  useEffect(() => {
    if (defPage > totalDefPages) {
      setDefPage(totalDefPages);
    }
  }, [filteredDefs.length, totalDefPages, defPage]);

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
          执行实例 ({instances.length})
        </button>
        <button
          onClick={() => setActiveTab('stats')}
          className={"pd-tab" + (activeTab === 'stats' ? " pd-tab-active" : "")}
        >
          <BarChart3 size={12} />
          统计
        </button>
        <button
          onClick={() => setActiveTab('templates')}
          className={"pd-tab" + (activeTab === 'templates' ? " pd-tab-active" : "")}
        >
          <Sparkles size={12} />
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
            <Download size={14} /> 从文件导入
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
            {/* ── 工作流定义筛选工具栏 ── */}
            {definitions.length > 0 && (
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 8,
                  padding: '10px 12px',
                  marginBottom: 12,
                  borderRadius: 6,
                  backgroundColor: 'var(--bg-secondary)',
                  border: '1px solid var(--border)',
                }}
              >
                {/* Row：控件单行布局 */}
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
                  <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                    <Filter size={12} style={{ color: 'var(--text-tertiary)' }} />
                    <span className="text-[11px]" style={{ color: 'var(--text-tertiary)', fontWeight: 500 }}>筛选：</span>
                  </div>

                  {/* 启用状态药丸 */}
                  <div
                    aria-label="启用状态"
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 3,
                      flexWrap: 'nowrap',
                      padding: '2px 4px',
                      border: '1px solid var(--border)',
                      backgroundColor: 'var(--bg-primary)',
                      borderRadius: 4,
                      overflowX: 'auto',
                      scrollbarWidth: 'thin',
                    }}
                  >
                    {DEFINITION_ENABLED_OPTIONS.map(opt => {
                      const active = defFilterEnabled === opt.key;
                      return (
                        <button
                          key={opt.key}
                          onClick={() => setDefFilterEnabled(opt.key)}
                          className="text-[10px] px-2 py-0.5 rounded-full transition-colors whitespace-nowrap"
                          style={{
                            border: active ? '1px solid ' + opt.color : '1px solid transparent',
                            backgroundColor: active ? (opt.key === 'all' ? 'rgba(var(--accent-rgb, var(--accent)), 0.1)' : opt.color + '22') : 'transparent',
                            color: active ? (opt.key === 'all' ? 'var(--accent)' : opt.color) : 'var(--text-tertiary)',
                            fontWeight: active ? 500 : 400,
                            flexShrink: 0,
                          }}
                        >
                          {opt.label}
                        </button>
                      );
                    })}
                  </div>

                  {/* 触发方式下拉 */}
                  <select
                    value={defFilterTrigger}
                    onChange={e => setDefFilterTrigger(e.target.value as typeof defFilterTrigger)}
                    className="pd-btn text-[11px] rounded py-1 px-2"
                    style={{
                      border: '1px solid var(--border)',
                      backgroundColor: 'var(--bg-primary)',
                      color: 'var(--text-primary)',
                      outline: 'none',
                      minWidth: 88,
                    }}
                    title="按触发方式筛选"
                  >
                    {DEFINITION_TRIGGER_OPTIONS.map(t => (
                      <option key={t.key} value={t.key}>{t.label}</option>
                    ))}
                  </select>

                  {/* 排序 */}
                  <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                    <span className="inline-flex items-center gap-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                      <Calendar size={11} />
                      排序：
                    </span>
                    <select
                      value={defSortKey}
                      onChange={e => setDefSortKey(e.target.value as typeof defSortKey)}
                      className="pd-btn text-[11px] rounded py-1 px-2"
                      style={{
                        border: '1px solid var(--border)',
                        backgroundColor: 'var(--bg-primary)',
                        color: 'var(--text-primary)',
                        outline: 'none',
                        minWidth: 96,
                      }}
                      title="排序方式"
                    >
                      {DEFINITION_SORT_OPTIONS.map(s => (
                        <option key={s.key} value={s.key}>{s.label}</option>
                      ))}
                    </select>
                    <button
                      onClick={() => setDefSortAsc(v => !v)}
                      className="pd-btn p-1 rounded transition-colors inline-flex items-center gap-0.5"
                      style={{
                        border: '1px solid var(--border)',
                        backgroundColor: defSortAsc ? 'var(--accent)' : 'var(--bg-primary)',
                        color: defSortAsc ? '#fff' : 'var(--text-secondary)',
                      }}
                      title={defSortAsc ? '降序' : '升序'}
                    >
                      <ArrowUpDown size={11} />
                      <span className="text-[10px] font-mono">{defSortAsc ? '↑' : '↓'}</span>
                    </button>
                  </div>

                  {/* 关键字搜索 */}
                  <div
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 4,
                      border: '1px solid var(--border)',
                      backgroundColor: 'var(--bg-primary)',
                      borderRadius: 4,
                      padding: '2px 6px',
                      flex: '1 1 200px',
                      minWidth: 180,
                      maxWidth: 320,
                    }}
                  >
                    <Search size={11} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
                    <input
                      value={defFilterKeyword}
                      onChange={e => setDefFilterKeyword(e.target.value)}
                      placeholder="搜索名称 / 描述 / 版本 / ID..."
                      className="flex-1 min-w-0 outline-none text-[11px]"
                      style={{ background: 'transparent', border: 'none', color: 'var(--text-primary)' }}
                    />
                    {defFilterKeyword && (
                      <button
                        onClick={() => setDefFilterKeyword('')}
                        className="pd-btn p-0.5 rounded hover:opacity-80 transition-opacity"
                        style={{ color: 'var(--text-tertiary)', background: 'transparent' }}
                        title="清除关键字"
                      >
                        <X size={11} />
                      </button>
                    )}
                  </div>

                  {/* 清除全部筛选 */}
                  <button
                    onClick={clearDefFilters}
                    disabled={!hasDefFilter}
                    className="pd-btn px-2 py-1 text-[11px] rounded transition-colors whitespace-nowrap"
                    style={{
                      border: '1px solid var(--border)',
                      background: hasDefFilter ? 'var(--bg-secondary)' : 'var(--bg-tertiary)',
                      color: hasDefFilter ? 'var(--text-primary)' : 'var(--text-tertiary)',
                      cursor: hasDefFilter ? 'pointer' : 'not-allowed',
                      opacity: hasDefFilter ? 1 : 0.5,
                    }}
                  >
                    清除筛选
                  </button>
                </div>
              </div>
            )}

            {definitions.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-48 gap-2">
                <GitBranch size={32} style={{ opacity: 0.25, color: 'var(--text-tertiary)' }} />
                <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无工作流定义</div>
              </div>
            ) : filteredDefs.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-32 gap-2" style={{ backgroundColor: 'var(--bg-secondary)', borderRadius: 8, border: '1px solid var(--border)' }}>
                <Filter size={20} style={{ color: 'var(--text-tertiary)', opacity: 0.5 }} />
                <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>没有符合筛选条件的工作流</span>
                <button
                  onClick={clearDefFilters}
                  className="pd-btn px-3 py-1 text-[11px] rounded transition-colors"
                  style={{ border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--accent)' }}
                >清除筛选条件</button>
              </div>
            ) : (
              <div className="grid gap-3">
                {paginatedDefs.map((def) => {
                  const instCount = defInstanceCounts.get(def.id) || 0;
                  return (
                    <div
                      key={def.id}
                      className="p-3 rounded-lg transition-colors cursor-pointer hover:opacity-90"
                      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                      onClick={() => navigate(`/workflow/editor?id=${def.id}`)}
                    >
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="text-base" style={{ flexShrink: 0 }}>{def.icon || '🔀'}</span>
                          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>{def.name}</span>
                          <span className="text-[10px] px-1.5 py-0.5 rounded" style={{
                            backgroundColor: def.enabled ? 'rgba(34,197,94,0.15)' : 'rgba(107,114,128,0.15)',
                            color: def.enabled ? '#22c55e' : 'var(--text-tertiary)',
                          }}>
                            {def.enabled ? '已启用' : '已禁用'}
                          </span>
                          {instCount > 0 && (
                            <span className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded" style={{
                              backgroundColor: 'rgba(59,130,246,0.1)',
                              color: '#3B82F6',
                            }}>
                              <Activity size={10} />
                              {instCount} 次执行
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-1">
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
                            <Upload size={14} />
                          </button>
                          <button
                            onClick={(e) => { e.stopPropagation(); handleDuplicate(def.id, def.name); }}
                            className="pd-btn p-1.5 rounded hover:opacity-80"
                            style={{ color: 'var(--text-secondary)' }}
                            title="复制"
                          >
                            <Copy size={14} />
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
                          <Upload size={10} />
                          {new Date(Number(def.updatedAt) * 1000).toLocaleString()}
                        </span>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
            {filteredDefs.length > 0 && (
              <div className="flex items-center justify-between mt-3 pt-3" style={{ borderTop: '1px solid var(--border)' }}>
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {hasDefFilter ? (
                    <>筛选结果 {filteredDefs.length} 条（共 {definitions.length} 条），第 {safeDefPage} / {totalDefPages} 页</>
                  ) : (
                    <>共 {filteredDefs.length} 条，第 {safeDefPage} / {totalDefPages} 页</>
                  )}
                </span>
                <div className="flex items-center gap-1">
                  <button
                    onClick={(e) => { e.stopPropagation(); setDefPage(1); }}
                    disabled={safeDefPage <= 1}
                    className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                    style={{
                      border: '1px solid var(--border)',
                      background: safeDefPage <= 1 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      color: safeDefPage <= 1 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                      cursor: safeDefPage <= 1 ? 'not-allowed' : 'pointer',
                      opacity: safeDefPage <= 1 ? 0.5 : 1,
                    }}
                  >首页</button>
                  <button
                    onClick={(e) => { e.stopPropagation(); setDefPage(p => Math.max(1, p - 1)); }}
                    disabled={safeDefPage <= 1}
                    className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                    style={{
                      border: '1px solid var(--border)',
                      background: safeDefPage <= 1 ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      color: safeDefPage <= 1 ? 'var(--text-tertiary)' : 'var(--text-primary)',
                      cursor: safeDefPage <= 1 ? 'not-allowed' : 'pointer',
                      opacity: safeDefPage <= 1 ? 0.5 : 1,
                    }}
                  >上一页</button>
                  <button
                    onClick={(e) => { e.stopPropagation(); setDefPage(p => Math.min(totalDefPages, p + 1)); }}
                    disabled={safeDefPage >= totalDefPages}
                    className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                    style={{
                      border: '1px solid var(--border)',
                      background: safeDefPage >= totalDefPages ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      color: safeDefPage >= totalDefPages ? 'var(--text-tertiary)' : 'var(--text-primary)',
                      cursor: safeDefPage >= totalDefPages ? 'not-allowed' : 'pointer',
                      opacity: safeDefPage >= totalDefPages ? 0.5 : 1,
                    }}
                  >下一页</button>
                  <button
                    onClick={(e) => { e.stopPropagation(); setDefPage(totalDefPages); }}
                    disabled={safeDefPage >= totalDefPages}
                    className="pd-btn px-2 py-1 text-[10px] rounded transition-colors"
                    style={{
                      border: '1px solid var(--border)',
                      background: safeDefPage >= totalDefPages ? 'var(--bg-tertiary)' : 'var(--bg-secondary)',
                      color: safeDefPage >= totalDefPages ? 'var(--text-tertiary)' : 'var(--text-primary)',
                      cursor: safeDefPage >= totalDefPages ? 'not-allowed' : 'pointer',
                      opacity: safeDefPage >= totalDefPages ? 0.5 : 1,
                    }}
                  >末页</button>
                </div>
              </div>
            )}
            </>)}

        {/* 待审批提示条 */}
        {pendingInputs.length > 0 && (
          <div
            className="p-3 rounded-lg mb-2"
            style={{ backgroundColor: 'rgba(245, 158, 11, 0.1)', border: '1px solid rgba(245, 158, 11, 0.3)' }}
          >
            <div className="flex items-center gap-2 mb-2">
              <UserCheck size={14} style={{ color: '#F59E0B' }} />
              <span className="text-xs font-medium" style={{ color: '#F59E0B' }}>
                待审批 ({pendingInputs.length})
              </span>
            </div>
            {pendingInputs.map((item) => (
              <PendingInputCard
                key={`${item.execution_id}-${item.node_id}`}
                item={item}
                onSubmit={(response) => respondHumanInput(item.execution_id, item.node_id, response)}
              />
            ))}
          </div>
        )}

        {activeTab === 'instances' && (
          <WorkflowMonitor
            onViewDefinition={(defId) => {
              selectDefinition(defId);
              navigate(`/workflow/editor?id=${defId}`);
            }}
            onDeleteExecution={(id, name) => handleDeleteExecution(id, name)}
          />
        )}

        {!loading && activeTab === 'stats' && (
          <ExecutionStats workflowId={undefined} />
        )}

        {activeTab === 'templates' && (
          <WorkflowTemplateMarket
            onBack={() => setActiveTab('definitions')}
            onUseTemplate={(tplId) => {
              showToast(`已安装模板 ${tplId}，可在"工作流定义"中查看`, 'success');
              setActiveTab('definitions');
            }}
          />
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

/** 待审批输入卡片 */
function PendingInputCard({ item, onSubmit }: { item: PendingHumanInput; onSubmit: (response: string) => void }) {
  const [value, setValue] = useState('');
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async () => {
    // 允许空字符串响应（避免节点卡死在等待输入状态）
    if (submitting) return;
    setSubmitting(true);
    try {
      await onSubmit(value);
      setValue('');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div
      className="p-2 rounded-md mb-2"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
    >
      <div className="text-[10px] mb-1" style={{ color: 'var(--text-tertiary)' }}>
        节点: {item.node_label}
      </div>
      <div className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>
        {item.prompt}
      </div>
      <div className="flex gap-2">
        {item.input_type === 'select' ? (
          <select
            value={value}
            onChange={(e) => setValue(e.target.value)}
            className="flex-1 text-xs px-2 py-1 rounded-md"
            style={{
              backgroundColor: 'var(--bg-primary)',
              border: '1px solid var(--border)',
              color: 'var(--text-primary)',
            }}
          >
            <option value="">请选择...</option>
            <option value="approve">通过</option>
            <option value="reject">拒绝</option>
          </select>
        ) : (
          <input
            type="text"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="请输入响应内容..."
            className="flex-1 text-xs px-2 py-1 rounded-md"
            style={{
              backgroundColor: 'var(--bg-primary)',
              border: '1px solid var(--border)',
              color: 'var(--text-primary)',
            }}
            onKeyDown={(e) => { if (e.key === 'Enter') handleSubmit(); }}
          />
        )}
        <button
          onClick={handleSubmit}
          disabled={submitting}
          className="text-xs px-3 py-1 rounded-md font-medium transition-colors"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
            cursor: 'pointer',
          }}
        >
          {submitting ? '提交中...' : '提交'}
        </button>
      </div>
    </div>
  );
}
