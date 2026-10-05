import { useState, useEffect, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { Plus, Trash2, Clock, Upload, Download, Settings, GitBranch, Activity, BarChart3, FileText, Tag, Layers, Zap, Copy, AlertTriangle, Search, Filter, X, ArrowUpDown, Calendar, LayoutTemplate, ScrollText, Play, Loader2, Square, Share2, Building2 } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { showToast, showLiveToastOnce } from '../utils/toast';
import { confirmDialog } from '../stores/confirmStore';
import { CAP_WORKFLOW_EXPORT_BATCH, useCapability, useAccountStore } from '../stores/accountStore';
import { TitleBar, StatusBar } from '../components/layout';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import { WorkflowPropertyDialog } from '../components/workflow/WorkflowPropertyDialog';
import { WorkflowInputDialog } from '../components/workflow/WorkflowInputDialog';
import { WorkflowMonitor } from '../components/workflow/WorkflowMonitor';
import { WorkflowOutputCard } from '../components/workflow/WorkflowOutputCard';
import { WorkflowNodeTimeline } from '../components/workflow/WorkflowNodeTimeline';
import type { NodeExecRow } from '../components/workflow/WorkflowNodeTimeline';
import { ExecutionStats } from '../components/workflow/ExecutionStats';
import { Select } from '../components/common/Select';
import type { WorkflowDefinition, WorkflowInstance, ExecutionProgressPayload } from '../types/workflow';
import type { JsonValue } from '../types/plugin';
import { useCommandCenterStore } from '../stores/commandCenterStore';
import { errorMessage } from '../utils/errorMessage';

interface WorkflowPageProps {
  onBack?: () => void;
  /** 嵌入主布局模式：不渲染自身 TitleBar/StatusBar（由外层提供） */
  embedded?: boolean;
}

/** 组织共享空间：我所属组织（与 Rust `org_list_mine` 返回对齐） */
interface MyOrg {
  id: number;
  name: string;
  role: string;
  planKey: string;
  planExpiresAt: string | null;
  seats: number;
  memberCount: number;
  status: string;
}

/** 组织的共享工作流列表项（与 Rust `org_list_shared_workflows` 返回对齐，不含 payload） */
interface SharedWorkflow {
  id: number;
  name: string;
  description: string;
  creatorLabel: string;
  updatedAt: string;
}

/** 将 Cron 表达式转换为用户友好描述 */
function describeCron(expr: string): string {
  if (!expr) return '定时';
  const parts = expr.trim().split(/\s+/);
  if (parts.length < 6) return expr;

  const [, min, hour, day, month, week] = parts;

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

/** 仍在进行中的实例状态（用于卡片上的"运行中"标记） */
const ACTIVE_INSTANCE_STATUSES = ['pending', 'running', 'paused'];

/** 已运行时长（卡片上的"运行中 · 12s"） */
function formatElapsed(startedAt?: number | null): string {
  if (!startedAt) return '进行中';
  const secs = Math.max(0, Math.floor(Date.now() / 1000 - Number(startedAt)));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m${secs % 60}s`;
  return `${Math.floor(mins / 60)}h${mins % 60}m`;
}

export function WorkflowPage({ embedded }: WorkflowPageProps) {
  // 批量导出是平台的受限能力：未解锁（未登录 / 免费档）不显示入口；单个导出对所有用户开放
  const canExportBatch = useCapability(CAP_WORKFLOW_EXPORT_BATCH);
  const navigate = useNavigate();
  const { definitions, instances, schedules, pendingInputs, pendingApprovals, loading, error, loadDefinitions, loadInstances, loadSchedules, loadPendingInputs, loadPendingApprovals, createDefinition, updateDefinition, deleteDefinition, deleteExecutions, selectDefinition } = useWorkflowStore();
  const [activeTab, setActiveTab] = useState<'definitions' | 'instances' | 'stats'>('definitions');
  const [showPropertyDialog, setShowPropertyDialog] = useState<'create' | 'edit' | null>(null);
  const [editingDef, setEditingDef] = useState<WorkflowDefinition | null>(null);
  /** 「查看结果」抽屉：直接给最近一次产出，省去跳实例页再翻记录 */
  const [resultDefId, setResultDefId] = useState<string | null>(null);
  /** 列表页「运行」：正在启动中的定义（按钮转圈）与待收参数的入参表单 */
  const [runningDefId, setRunningDefId] = useState<string | null>(null);
  const [stoppingDefId, setStoppingDefId] = useState<string | null>(null);
  const [inputDialogDef, setInputDialogDef] = useState<WorkflowDefinition | null>(null);
  /** 抽屉里那次执行的逐节点记录（事件派生：node/start + node/status + node/result） */
  const [nodeExecs, setNodeExecs] = useState<NodeExecRow[]>([]);
  const [nodeExecsLoading, setNodeExecsLoading] = useState(false);
  const [nodeExecsError, setNodeExecsError] = useState<string | null>(null);
  /** 待审批（不含失效的）+ 待人工输入 = 待处理入口上的计数（与指挥中心同一口径） */
  const pendingCount = pendingApprovals.filter((a) => !a.stale).length + pendingInputs.length;
  /** 批量导出弹窗：选中的工作流 id 集合 + 导出中标记（受限能力，见 canExportBatch） */
  const [showBatchExport, setShowBatchExport] = useState(false);
  const [batchSelectedIds, setBatchSelectedIds] = useState<Set<string>>(new Set());
  const [batchExporting, setBatchExporting] = useState(false);

  // ---- 组织共享空间（团队版）：共享到组织 / 从组织导入 ----
  /** 平台账号（null = 未登录）：用于未登录时的前置提示 */
  const account = useAccountStore((s) => s.account);
  /** 「共享到组织」弹窗：目标工作流 + 组织列表 + 共享名称 */
  const [shareTarget, setShareTarget] = useState<WorkflowDefinition | null>(null);
  const [shareOrgs, setShareOrgs] = useState<MyOrg[] | null>(null);
  const [shareOrgId, setShareOrgId] = useState('');
  const [shareName, setShareName] = useState('');
  const [shareLoading, setShareLoading] = useState(false);
  const [shareSubmitting, setShareSubmitting] = useState(false);
  const [shareError, setShareError] = useState('');
  /** 「从组织导入」弹窗：组织列表 + 该组织的共享工作流列表 */
  const [showImportOrg, setShowImportOrg] = useState(false);
  const [importOrgs, setImportOrgs] = useState<MyOrg[] | null>(null);
  const [importOrgId, setImportOrgId] = useState('');
  const [importItems, setImportItems] = useState<SharedWorkflow[] | null>(null);
  const [importItemsLoading, setImportItemsLoading] = useState(false);
  const [importItemId, setImportItemId] = useState<number | null>(null);
  const [importSubmitting, setImportSubmitting] = useState(false);
  const [importError, setImportError] = useState('');
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

  /** 「查看结果」抽屉的取材：最近一次**有产出**的执行；都没有则退回最近一次执行（用于展示空态原因） */
  const resultInstance = useMemo<WorkflowInstance | null>(() => {
    if (!resultDefId) return null;
    const list = instances
      .filter(i => i.definitionId === resultDefId)
      .sort((a, b) => Number(b.createdAt ?? 0) - Number(a.createdAt ?? 0));
    return list.find(i => i.output !== undefined && i.output !== null) ?? list[0] ?? null;
  }, [instances, resultDefId]);

  const resultDefName = definitions.find(d => d.id === resultDefId)?.name || '';

  // 逐节点记录：抽屉一打开就取（按 inspected 的执行 id 变化重新取）
  const resultExecutionId = resultInstance?.id;
  // 换执行 / 关抽屉时先把记录与错误归位：用「渲染期修正」（React adjust-during-render）而不是在
  // effect 里同步 setState（后者会多一轮级联渲染，且 `react-hooks/set-state-in-effect` 会报）。
  const [nodeExecsIdPrev, setNodeExecsIdPrev] = useState<string | undefined>(resultExecutionId);
  if (nodeExecsIdPrev !== resultExecutionId) {
    setNodeExecsIdPrev(resultExecutionId);
    setNodeExecs([]);
    setNodeExecsError(null);
    setNodeExecsLoading(Boolean(resultExecutionId));
  }

  useEffect(() => {
    if (!resultExecutionId) return;
    let aborted = false;
    invoke<NodeExecRow[]>('get_node_executions', { executionId: resultExecutionId })
      .then((rows) => { if (!aborted) setNodeExecs(rows || []); })
      .catch((err) => { if (!aborted) setNodeExecsError(errorMessage(err)); })
      .finally(() => { if (!aborted) setNodeExecsLoading(false); });
    return () => { aborted = true; };
  }, [resultExecutionId]);

  /** nodeId → 节点名/类型（来自当前定义；定义被改过或已删时退回节点 id） */
  const resultNodeMeta = useMemo<Record<string, { label: string; type: string }>>(() => {
    const map: Record<string, { label: string; type: string }> = {};
    const def = definitions.find(d => d.id === resultDefId);
    def?.stages?.forEach(stage => {
      stage.nodes?.forEach(node => {
        map[node.id] = { label: node.label || node.id, type: node.type };
      });
    });
    return map;
  }, [definitions, resultDefId]);

  // Esc 关闭结果弹窗（看长内容时不必再移动鼠标去找关闭按钮）
  useEffect(() => {
    if (!resultDefId) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setResultDefId(null); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [resultDefId]);

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

  // 筛选条件/排序变化时回到第 1 页：用「渲染期修正」（React adjust-during-render）而不是 effect ——
  // 在 effect 里同步 setState 会多一轮级联渲染，且 `react-hooks/set-state-in-effect` 会报。
  // 判据取各筛选态拼出的签名，变化即归零；本帧直接以第 1 页渲染，最终状态与原 effect 一致。
  const defFilterSignature = `${defFilterKeyword}|${defFilterEnabled}|${defFilterTrigger}|${defSortKey}|${defSortAsc}`;
  // 前值哨兵初值用 null（而非当帧签名）：筛选项在首帧就可能已有非默认值，
  // 用当帧值作初值会让判据第一帧就不成立，归零那次被静默丢掉。
  const [syncedFilterSignature, setSyncedFilterSignature] = useState<string | null>(null);
  if (syncedFilterSignature !== defFilterSignature) {
    setSyncedFilterSignature(defFilterSignature);
    setDefPage(1);
  }

  useEffect(() => {
    loadDefinitions();
    loadInstances();
    loadSchedules();
    loadPendingInputs();
    loadPendingApprovals();
    // 两类待办一起轮询：漏掉事件（页面重挂/事件早于订阅）也能在 5 秒内补齐
    const pendingInterval = setInterval(() => {
      loadPendingInputs();
      loadPendingApprovals();
    }, 5000);
    // 检测**中断的**执行（进程重启后残留的 running/paused；后台正在跑的已被后端按运行登记表排除）。
    //
    // 两点必须做对，否则就是刷屏：
    // ① 这段在每次进工作流页时都会跑（StrictMode 下还会双跑），用按 key 去重的现场提示，
    //    同一条残留只提示一次，而不是每进一次页面再弹一遍；
    // ② 它讲的是"发现的残留状态"，不是刚发生的事——失败本身已有权威通知
    //    （`notificationEvents` 的「xx 执行失败」），所以不进通知中心，只现场提示。
    invoke<Array<{ executionId: string; definitionName: string; status: string }>>('list_recoverable_executions')
      .then(recoverable => {
        if (recoverable.length > 0) {
          showLiveToastOnce(
            `recoverable:${recoverable.map(r => r.executionId).sort().join(',')}`,
            // 措辞按实际可做的操作写：这类实例还是 running，删除会被后端跳过（未结束不许删），
            // 得先补全执行、或先停止再清理。
            `有 ${recoverable.length} 个中断未完成的执行记录，可到「执行实例」补全执行或停止`,
            'warning',
          );
        }
      })
      .catch(() => {});
    return () => {
      clearInterval(pendingInterval);
    };
    // store 的 action 引用恒定（zustand 只创建一次），补进依赖后本 effect 仍等价于「挂载时跑一次」
  }, [loadDefinitions, loadInstances, loadSchedules, loadPendingInputs, loadPendingApprovals]);

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
    } catch (err) {
      console.error('导出工作流失败:', err);
      showToast(`导出工作流失败: ${errorMessage(err)}`, 'error');
    }
  };

  /** 批量导出：弹窗里勾选若干工作流，选目录后逐个写入独立子目录，返回成功数量 */
  const handleBatchExport = async () => {
    if (batchSelectedIds.size === 0) {
      showToast('请先勾选要导出的工作流', 'warning');
      return;
    }
    const ids = Array.from(batchSelectedIds);
    try {
      const dirPath = await openDialog({
        directory: true,
        title: '选择批量导出目录',
        defaultPath: '工作流批量导出',
      });
      if (!dirPath) return;
      setBatchExporting(true);
      const count = await invoke<number>('export_workflows_to_file', { ids, dirPath });
      showToast(`成功导出 ${count} 个工作流`, 'success');
      setShowBatchExport(false);
      setBatchSelectedIds(new Set());
    } catch (err) {
      console.error('批量导出工作流失败:', err);
      showToast(`批量导出失败: ${errorMessage(err)}`, 'error');
    } finally {
      setBatchExporting(false);
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
        } catch (innerErr) {
          const fileName = filePath.split(/[/]/).pop();
          showToast(`导入工作流「${fileName}」失败: ${innerErr}`, 'error');
        }
      }
      if (successCount > 0) {
        showToast(`成功导入 ${successCount} 个工作流`, 'success');
      }
      await loadDefinitions();
    } catch (err) {
      console.error('导入工作流失败:', err);
    }
  };

  const handleCreateAndEdit = () => {
    setEditingDef(null);
    setShowPropertyDialog('create');
  };

  // ── 组织共享空间（团队版）──────────────────────────────────────

  /** 「共享到组织…」：未登录先提示；否则拉取组织列表并打开弹窗 */
  const handleOpenShareDialog = async (e: React.MouseEvent, def: WorkflowDefinition) => {
    e.stopPropagation();
    if (!account) {
      showToast('请先登录平台账号', 'warning');
      return;
    }
    setShareTarget(def);
    setShareName(def.name);
    setShareOrgs(null);
    setShareOrgId('');
    setShareError('');
    setShareLoading(true);
    try {
      const orgs = await invoke<MyOrg[]>('org_list_mine');
      setShareOrgs(orgs);
      if (orgs.length === 0) {
        showToast('你还没有加入任何组织', 'warning');
      } else {
        setShareOrgId(String(orgs[0].id));
      }
    } catch (err) {
      setShareError(errorMessage(err));
      showToast(`获取组织列表失败：${errorMessage(err)}`, 'error');
    } finally {
      setShareLoading(false);
    }
  };

  /** 确认共享：调用后端生成导出 JSON 并 POST 到组织共享空间 */
  const handleConfirmShare = async () => {
    if (!shareTarget || !shareOrgId) return;
    setShareSubmitting(true);
    setShareError('');
    try {
      await invoke<SharedWorkflow>('org_share_workflow', {
        input: {
          workflowId: shareTarget.id,
          orgId: Number(shareOrgId),
          name: shareName.trim() || undefined,
        },
      });
      const orgName = (shareOrgs ?? []).find((o) => String(o.id) === shareOrgId)?.name ?? '组织';
      showToast(`已共享到「${orgName}」`, 'success');
      setShareTarget(null);
    } catch (err) {
      setShareError(errorMessage(err));
    } finally {
      setShareSubmitting(false);
    }
  };

  /** 加载某组织的共享工作流列表 */
  const loadOrgSharedWorkflows = async (orgId: number) => {
    setImportItemsLoading(true);
    setImportError('');
    setImportItems(null);
    setImportItemId(null);
    try {
      const items = await invoke<SharedWorkflow[]>('org_list_shared_workflows', { orgId });
      setImportItems(items);
    } catch (err) {
      setImportError(errorMessage(err));
    } finally {
      setImportItemsLoading(false);
    }
  };

  /** 「从组织导入…」：未登录先提示；否则拉取组织列表并打开弹窗 */
  const handleOpenImportOrgDialog = async () => {
    if (!account) {
      showToast('请先登录平台账号', 'warning');
      return;
    }
    setShowImportOrg(true);
    setImportOrgs(null);
    setImportOrgId('');
    setImportItems(null);
    setImportItemId(null);
    setImportError('');
    try {
      const orgs = await invoke<MyOrg[]>('org_list_mine');
      setImportOrgs(orgs);
      if (orgs.length === 0) {
        showToast('你还没有加入任何组织', 'warning');
      } else {
        setImportOrgId(String(orgs[0].id));
        await loadOrgSharedWorkflows(orgs[0].id);
      }
    } catch (err) {
      setImportError(errorMessage(err));
    }
  };

  /** 确认导入：取资源详情 → 后端新建本地工作流 → 刷新列表 */
  const handleImportFromOrg = async () => {
    if (!importOrgId || importItemId == null) return;
    setImportSubmitting(true);
    setImportError('');
    try {
      const res = await invoke<{ workflowId: string; name: string; subflowCount?: number }>('org_import_workflow', {
        input: { orgId: Number(importOrgId), resourceId: importItemId },
      });
      const subflowSuffix = res.subflowCount ? `（含 ${res.subflowCount} 个子流程）` : '';
      showToast(`已导入「${res.name}」${subflowSuffix}`, 'success');
      setShowImportOrg(false);
      await loadDefinitions();
    } catch (err) {
      setImportError(errorMessage(err));
    } finally {
      setImportSubmitting(false);
    }
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
    inputSchema?: Record<string, { type: string; description?: string; required?: boolean; default?: JsonValue }>;
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

  // ── 删除二次确认弹窗 ──

  /**
   * 监听执行进度（组件挂载时注册）。
   *
   * 两处历史包袱在这里一次修掉：
   * 1. 原先监听的 `workflow:execution-status` 后端**从未发出过**——引擎早已把
   *    node/stage/execution 三类事件合并成 `workflow:execution-progress`（见 engine.rs emit_progress），
   *    所以那段"执行结束提示 + 刷新实例"的代码一直是死监听；
   * 2. 定时任务执行后卡片上"上次执行"总显示"从未"：调度器写的是库里的 last_run_at，
   *    而前端只在页面挂载时读过一次排期——没有任何刷新时机。这里补上。
   */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    // 卸载与 listen() 的 Promise 是竞态：先卸载后 resolve 时，cleanup 拿到的还是 null，
    // 监听器就永远留着了。StrictMode 下每次挂载都双跑一次 effect，会稳定泄漏一个，
    // 于是同一帧被多份监听各推一次通知（"门控策略未通过"曾一次推 4 条就是这个原因）。
    let disposed = false;
    listen<ExecutionProgressPayload>('workflow:execution-progress', (event) => {
      const p = event.payload;
      // 只要带 execution.status（起始 running / 终态 completed·failed·cancelled）就刷新：
      // 实例记录在执行开始前就已落库，所以起始帧到达即可让卡片上的"运行中"立刻出现；
      // 排期（上次/下次执行）由调度器在执行前写好，同样靠这一步跟上。
      if (p.execution?.status) {
        useWorkflowStore.getState().loadInstances();
        useWorkflowStore.getState().loadSchedules();
      }
      // 阶段门控未通过：**只做现场提示**，不进通知中心历史。
      // 权威记录由执行终态那条通知承担（`notificationEvents` 的 "xx 执行失败"，detail 就是这里的原因），
      // 再记一条等于同一件事说两遍。按 执行+阶段 去重：同一帧被多份监听收到也只弹一次。
      if (p.stage?.status === 'gate_failed') {
        const detail = p.stage.reason || p.stage.error || '';
        showLiveToastOnce(
          `${p.execution_id}:gate:${p.stage.id}`,
          `${p.stage.name || '阶段'} 门控策略未通过${detail ? ': ' + detail : ''}`,
          'error',
        );
      }
    }).then(fn => {
      if (disposed) { fn(); return; }
      unlisten = fn;
    });

    return () => { disposed = true; unlisten?.(); };
  }, []);

  /**
   * 删工作流定义（二次确认走全局确认弹窗；后端在有未结束执行时会拒绝并给出提示）。
   */
  const handleDelete = async (id: string, name: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: `确定删除工作流「${name}」？此操作不可撤销，其已结束的执行记录、以及节点自动创建的内部会话与用量记录将一并删除。若仍有未结束的执行，需先停止后才能删除。`,
      confirmText: '删除',
    });
    if (!ok) return;
    try {
      const res = await deleteDefinition(id);
      showToast(
        res.deleted > 0
          ? `已删除工作流「${name}」，并清理 ${res.deleted} 条执行记录`
          : `已删除工作流「${name}」`,
        'success',
      );
    } catch (err) {
      showToast(`删除失败: ${errorMessage(err)}`, 'error');
    }
  };

  const handleDuplicate = async (id: string, name: string) => {
    try {
      await useWorkflowStore.getState().duplicateDefinition(id, name + ' (副本)');
    } catch (err) {
      console.error('复制失败:', err);
    }
  };

  /** 单个 / 批量删除执行记录共用入口（由 WorkflowMonitor 上抛选中集合与状态摘要） */
  const handleDeleteExecutions = async (executionIds: string[], summary: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: `确定删除选中的 ${executionIds.length} 条执行记录${summary ? `（${summary}）` : ''}？此操作不可撤销。关联的会话与磁盘工件不会被删除，但「统计」页指标会随之变化。`,
      confirmText: '删除',
    });
    if (!ok) return;
    try {
      const res = await deleteExecutions(executionIds);
      const skipped = res.skipped.length;
      showToast(
        skipped > 0
          ? `已删除 ${res.deleted} 条执行记录，${skipped} 条未结束已跳过`
          : `已删除 ${res.deleted} 条执行记录`,
        skipped > 0 ? 'warning' : 'success',
      );
    } catch (err) {
      showToast(`删除失败: ${errorMessage(err)}`, 'error');
    }
  };

  /**
   * 直接运行一个定义（列表页快捷入口）。
   *
   * 与编辑器里的「运行」同口径：先 `validate_workflow`，error 级直接拦下（否则只会跑出一个
   * 定义有错的实例），warning 级提示但不阻止；定义声明了 inputSchema 时先弹表单收参数
   * （顶层没有上游节点可以补齐入参）。
   */
  const startDefinitionRun = async (def: WorkflowDefinition, input?: Record<string, unknown>) => {
    if (runningDefId) return;
    setRunningDefId(def.id);
    try {
      const result = await invoke<{ ok: boolean; checks: Array<{ severity: string; message: string }> }>(
        'validate_workflow',
        { workflowId: def.id },
      );
      if (!result.ok) {
        const errors = result.checks.filter(c => c.severity === 'error').map(c => c.message).join('\n');
        showToast(`工作流验证失败：${errors}`, 'error');
        return;
      }
      const warnings = result.checks.filter(c => c.severity === 'warning');
      if (warnings.length > 0) {
        showToast(`验证提示：${warnings.map(c => c.message).join('; ')}`, 'warning');
      }
      await useWorkflowStore.getState().safeStartWorkflow(def.id, input);
      showToast(`已开始执行「${def.name}」`, 'success');
    } catch (err) {
      showToast(`执行失败: ${errorMessage(err)}`, 'error');
    } finally {
      setRunningDefId(null);
    }
  };

  const handleRunDefinition = (e: React.MouseEvent, def: WorkflowDefinition) => {
    e.stopPropagation();
    if (def.inputSchema && Object.keys(def.inputSchema).length > 0) {
      setInputDialogDef(def);
      return;
    }
    void startDefinitionRun(def);
  };

  /** 停掉该定义正在跑的实例：执行中被中断的节点本轮不再运行，已完成的产出留在这次记录里 */
  const handleStopDefinition = async (e: React.MouseEvent, def: WorkflowDefinition, instance: WorkflowInstance) => {
    e.stopPropagation();
    const ok = await confirmDialog({
      title: '停止执行',
      message: `确定停止「${def.name}」正在运行的实例？未执行的节点不会再运行，已完成的节点产出会保留在这次记录里。`,
      confirmText: '停止',
    });
    if (!ok) return;
    setStoppingDefId(def.id);
    try {
      await useWorkflowStore.getState().cancelWorkflow(instance.id);
      showToast('已停止执行', 'success');
    } catch (err) {
      showToast(`停止失败: ${errorMessage(err)}`, 'error');
    } finally {
      setStoppingDefId(null);
    }
  };

  // Computed pagination (moved outside JSX for oxc parser compatibility)
  const totalDefPages = Math.max(1, Math.ceil(filteredDefs.length / DEF_PAGE_SIZE));
  const safeDefPage = Math.min(defPage, totalDefPages);
  const paginatedDefs = filteredDefs.slice((safeDefPage - 1) * DEF_PAGE_SIZE, safeDefPage * DEF_PAGE_SIZE);

  // 数据变化导致当前页超出范围时回到末页：用「渲染期修正」（React adjust-during-render）而不是 effect ——
  // 在 effect 里同步 setState 会多一轮级联渲染，且 `react-hooks/set-state-in-effect` 会报。
  // 本帧已由 safeDefPage 兜底，渲染结果不变。
  if (defPage > totalDefPages) {
    setDefPage(totalDefPages);
  }

  /** 每个定义当前是否有正在进行的执行（卡片上的"运行中"标记） */
  const runningByDef = useMemo(() => {
    const map = new Map<string, WorkflowInstance>();
    for (const inst of instances) {
      if (ACTIVE_INSTANCE_STATUSES.includes(inst.status) && !map.has(inst.definitionId)) {
        map.set(inst.definitionId, inst);
      }
    }
    return map;
  }, [instances]);

  // 有执行在跑时每秒重渲染一次，让"运行中 · 12s"真的在走（没有运行中的实例就不挂定时器）
  const [, setElapsedTick] = useState(0);
  useEffect(() => {
    if (runningByDef.size === 0) return;
    const timer = setInterval(() => setElapsedTick((v) => v + 1), 1000);
    return () => clearInterval(timer);
  }, [runningByDef.size]);

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {!embedded && (
        <TitleBar
          showBackButton={true}
          titleText="工作流管理"
          onBack={() => navigate('/')}
          onOpenSettings={() => navigate('/settings')}
          onOpenMarket={() => navigate('/market')}
        />
      )}
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

        <div className="flex-1" />

        <div className="flex items-center gap-2 pb-2">
          {/* 待处理入口：审批卡与人工输入统一在指挥中心就地处理（这里只做入口与计数，
              避免同一批待办在页面底部再堆一份、位置还随 tab 漂移） */}
          {pendingCount > 0 && (
            <button
              onClick={() => useCommandCenterStore.getState().openCenter()}
              className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
              style={{ border: '1px solid #F59E0B', backgroundColor: 'rgba(245,158,11,0.1)', color: '#F59E0B' }}
              title="有待审批的工具调用或等待输入，点击到指挥中心处理"
            >
              <AlertTriangle size={14} />
              待处理 {pendingCount}
            </button>
          )}
          <button
            onClick={handleCreateAndEdit}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={14} /> 新建工作流
          </button>
          <button
            onClick={handleImportWorkflow}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <Download size={14} /> 从文件导入
          </button>
          {/* 从组织导入：走组织共享空间（团队版），与「共享到组织」配合使用 */}
          <button
            onClick={() => void handleOpenImportOrgDialog()}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="从组织共享空间导入工作流"
          >
            <Building2 size={14} /> 从组织导入
          </button>
          {/* 批量导出是平台的受限能力：未解锁（未登录 / 免费档）不显示入口 */}
          {canExportBatch && (
            <button
              onClick={() => {
                setBatchSelectedIds(new Set());
                setShowBatchExport(true);
              }}
              className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
              style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
              title="勾选多个工作流一次性导出到指定目录"
            >
              <Upload size={14} /> 批量导出
            </button>
          )}
          {/* 模板市场已迁至「资源市集 › 工作流模板」：这里只留跳转入口 */}
          <button
            onClick={() => navigate('/market?tab=workflow')}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            title="前往资源市集 › 工作流模板"
          >
            <LayoutTemplate size={14} /> 模板市场
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4 pd-scroll-stable">
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

                  {/* 启用状态药丸（容器固定 28px，与同行 Select(size=sm) 等高） */}
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
                      height: 28,
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

                  {/* 触发方式：平铺按钮组（下拉看不到"有哪几种"，平铺更直观，也与左侧启用状态同形） */}
                  <span className="inline-flex items-center gap-1 text-[11px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                    触发：
                  </span>
                  <div
                    aria-label="触发方式"
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 3,
                      flexWrap: 'nowrap',
                      padding: '2px 4px',
                      border: '1px solid var(--border)',
                      backgroundColor: 'var(--bg-primary)',
                      borderRadius: 4,
                      height: 28,
                      overflowX: 'auto',
                      scrollbarWidth: 'thin',
                    }}
                  >
                    {DEFINITION_TRIGGER_OPTIONS.map(opt => {
                      const active = defFilterTrigger === opt.key;
                      return (
                        <button
                          key={opt.key}
                          onClick={() => setDefFilterTrigger(opt.key)}
                          className="text-[10px] px-2 py-0.5 rounded-full transition-colors whitespace-nowrap"
                          style={{
                            border: active ? '1px solid var(--accent)' : '1px solid transparent',
                            backgroundColor: active ? 'var(--accent-light)' : 'transparent',
                            color: active ? 'var(--accent)' : 'var(--text-tertiary)',
                            fontWeight: active ? 500 : 400,
                            flexShrink: 0,
                          }}
                          title={`只看「${opt.label}」触发的工作流`}
                        >
                          {opt.label}
                        </button>
                      );
                    })}
                  </div>

                  {/* 排序 */}
                  <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                    <span className="inline-flex items-center gap-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                      <Calendar size={11} />
                      排序：
                    </span>
                    <Select
                      value={defSortKey}
                      onChange={v => setDefSortKey(v as typeof defSortKey)}
                      options={DEFINITION_SORT_OPTIONS.map(s => ({ value: s.key, label: s.label }))}
                      size="sm"
                      className="shrink-0"
                      style={{ minWidth: 96 }}
                      title="排序方式"
                    />
                    <button
                      onClick={() => setDefSortAsc(v => !v)}
                      className="pd-btn p-1 rounded transition-colors inline-flex items-center gap-0.5"
                      style={{
                        border: '1px solid var(--border)',
                        backgroundColor: defSortAsc ? 'var(--accent)' : 'var(--bg-primary)',
                        color: defSortAsc ? '#fff' : 'var(--text-secondary)',
                        height: 28,
                      }}
                      title={defSortAsc ? '降序' : '升序'}
                    >
                      <ArrowUpDown size={11} />
                      <span className="text-[10px] font-mono">{defSortAsc ? '↑' : '↓'}</span>
                    </button>
                  </div>

                  {/* 关键字搜索（高度对齐同行 Select(size=sm) 的 28px，不再靠内容撑高） */}
                  <div
                    className="pd-field"
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 4,
                      border: '1px solid var(--border)',
                      backgroundColor: 'var(--bg-primary)',
                      borderRadius: 4,
                      height: 28,
                      padding: '0 6px',
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
                    className="pd-btn px-2 py-1 h-7 text-[11px] rounded transition-colors whitespace-nowrap inline-flex items-center"
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
                          {/* 运行态：自动触发（定时/事件）的执行也在这里显形，不必再翻实例页 */}
                          {runningByDef.has(def.id) && (
                            <button
                              onClick={(e) => { e.stopPropagation(); setActiveTab('instances'); }}
                              className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-full transition-opacity hover:opacity-80"
                              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                              title="正在运行，点击查看执行实例"
                            >
                              <span
                                className="inline-block w-1.5 h-1.5 rounded-full animate-pulse"
                                style={{ backgroundColor: 'var(--accent)' }}
                              />
                              运行中 · {formatElapsed(runningByDef.get(def.id)?.startedAt)}
                            </button>
                          )}
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
                          {(() => {
                            // 有实例在跑就变「停止」：同一位置一个按钮表达"开始/结束这件事"
                            const runningInst = runningByDef.get(def.id);
                            if (runningInst) {
                              const stopping = stoppingDefId === def.id;
                              return (
                                <button
                                  onClick={(e) => void handleStopDefinition(e, def, runningInst)}
                                  disabled={stopping}
                                  className="pd-btn p-1.5 rounded hover:opacity-80"
                                  style={{ color: '#EF4444', cursor: stopping ? 'default' : 'pointer' }}
                                  title={stopping ? '正在停止…' : '停止执行'}
                                >
                                  {stopping ? <Loader2 size={14} className="animate-spin" /> : <Square size={14} />}
                                </button>
                              );
                            }
                            const starting = runningDefId === def.id;
                            return (
                              <button
                                onClick={(e) => handleRunDefinition(e, def)}
                                disabled={starting}
                                className="pd-btn p-1.5 rounded hover:opacity-80"
                                style={{ color: 'var(--accent)', cursor: starting ? 'default' : 'pointer' }}
                                title={starting ? '正在启动…' : '运行此工作流'}
                              >
                                {starting ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} />}
                              </button>
                            );
                          })()}
                          {instCount > 0 && (
                            <button
                              onClick={(e) => { e.stopPropagation(); setResultDefId(def.id); }}
                              className="pd-btn p-1.5 rounded hover:opacity-80"
                              style={{ color: 'var(--text-secondary)' }}
                              title="查看最近一次执行结果"
                            >
                              <ScrollText size={14} />
                            </button>
                          )}
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
                            onClick={(e) => { void handleOpenShareDialog(e, def); }}
                            className="pd-btn p-1.5 rounded hover:opacity-80"
                            style={{ color: 'var(--text-secondary)' }}
                            title="共享到组织"
                          >
                            <Share2 size={14} />
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
                            if (t.triggerType === 'cron') {
                              // 排期信息直接读出来挂在卡片上：以前要切到「定时任务」页才看得到
                              // 上次/下次执行，改配置反而得多跳一次 tab
                              const s = schedules.find(sc => sc.workflowId === def.id);
                              const last = s?.lastRunAt ? new Date(Number(s.lastRunAt) * 1000).toLocaleString() : '从未';
                              const next = s?.nextRunAt ? new Date(Number(s.nextRunAt) * 1000).toLocaleString() : '—';
                              return `定时（${describeCron(t.cron || '')}，上次执行: ${last}，下次执行: ${next}）`;
                            }
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

        {activeTab === 'instances' && (
          <WorkflowMonitor
            onViewDefinition={(defId) => {
              selectDefinition(defId);
              navigate(`/workflow/editor?id=${defId}`);
            }}
            onDeleteExecutions={handleDeleteExecutions}
          />
        )}

        {!loading && activeTab === 'stats' && (
          <ExecutionStats workflowId={undefined} />
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

      {/* 运行前入参表单（定义声明了 inputSchema 时）：与编辑器里的「运行」同一流程 */}
      {inputDialogDef?.inputSchema && (
        <WorkflowInputDialog
          definitionName={inputDialogDef.name}
          schema={inputDialogDef.inputSchema}
          onRun={(input) => {
            const target = inputDialogDef;
            setInputDialogDef(null);
            void startDefinitionRun(target, input);
          }}
          onClose={() => setInputDialogDef(null)}
        />
      )}

      {/* 批量导出弹窗：列出全部工作流定义并勾选（受限能力入口） */}
      {showBatchExport && (
        <div
          className="fixed inset-0 z-[120] flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => { if (!batchExporting) setShowBatchExport(false); }}
        >
          <div
            className="rounded-xl shadow-xl w-full mx-4 flex flex-col"
            style={{ maxWidth: 420, maxHeight: '70vh', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 text-sm font-medium shrink-0" style={{ color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}>
              批量导出工作流
            </div>
            <div className="px-4 py-2 flex items-center gap-2 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
              <label className="flex items-center gap-1.5 text-xs cursor-pointer" style={{ color: 'var(--text-secondary)' }}>
                <input
                  type="checkbox"
                  checked={definitions.length > 0 && batchSelectedIds.size === definitions.length}
                  onChange={(e) => {
                    setBatchSelectedIds(e.target.checked ? new Set(definitions.map((d) => d.id)) : new Set());
                  }}
                />
                全选
              </label>
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                已选 {batchSelectedIds.size} / {definitions.length}
              </span>
            </div>
            <div className="flex-1 overflow-y-auto px-4 py-2">
              {definitions.length === 0 ? (
                <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>暂无工作流</div>
              ) : (
                definitions.map((d) => (
                  <label
                    key={d.id}
                    className="flex items-center gap-2 py-1.5 text-xs cursor-pointer"
                    style={{ color: 'var(--text-primary)' }}
                  >
                    <input
                      type="checkbox"
                      checked={batchSelectedIds.has(d.id)}
                      onChange={(e) => {
                        setBatchSelectedIds((prev) => {
                          const next = new Set(prev);
                          if (e.target.checked) next.add(d.id);
                          else next.delete(d.id);
                          return next;
                        });
                      }}
                    />
                    <span className="truncate" title={d.name}>{d.name}</span>
                    <span className="ml-auto text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                      {d.stages?.length || 0} 阶段
                    </span>
                  </label>
                ))
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
              <button
                onClick={() => setShowBatchExport(false)}
                disabled={batchExporting}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)', cursor: batchExporting ? 'default' : 'pointer' }}
              >
                取消
              </button>
              <button
                onClick={() => void handleBatchExport()}
                disabled={batchExporting || batchSelectedIds.size === 0}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: batchExporting || batchSelectedIds.size === 0 ? 0.5 : 1,
                  cursor: batchExporting || batchSelectedIds.size === 0 ? 'default' : 'pointer',
                }}
              >
                {batchExporting ? '导出中…' : `导出所选（${batchSelectedIds.size}）`}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 「共享到组织」弹窗：选择组织 + 可选共享名称，走后端 org_share_workflow */}
      {shareTarget && (
        <div
          className="fixed inset-0 z-[120] flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => { if (!shareSubmitting) setShareTarget(null); }}
        >
          <div
            className="rounded-xl shadow-xl w-full mx-4 flex flex-col"
            style={{ maxWidth: 440, maxHeight: '70vh', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 text-sm font-medium shrink-0" style={{ color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}>
              共享到组织
            </div>
            <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
              <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                工作流：<span style={{ color: 'var(--text-primary)' }}>{shareTarget.name}</span>
              </div>
              {shareLoading ? (
                <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>正在获取组织列表…</div>
              ) : shareOrgs && shareOrgs.length === 0 ? (
                <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>你还没有加入任何组织</div>
              ) : (
                <>
                  <div>
                    <div className="text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>选择组织</div>
                    <Select
                      value={shareOrgId}
                      onChange={setShareOrgId}
                      placeholder="请选择组织"
                      options={(shareOrgs ?? []).map((o) => ({ value: String(o.id), label: o.name }))}
                      className="w-full"
                    />
                  </div>
                  <div>
                    <div className="text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>共享名称（默认工作流名称）</div>
                    <input
                      value={shareName}
                      onChange={(e) => setShareName(e.target.value)}
                      className="w-full px-2 py-1.5 text-xs rounded"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                      placeholder={shareTarget.name}
                    />
                  </div>
                </>
              )}
              {shareError && (
                <div className="text-xs px-2 py-1.5 rounded" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
                  {shareError}
                </div>
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
              <button
                onClick={() => setShareTarget(null)}
                disabled={shareSubmitting}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)', cursor: shareSubmitting ? 'default' : 'pointer' }}
              >
                取消
              </button>
              <button
                onClick={() => void handleConfirmShare()}
                disabled={shareSubmitting || shareLoading || !shareOrgId}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: shareSubmitting || shareLoading || !shareOrgId ? 0.5 : 1,
                  cursor: shareSubmitting || shareLoading || !shareOrgId ? 'default' : 'pointer',
                }}
              >
                {shareSubmitting ? '共享中…' : '确认共享'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 「从组织导入」弹窗：选组织 → 加载共享工作流列表 → 选中一条导入 */}
      {showImportOrg && (
        <div
          className="fixed inset-0 z-[120] flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => { if (!importSubmitting) setShowImportOrg(false); }}
        >
          <div
            className="rounded-xl shadow-xl w-full mx-4 flex flex-col"
            style={{ maxWidth: 480, maxHeight: '74vh', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 text-sm font-medium shrink-0" style={{ color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}>
              从组织导入工作流
            </div>
            <div className="px-4 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
              <div className="text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>选择组织</div>
              {importOrgs && importOrgs.length > 0 ? (
                <Select
                  value={importOrgId}
                  onChange={(v) => {
                    setImportOrgId(v);
                    void loadOrgSharedWorkflows(Number(v));
                  }}
                  placeholder="请选择组织"
                  options={importOrgs.map((o) => ({ value: String(o.id), label: o.name }))}
                  className="w-full"
                  disabled={importSubmitting}
                />
              ) : (
                <div className="text-xs py-2 text-center" style={{ color: 'var(--text-tertiary)' }}>
                  {importOrgs === null ? '正在获取组织列表…' : '你还没有加入任何组织'}
                </div>
              )}
            </div>
            <div className="flex-1 overflow-y-auto px-4 py-2">
              {importItemsLoading ? (
                <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>正在加载共享工作流…</div>
              ) : importItems && importItems.length === 0 ? (
                <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>该组织暂无共享工作流</div>
              ) : (
                (importItems ?? []).map((item) => (
                  <label
                    key={item.id}
                    className="flex items-start gap-2 py-2 text-xs cursor-pointer"
                    style={{ color: 'var(--text-primary)' }}
                  >
                    <input
                      type="radio"
                      name="org-shared-workflow"
                      className="mt-0.5"
                      checked={importItemId === item.id}
                      onChange={() => setImportItemId(item.id)}
                      disabled={importSubmitting}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate" title={item.name}>{item.name}</span>
                      <span className="block text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                        {item.creatorLabel || '未知'} · {item.updatedAt ? new Date(item.updatedAt).toLocaleString() : ''}
                      </span>
                    </span>
                  </label>
                ))
              )}
              {importError && (
                <div className="text-xs px-2 py-1.5 rounded my-2" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
                  {importError}
                </div>
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
              <button
                onClick={() => setShowImportOrg(false)}
                disabled={importSubmitting}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)', cursor: importSubmitting ? 'default' : 'pointer' }}
              >
                取消
              </button>
              <button
                onClick={() => void handleImportFromOrg()}
                disabled={importSubmitting || importItemId == null}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: importSubmitting || importItemId == null ? 0.5 : 1,
                  cursor: importSubmitting || importItemId == null ? 'default' : 'pointer',
                }}
              >
                {importSubmitting ? '导入中…' : '导入'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 「查看结果」悬浮侧栏：带外边距、四角圆角，不再是满屏抽屉也不会盖住整页 */}
      {resultDefId && (
        <div
          className="fixed inset-0 z-40"
          style={{ backgroundColor: 'rgba(0,0,0,0.3)' }}
          onClick={() => setResultDefId(null)}
        />
      )}
      {resultDefId && (
        <div
          className="fixed right-4 top-4 bottom-4 z-50 flex flex-col rounded-xl shadow-2xl overflow-hidden"
          style={{ width: 'min(920px, 92vw)', backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        >
          {/* Header：定义名 + 本次执行概览 */}
          <div
            className="flex items-center gap-2 px-4 py-3 shrink-0"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <ScrollText size={16} style={{ color: 'var(--accent)', flexShrink: 0 }} />
            <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)', maxWidth: 300 }}>
              {resultDefName || '最近一次结果'}
            </span>
            {resultInstance && (
              <span className="flex items-center gap-2 text-[10px] min-w-0" style={{ color: 'var(--text-tertiary)' }}>
                <span
                  className="px-1.5 py-0.5 rounded-full"
                  style={{
                    backgroundColor: resultInstance.status === 'success' ? 'rgba(16,185,129,0.12)' : resultInstance.status === 'failed' ? 'rgba(239,68,68,0.12)' : 'var(--bg-tertiary)',
                    color: resultInstance.status === 'success' ? '#10B981' : resultInstance.status === 'failed' ? '#EF4444' : 'var(--text-tertiary)',
                  }}
                >
                  {({ success: '成功', failed: '失败', cancelled: '已取消', timeout: '超时', running: '运行中', paused: '已暂停', pending: '待触发' } as Record<string, string>)[resultInstance.status] || resultInstance.status}
                </span>
                <span>{({ manual: '手动', cron: '定时', event: '事件' } as Record<string, string>)[resultInstance.trigger] || resultInstance.trigger}</span>
                <span className="truncate">
                  {resultInstance.completedAt ? new Date(Number(resultInstance.completedAt) * 1000).toLocaleString() : '未结束'}
                </span>
              </span>
            )}
            <span className="flex-1" />
            <button
              onClick={() => setResultDefId(null)}
              className="pd-btn p-1 rounded hover:opacity-80 shrink-0"
              style={{ color: 'var(--text-tertiary)' }}
              title="关闭（Esc）"
            >
              <X size={14} />
            </button>
          </div>

          {resultInstance ? (
            <>
              <div className="flex-1 overflow-y-auto px-4 py-3.5 space-y-4">
                <WorkflowOutputCard instance={resultInstance} compact />
                <div>
                  <div className="flex items-center gap-2 mb-2">
                    <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>执行记录</span>
                    <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      共 {nodeExecs.length} 个节点，按执行顺序
                    </span>
                  </div>
                  <WorkflowNodeTimeline
                    rows={nodeExecs}
                    meta={resultNodeMeta}
                    loading={nodeExecsLoading}
                    error={nodeExecsError}
                    finalOutput={resultInstance.output}
                  />
                </div>
              </div>
              <div className="px-4 py-2.5 shrink-0 flex items-center justify-between" style={{ borderTop: '1px solid var(--border)' }}>
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  这是该工作流最近一次有产出的执行
                </span>
                <button
                  onClick={() => { setResultDefId(null); setActiveTab('instances'); }}
                  className="pd-btn text-[11px] px-2.5 py-1 rounded"
                  style={{ border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
                >
                  查看全部执行记录
                </button>
              </div>
            </>
          ) : (
            <div className="flex-1 flex items-center justify-center text-xs py-16" style={{ color: 'var(--text-tertiary)' }}>
              还没有执行记录
            </div>
          )}
        </div>
      )}

      {!embedded && (
        <StatusBar
          onOpenSettings={() => navigate('/settings')}
          onOpenEnvSettings={() => navigate('/settings?tab=environment')}
        />
      )}
    </div>
  );
}
