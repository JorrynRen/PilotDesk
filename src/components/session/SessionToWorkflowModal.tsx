// 会话 → 工作流弹窗：打开时拉取转换候选清单（会话计划 + 会话消息，and 合并）做预览，
// 清单可勾选/可编辑（本地清单是唯一事实来源），确认后连同清单一起回传后端落库并交由调用方跳转编辑器。
// 清单有两个互斥来源（两态切换，两边都可自由编辑）：来源清单（export 候选）/ 模型提炼（后端专用调用）。
// 转换规则在后端；前端只负责清单编辑、预览刷新、报错与确认。

import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Plus, Workflow, X } from 'lucide-react';
import {
  useSessionStore,
  type PlanCandidate,
  type PlanCandidateOrigin,
  type PlanTaskInput,
  type SessionPlanSource,
} from '../../stores/sessionStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import type { WorkflowDefinition } from '../../types/workflow';

interface SessionToWorkflowModalProps {
  open: boolean;
  sessionId: string;
  /** 当前会话标题（写进说明文案，让用户确认转换对象没弄错） */
  sessionTitle: string;
  onClose: () => void;
  /** 落库成功回调：参数为新定义 id（由调用方负责跳转工作流编辑器） */
  onCreated: (definitionId: string) => void;
}

/** 清单来源模式：来源清单（export 候选）/ 模型提炼（session_extract_tasks） */
type ListMode = 'source' | 'ai';

/** 本地清单行（仅前端用；export 结果只在初始化清单时用于填充，之后不再回写） */
interface EditableTask {
  id: string;
  title: string;
  detail: string;
  /** 后端给的"建议默认勾选"（false = 被自动忽略的确认/追问） */
  suggested: boolean;
  checked: boolean;
  /** 候选来源分组（新增的手工行没有来源，不展示标签） */
  origin?: PlanCandidateOrigin;
  /**
   * 前置依赖的**本地行 id**（不存序号：增删/调序后显示序号会变，提交时再换算成 1-based 位置）；
   * 空数组 = 无依赖（可并行）
   */
  deps: string[];
  /**
   * 确认类任务（需要用户拍板才能继续）：转成工作流时生成「人工交互」节点，
   * 运行到该节点会挂起等用户作答（答复随 result 流向下游），而不是生成空转的 Agent 节点
   */
  confirm: boolean;
  /** 确认项的默认值：无人应答（超时）时交互节点按它继续 —— 自动运行场景下不空转的关键 */
  defaultValue: string;
}

/** 来源小标签：仅 todos/messages/ai 展示（edited 是回传口径，不展示） */
const ORIGIN_LABEL: Partial<Record<PlanCandidateOrigin, string>> = {
  todos: '计划',
  messages: '消息',
  ai: 'AI',
};

/** 本地行 id（不参与提交，仅用于 React key 与焦点定位） */
function genId(): string {
  return `t-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/** 候选清单 → 本地清单（suggested 决定默认勾选；不按 origin 过滤，被忽略的候选也照列） */
function toEditableTasks(tasks: PlanCandidate[] | undefined): EditableTask[] {
  return (tasks ?? []).map((t) => ({
    id: genId(),
    title: t.title,
    detail: t.detail,
    suggested: t.suggested,
    checked: t.suggested,
    origin: t.origin,
    deps: [],
    confirm: t.confirm ?? false,
    defaultValue: t.defaultValue ?? '',
  }));
}

/** 按当前顺序把清单串成链（第 i 行依赖第 i-1 行）；依赖已由用户/模型显式设置时不调用 */
function chainDeps(list: EditableTask[]): EditableTask[] {
  return list.map((t, i) => ({ ...t, deps: i === 0 ? [] : [list[i - 1].id] }));
}

/**
 * 新增依赖前的成环判定：`fromId` 是否直接或间接依赖 `toId`。
 * 成立说明"让 `toId` 依赖 `fromId`"会成环，本次勾选应被拒绝。
 */
function dependsOn(list: EditableTask[], fromId: string, toId: string): boolean {
  const seen = new Set<string>();
  const stack = [fromId];
  while (stack.length > 0) {
    const id = stack.pop() as string;
    if (seen.has(id)) continue;
    if (id === toId) return true;
    seen.add(id);
    const row = list.find((t) => t.id === id);
    if (row) stack.push(...row.deps);
  }
  return false;
}

/** 候选中实际出现的来源分组（用于来源说明行） */
function originGroups(tasks: PlanCandidate[] | undefined): PlanCandidateOrigin[] {
  const set = new Set<PlanCandidateOrigin>();
  for (const t of tasks ?? []) if (t.origin) set.add(t.origin);
  return [...set];
}

/**
 * 转换来源说明：候选来源是 and 合并（计划 + 消息并列），按实际出现的分组表述；
 * source 仅作兜底（后端在"提交覆盖"时返回 edited）。
 */
function sourceNote(source: SessionPlanSource, ignoredCount: number, origins: PlanCandidateOrigin[]): string {
  const ignoredNote = ignoredCount > 0
    ? `（已自动忽略 ${ignoredCount} 条确认与追问，可勾选回来）`
    : '';
  if (origins.includes('ai')) return '任务来源：模型提炼（可继续编辑）';
  if (origins.includes('todos') && origins.includes('messages')) {
    return `任务来源：会话计划 + 会话消息${ignoredNote}`;
  }
  if (origins.includes('todos')) return '任务来源：会话计划';
  if (origins.includes('messages')) return `任务来源：会话消息${ignoredNote}`;
  if (source === 'edited') return '任务来源：已编辑';
  return '';
}

export function SessionToWorkflowModal({ open, sessionId, sessionTitle, onClose, onCreated }: SessionToWorkflowModalProps) {
  const exportWorkflow = useSessionStore((s) => s.exportWorkflow);
  const promoteWorkflow = useSessionStore((s) => s.promoteWorkflow);
  const extractTasks = useSessionStore((s) => s.extractTasks);

  const [definition, setDefinition] = useState<WorkflowDefinition | null>(null);
  const [source, setSource] = useState<SessionPlanSource | null>(null);
  const [ignoredCount, setIgnoredCount] = useState(0);
  /** 本会话里用户已确认过的需求条数（答复已并入总目标，运行期无需再确认） */
  const [confirmedCount, setConfirmedCount] = useState(0);
  /** 当前候选清单里实际出现的来源分组（模式初始化时确定，编辑不改变来源说明） */
  const [origins, setOrigins] = useState<PlanCandidateOrigin[]>([]);
  /** 本地任务清单：唯一事实来源 */
  const [list, setList] = useState<EditableTask[]>([]);
  /** 工作流名称：首次导出成功后由 definition.name 预填，此后由本地 state 掌管（预览刷新不覆盖） */
  const [wfName, setWfName] = useState('');
  /** 工作流描述：口径同上，同时它就是展示给用户的"工作流说明" */
  const [wfDescription, setWfDescription] = useState('');
  /** 清单来源模式：来源清单（默认）/ 模型提炼 —— 两态互斥，仅决定清单从哪来 */
  const [listMode, setListMode] = useState<ListMode>('source');
  const [loading, setLoading] = useState(false);
  /** 「来源清单」重新导出中（不阻塞编辑） */
  const [switching, setSwitching] = useState(false);
  /** 「模型提炼」请求中 */
  const [extracting, setExtracting] = useState(false);
  /** 清单改动后的预览刷新中（不阻塞编辑） */
  const [refreshing, setRefreshing] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showJson, setShowJson] = useState(false);
  /** 待聚焦的行（新增任务后聚焦到它） */
  const [focusId, setFocusId] = useState<string | null>(null);
  /** 当前展开依赖面板的行 id（行内就地展开，不用 portal） */
  const [openDepsFor, setOpenDepsFor] = useState<string | null>(null);
  /** 最近一次被拒绝的成环勾选所在行 id（行下方红字提示） */
  const [cycleHintId, setCycleHintId] = useState<string | null>(null);

  /** 用户是否改过清单：未改动前不做防抖重导出（避免来源被无谓刷成"已编辑"） */
  const dirtyRef = useRef(false);
  /**
   * 依赖是否已被用户/模型显式设置：false = 顺序调整时重建链（顺序仍紧凑）；
   * true = 保持既有依赖不变（尊重用户/模型的显式设置）
   */
  const depsTouchedRef = useRef(false);
  /** 事件处理器里读取最新清单（成环判定等需在 setList 之外同步计算） */
  const listRef = useRef<EditableTask[]>([]);
  /** 仅用于"最新一次预览请求"的竞态保护 */
  const reqSeqRef = useRef(0);
  const inputRefs = useRef(new Map<string, HTMLInputElement>());

  // 提交清单：只有勾选且有标题的行参与转换（detail 缺失时退回标题）；
  // deps 记录的是行 id，这里换算成**提交数组内的 1-based 位置**（依赖项未勾选/被删则丢弃，并排除自依赖）
  const submitTasks = useMemo<PlanTaskInput[]>(
    () => {
      const visible = list.filter((t) => t.checked && t.title.trim());
      const idToPos = new Map(visible.map((t, i) => [t.id, i + 1]));
      return visible.map((t) => ({
        title: t.title,
        detail: t.detail || t.title,
        confirm: t.confirm,
        // 默认值只对确认类任务有意义：留空传 null（后端不写 defaultValue，超时仍取空）
        defaultValue: t.confirm && t.defaultValue.trim() ? t.defaultValue.trim() : null,
        deps: t.deps
          .map((id) => idToPos.get(id))
          .filter((p): p is number => p !== undefined && p !== idToPos.get(t.id)),
      }));
    },
    [list],
  );
  const submitKey = useMemo(() => JSON.stringify(submitTasks), [submitTasks]);
  // 防抖回调里读最新提交清单，避免闭包过期。用 effect 同步 ref：渲染期写 ref 会被 `react-hooks/refs` 判违规
  const submitRef = useRef(submitTasks);
  useEffect(() => { submitRef.current = submitTasks; }, [submitTasks]);
  useEffect(() => { listRef.current = list; }, [list]);

  // 每次打开都重新导出：会话内容可能已变化；用返回的候选清单初始化本地清单（此后不再回写）。
  // 状态重置用「渲染期修正」（React adjust-during-render）而不是 effect 内同步 setState —— 后者会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），两者最终渲染结果一致。
  // 初始用 undefined 哨兵，保证"挂载即打开"时也重置一次（与原 effect 首跑对齐）。
  const openKey = open ? sessionId : null;
  const [openKeyPrev, setOpenKeyPrev] = useState<string | null | undefined>(undefined);
  if (openKeyPrev !== openKey) {
    setOpenKeyPrev(openKey);
    if (openKey !== null) {
      setDefinition(null);
      setSource(null);
      setIgnoredCount(0);
      setConfirmedCount(0);
      setOrigins([]);
      setList([]);
      setWfName('');
      setWfDescription('');
      setOpenDepsFor(null);
      setCycleHintId(null);
      setListMode('source');
      setSwitching(false);
      setExtracting(false);
      setRefreshing(false);
      setError(null);
      setShowJson(false);
      setLoading(true);
    }
  }

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // ref 只能在 effect 里重置（渲染期写 ref 会被 `react-hooks/refs` 判违规）
    dirtyRef.current = false;
    reqSeqRef.current += 1;
    depsTouchedRef.current = false;
    (async () => {
      try {
        const res = await exportWorkflow(sessionId, null);
        if (cancelled) return;
        setDefinition(res.definition);
        setSource(res.source);
        setIgnoredCount(res.ignoredCount ?? 0);
        setConfirmedCount(res.confirmedCount ?? 0);
        setOrigins(originGroups(res.tasks));
        setList(chainDeps(toEditableTasks(res.tasks)));
        // 名称/描述只在首次导出时预填（后端默认值），之后不再回写，避免覆盖用户输入
        setWfName(res.definition.name ?? '');
        setWfDescription(res.definition.description ?? '');
      } catch (err) {
        if (!cancelled) {
          const message = errorMessage(err);
          setError(message);
          showToast(message, 'error');
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [open, sessionId, exportWorkflow]);

  // 勾选/编辑/增删后（防抖 400ms）用当前提交清单再导出一次，只刷新预览，不触碰本地清单
  // （来源说明不随之变化：它描述的是候选来源分组，不因用户编辑而变）
  // 名称/描述改动同样走这里：保证预览 JSON 里的 name/description 与输入一致
  useEffect(() => {
    if (!open || !dirtyRef.current) return;
    const tasks = submitRef.current;
    // 空清单后端会直接拒绝（弹窗内已有提示且禁用创建），这里静默跳过
    if (tasks.length === 0) return;
    const timer = setTimeout(async () => {
      const seq = ++reqSeqRef.current;
      setRefreshing(true);
      try {
        const res = await exportWorkflow(sessionId, tasks, wfName.trim() || null, wfDescription);
        if (seq !== reqSeqRef.current) return;
        setDefinition(res.definition);
      } catch (err) {
        // 预览刷新失败不打断编辑：保留上一份预览，提交时后端错误仍会展示
        console.warn('Failed to refresh workflow preview:', err);
      } finally {
        if (seq === reqSeqRef.current) setRefreshing(false);
      }
    }, 400);
    return () => clearTimeout(timer);
  }, [open, sessionId, exportWorkflow, submitKey, wfName, wfDescription]);

  // 新增行后聚焦到它（等新行挂载完成）
  useEffect(() => {
    if (!focusId) return;
    // 推到微任务：effect 体内同步 setState 会被 `react-hooks/set-state-in-effect` 判为级联渲染。
    // 同一个任务、早于绘制，行为一致（新行的 input 在本次提交已挂载，ref 已登记）。
    void Promise.resolve().then(() => {
      inputRefs.current.get(focusId)?.focus();
      setFocusId(null);
    });
  }, [focusId]);

  // Esc 关闭（创建落库过程中不响应，避免半途打断）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !creating) onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, creating, onClose]);

  function toggleTask(id: string) {
    dirtyRef.current = true;
    setList((l) => {
      const next = l.map((t) => (t.id === id ? { ...t, checked: !t.checked } : t));
      // 未勾选的任务不参与依赖：取消勾选时把它从别人依赖里清掉；
      // 同时任何一行的依赖里若指向"当前未勾选"的行，也一并清掉（未勾选者不能作为前置）
      const checkedIds = new Set(next.filter((t) => t.checked).map((t) => t.id));
      return next.map((t) => {
        const kept = t.deps.filter((d) => checkedIds.has(d) && d !== t.id);
        return kept.length === t.deps.length ? t : { ...t, deps: kept };
      });
    });
  }

  function editTitle(id: string, title: string) {
    dirtyRef.current = true;
    setList((l) => l.map((t) => (t.id === id ? { ...t, title } : t)));
  }

  /** 切换任务类型：Agent 任务 ⇄ 确认类任务（后者转成「人工交互」节点，运行期挂起等用户作答） */
  function toggleConfirm(id: string) {
    dirtyRef.current = true;
    setList((l) => l.map((t) => (t.id === id ? { ...t, confirm: !t.confirm } : t)));
  }

  /** 确认项的默认值：无人应答（超时）时交互节点按它继续，自动运行场景下不空转 */
  function editDefaultValue(id: string, value: string) {
    dirtyRef.current = true;
    setList((l) => l.map((t) => (t.id === id ? { ...t, defaultValue: value } : t)));
  }

  /** 名称/描述由用户改动：同样置脏，让 400ms 防抖预览带上新值（预览即落库口径） */
  function editWfName(value: string) {
    dirtyRef.current = true;
    setWfName(value);
  }

  function editWfDescription(value: string) {
    dirtyRef.current = true;
    setWfDescription(value);
  }

  function removeTask(id: string) {
    dirtyRef.current = true;
    // 从其它行的依赖里移除被删行的 id（否则会残留指向不存在行的依赖）
    setList((l) => l
      .filter((t) => t.id !== id)
      .map((t) => (t.deps.includes(id) ? { ...t, deps: t.deps.filter((d) => d !== id) } : t)));
    if (openDepsFor === id) setOpenDepsFor(null);
    setCycleHintId(null);
  }

  function addTask() {
    dirtyRef.current = true;
    const id = genId();
    const next = [...listRef.current, { id, title: '', detail: '', suggested: true, checked: true, deps: [], confirm: false, defaultValue: '' }];
    // 顺序未被动过依赖时，新行照样接进链尾（与初始化口径一致）
    setList(depsTouchedRef.current ? next : chainDeps(next));
    setFocusId(id);
  }

  /** 与相邻行交换（dir: -1 上移 / 1 下移）；依赖未被显式设置过时按新顺序重建链 */
  function moveTask(id: string, dir: -1 | 1) {
    const l = listRef.current;
    const i = l.findIndex((t) => t.id === id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= l.length) return;
    dirtyRef.current = true;
    const next = [...l];
    [next[i], next[j]] = [next[j], next[i]];
    setCycleHintId(null);
    setList(depsTouchedRef.current ? next : chainDeps(next));
  }

  /** 勾选/取消某个前置依赖；新增依赖若会造成环则拒绝并给出红字提示 */
  function toggleDep(rowId: string, depId: string) {
    const l = listRef.current;
    const row = l.find((t) => t.id === rowId);
    if (!row) return;
    dirtyRef.current = true;
    depsTouchedRef.current = true;
    if (row.deps.includes(depId)) {
      setCycleHintId(null);
      setList(l.map((t) => (t.id === rowId ? { ...t, deps: t.deps.filter((d) => d !== depId) } : t)));
      return;
    }
    // depId 已（直接或间接）依赖 rowId 时，再让 rowId 依赖 depId 就会成环
    if (dependsOn(l, depId, rowId)) {
      setCycleHintId(rowId);
      return;
    }
    setCycleHintId(null);
    setList(l.map((t) => (t.id === rowId ? { ...t, deps: [...t.deps, depId] } : t)));
  }

  /** 清空某行的依赖（= 显式无依赖，可并行） */
  function clearDeps(rowId: string) {
    dirtyRef.current = true;
    depsTouchedRef.current = true;
    setCycleHintId(null);
    setList((l) => l.map((t) => (t.id === rowId ? { ...t, deps: [] } : t)));
  }

  /**
   * 切到「来源清单」：重新调用 export（tasks: null）并据此重置本地清单
   * （对来源清单的编辑就此丢弃 —— 这是模式切换的语义）。失败则保留当前清单与模式。
   */
  async function switchToSourceList() {
    if (listMode === 'source' || switching || extracting) return;
    setSwitching(true);
    // 丢弃在途/待触发的预览刷新，避免旧清单的预览覆盖新结果
    reqSeqRef.current += 1;
    dirtyRef.current = false;
    try {
      const res = await exportWorkflow(sessionId, null, wfName.trim() || null, wfDescription);
      setDefinition(res.definition);
      setSource(res.source);
      setIgnoredCount(res.ignoredCount ?? 0);
      setConfirmedCount(res.confirmedCount ?? 0);
      setOrigins(originGroups(res.tasks));
      setList(chainDeps(toEditableTasks(res.tasks)));
      setOpenDepsFor(null);
      setCycleHintId(null);
      depsTouchedRef.current = false;
      setListMode('source');
    } catch (err) {
      showToast(errorMessage(err), 'error');
    } finally {
      setSwitching(false);
    }
  }

  /**
   * 「模型提炼」：后端发起一次专用调用（不进会话对话），整体替换本地清单为模型结果
   * 并切到 ai 模式；之后所有编辑照旧保留在本地。失败保留当前清单与模式。
   */
  async function extractWithAi() {
    if (listMode === 'ai' || extracting || switching) return;
    setExtracting(true);
    try {
      const res = await extractTasks(sessionId, 'workflow');
      reqSeqRef.current += 1;
      // 先建行拿到本地 id，再把后端返回的 1-based deps 位置映射成本地行 id
      const src = res.tasks ?? [];
      const rows: EditableTask[] = src.map((t) => ({
        id: genId(),
        title: t.title,
        detail: t.detail,
        suggested: t.suggested,
        checked: t.suggested,
        origin: 'ai' as const,
        deps: [],
        confirm: t.confirm ?? false,
        defaultValue: t.defaultValue ?? '',
      }));
      setList(rows.map((t, i) => ({
        ...t,
        deps: (src[i].deps ?? [])
          .map((p) => rows[p - 1]?.id)
          .filter((v): v is string => !!v && v !== t.id),
      })));
      setOrigins(['ai']);
      setOpenDepsFor(null);
      setCycleHintId(null);
      depsTouchedRef.current = true;
      setListMode('ai');
      // 清单被整体替换：置脏让防抖预览跟着刷新（提交口径不变，仍是勾选且有标题的行）
      dirtyRef.current = true;
    } catch (err) {
      // 失败：保留当前清单与当前模式，不清空
      showToast(errorMessage(err), 'error');
    } finally {
      setExtracting(false);
    }
  }

  if (!open) return null;

  const stages = definition?.stages ?? [];
  const nodeCount = stages.reduce((sum, s) => sum + (s.nodes?.length ?? 0), 0);
  const edgeCount = stages.reduce((sum, s) => sum + (s.edges?.length ?? 0), 0);
  const noSubmitTask = !loading && !error && submitTasks.length === 0;
  // 显示序号（T1、T2…）按当前显示顺序实时重排；依赖文案也用它
  const orderMap = new Map(list.map((t, i) => [t.id, i + 1]));

  async function handleCreate() {
    if (submitTasks.length === 0) return;
    setCreating(true);
    try {
      // 名称留空 = 用后端默认（传 null）；描述允许留空（传空串，后端原样落库）
      const def = await promoteWorkflow(sessionId, submitTasks, wfName.trim() || null, wfDescription);
      showToast('工作流已创建', 'success');
      onCreated(def.id);
    } catch (err) {
      // 失败保留弹窗，用户可调整清单后重试
      showToast(errorMessage(err), 'error');
    } finally {
      setCreating(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={() => { if (!creating) onClose(); }}
    >
      <div
        className="w-[600px] max-h-[80vh] max-w-[calc(100vw-32px)] flex flex-col rounded-xl p-4 pb-5 gap-3"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between">
          <span className="text-sm font-medium flex items-center gap-1.5" style={{ color: 'var(--text-primary)' }}>
            <Workflow size={14} style={{ color: 'var(--accent)' }} />
            转为工作流
          </span>
          <button
            className="pd-btn pd-btn-sm"
            style={{ color: 'var(--text-secondary)' }}
            onClick={onClose}
            disabled={creating}
            title="关闭"
          >
            <X size={14} />
          </button>
        </div>

        <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
          把会话「{sessionTitle || '未命名会话'}」的任务清单还原为可运行的工作流（每个任务 = 一个节点、
          任务依赖 = 节点连接；标注「需确认」的任务为人工交互节点）。
          勾选并编辑下列任务后创建，编辑器里可继续调整或直接运行。
        </span>

        {/* 加载中 */}
        {loading && (
          <div className="flex-1 flex items-center justify-center py-10">
            <div className="pilotdesk-spinner" />
            <span className="ml-2 text-xs" style={{ color: 'var(--text-secondary)' }}>正在生成预览...</span>
          </div>
        )}

        {/* 失败：展示后端文案（可读中文），并禁用创建 */}
        {!loading && error && (
          <div
            className="rounded-lg p-3 text-xs leading-relaxed"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
              color: 'var(--status-danger, #ef4444)',
            }}
          >
            {error}
          </div>
        )}

        {!loading && !error && (
          <div className="flex-1 overflow-y-auto flex flex-col gap-2 px-1 pb-1 pd-scroll-stable">
            {/* 名称 / 描述（可编辑）：首次导出后用后端默认值预填，描述同时也是展示给用户的"工作流说明" */}
            <div className="flex flex-col gap-1 shrink-0">
              <span className="text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>
                工作流名称
              </span>
              <input
                className="w-full rounded-lg px-2 py-1 text-[11px] leading-relaxed outline-none"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  border: '1px solid var(--border)',
                  color: 'var(--text-primary)',
                }}
                value={wfName}
                onChange={(e) => editWfName(e.target.value)}
                placeholder="留空则按来源会话自动命名"
                title="工作流名称（留空 = 用后端默认名称）"
              />
              <span className="text-[10px] font-medium mt-1" style={{ color: 'var(--text-secondary)' }}>
                工作流描述
              </span>
              <textarea
                className="w-full rounded-lg px-2 py-1 text-[11px] leading-relaxed outline-none resize-none"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  border: '1px solid var(--border)',
                  color: 'var(--text-primary)',
                }}
                rows={3}
                value={wfDescription}
                onChange={(e) => editWfDescription(e.target.value)}
                placeholder="工作流说明（可留空）"
                title="工作流描述：即工作流定义里的说明，创建后可在编辑器里继续调整"
              />
              <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                默认按“来源会话 + 任务清单”生成，可自行修改；它就是工作流的说明
              </span>
            </div>

            {/* 任务清单（可勾选 + 可编辑） */}
            <div className="flex flex-col gap-1 shrink-0">
              <div className="flex items-center gap-2">
                {/* 两态切换：来源清单（export 候选）/ 模型提炼（后端专用调用） */}
                <div
                  className="flex items-center gap-0.5 p-0.5 rounded-lg shrink-0"
                  style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                >
                  <button
                    className="pd-btn pd-btn-sm px-2 py-0.5 text-[10px] rounded"
                    style={listMode === 'source'
                      ? { backgroundColor: 'var(--accent-light)', color: 'var(--accent)', border: '1px solid var(--accent)' }
                      : { color: 'var(--text-tertiary)', border: '1px solid transparent' }}
                    onClick={switchToSourceList}
                    disabled={switching || extracting}
                    title="来源清单：会话计划 + 会话消息（可勾选与编辑）"
                  >
                    {switching ? '加载中…' : '来源清单'}
                  </button>
                  <button
                    className="pd-btn pd-btn-sm px-2 py-0.5 text-[10px] rounded"
                    style={listMode === 'ai'
                      ? { backgroundColor: 'var(--accent-light)', color: 'var(--accent)', border: '1px solid var(--accent)' }
                      : { color: 'var(--text-tertiary)', border: '1px solid transparent' }}
                    onClick={extractWithAi}
                    disabled={extracting || switching}
                    title="让模型读取本会话并整理任务清单（需要配置了提供商的 API 会话）"
                  >
                    {extracting ? '提炼中…' : '模型提炼'}
                  </button>
                </div>
                <span className="text-[10px] font-medium shrink-0" style={{ color: 'var(--text-secondary)' }}>
                  任务清单（{submitTasks.length}/{list.length}）
                </span>
                <button
                  className="pd-btn pd-btn-sm ml-auto shrink-0"
                  onClick={addTask}
                  style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
                >
                  <Plus size={11} /> 新增任务
                </button>
              </div>
              {source && (
                <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  {sourceNote(source, ignoredCount, origins)}
                </span>
              )}
              {confirmedCount > 0 && (
                <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  本会话已有 {confirmedCount} 条需求确认，答复已并入总目标，运行期无需再确认
                </span>
              )}
              {list.some((t) => t.checked && t.confirm) && (
                <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  标注「需确认」的任务会生成「人工交互」节点：工作流运行到该节点会挂起，你在编辑器里作答后继续；
                  填了默认值的话，无人应答（超时）时按默认值继续，自动运行不空转。答复会被下游节点引用
                </span>
              )}

              {list.length === 0 ? (
                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                  {listMode === 'ai' ? '模型没有给出任务，可点「＋ 新增任务」手动补充。' : '未识别到任务，可点「＋ 新增任务」手动补充。'}
                </span>
              ) : (
                <div className="flex flex-col gap-1">
                  {list.map((t) => {
                    const originLabel = t.origin ? ORIGIN_LABEL[t.origin] : undefined;
                    const pos = orderMap.get(t.id) ?? 0;
                    const depText = t.deps.length === 0
                      ? '无（可并行）'
                      : t.deps.map((d) => `T${orderMap.get(d) ?? '?'}`).join('、');
                    return (
                      <div
                        key={t.id}
                        className="flex flex-col rounded-lg"
                        style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                      >
                        <div className="flex items-center gap-2 px-2 py-1">
                          <span
                            className="shrink-0 text-[9px] tabular-nums"
                            style={{ color: 'var(--text-tertiary)', minWidth: 16 }}
                          >
                            T{pos}
                          </span>
                          <input
                            type="checkbox"
                            className="shrink-0 cursor-pointer"
                            checked={t.checked}
                            onChange={() => toggleTask(t.id)}
                            style={{ accentColor: 'var(--accent)' }}
                            title={t.checked ? '取消勾选（不参与转换）' : '勾选后参与转换'}
                          />
                          {originLabel && (
                            <span
                              className="shrink-0 text-[9px] leading-none px-1 py-0.5 rounded"
                              style={{
                                backgroundColor: 'var(--bg-tertiary)',
                                border: '1px solid var(--border)',
                                color: 'var(--text-tertiary)',
                              }}
                              title={`候选来源：${originLabel}`}
                            >
                              {originLabel}
                            </span>
                          )}
                          {/* 任务类型：普通 Agent 任务 / 确认类任务（人工交互节点，运行到此处停下来问用户） */}
                          <button
                            className="pd-btn shrink-0 text-[9px] leading-none px-1 py-0.5 rounded"
                            style={t.confirm
                              ? { backgroundColor: 'rgba(245,158,11,0.15)', border: '1px solid rgba(245,158,11,0.5)', color: '#F59E0B' }
                              : { backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)', color: 'var(--text-tertiary)' }}
                            onClick={() => toggleConfirm(t.id)}
                            title={t.confirm
                              ? '确认类任务：转成工作流后是「人工交互」节点（运行到此处挂起等你作答）。点一下改回 Agent 任务'
                              : '点一下标为确认类任务（需你拍板才能继续）：转成工作流后是「人工交互」节点，运行到此处会停下来等你作答'}
                          >
                            {t.confirm ? '需确认' : '任务'}
                          </button>
                          <input
                            ref={(el) => {
                              if (el) inputRefs.current.set(t.id, el);
                              else inputRefs.current.delete(t.id);
                            }}
                            className="flex-1 min-w-0 bg-transparent text-[11px] leading-relaxed outline-none"
                            style={{ color: t.checked ? 'var(--text-primary)' : 'var(--text-tertiary)' }}
                            value={t.title}
                            onChange={(e) => editTitle(t.id, e.target.value)}
                            placeholder="任务标题"
                          />
                          <button
                            className="pd-btn pd-btn-sm shrink-0 px-1"
                            onClick={() => moveTask(t.id, -1)}
                            disabled={pos <= 1}
                            style={{ color: 'var(--text-tertiary)' }}
                            title="上移（与上一行交换）"
                          >
                            <ArrowUp size={12} />
                          </button>
                          <button
                            className="pd-btn pd-btn-sm shrink-0 px-1"
                            onClick={() => moveTask(t.id, 1)}
                            disabled={pos >= list.length}
                            style={{ color: 'var(--text-tertiary)' }}
                            title="下移（与下一行交换）"
                          >
                            <ArrowDown size={12} />
                          </button>
                          <button
                            className="pd-btn pd-btn-sm shrink-0"
                            onClick={() => removeTask(t.id)}
                            style={{ color: 'var(--text-tertiary)' }}
                            title="删除该任务"
                          >
                            <X size={12} />
                          </button>
                        </div>
                        <div className="flex items-center gap-2 px-2 pb-1">
                          <span className="shrink-0" style={{ minWidth: 16 }} />
                          <button
                            className="pd-btn pd-btn-sm text-[10px] px-1.5 py-0.5 rounded"
                            style={{
                              border: '1px solid var(--border)',
                              color: t.deps.length > 0 ? 'var(--accent)' : 'var(--text-tertiary)',
                            }}
                            disabled={!t.checked}
                            onClick={() => {
                              if (!t.checked) return;
                              setCycleHintId(null);
                              setOpenDepsFor((v) => (v === t.id ? null : t.id));
                            }}
                            title={t.checked ? '设置前置依赖；不设置 = 无依赖（可与其它任务并行）' : '先勾选该任务，才能设置依赖'}
                          >
                            依赖：{depText}
                          </button>
                          {/* 确认项的默认值：无人应答（超时）时交互节点按它继续 —— 自动运行不空转的关键 */}
                          {t.confirm && (
                            <input
                              className="flex-1 min-w-0 rounded px-1.5 py-0.5 text-[10px] leading-relaxed outline-none"
                              style={{
                                backgroundColor: 'var(--bg-secondary)',
                                border: '1px solid var(--border)',
                                color: t.checked ? 'var(--text-secondary)' : 'var(--text-tertiary)',
                              }}
                              value={t.defaultValue}
                              onChange={(e) => editDefaultValue(t.id, e.target.value)}
                              disabled={!t.checked}
                              placeholder="默认值：无人应答时按它继续（可留空）"
                              title="人工确认节点的默认值（会写进节点的「默认响应内容」）：工作流在无人应答、超时后按此值继续；留空则超时取空串"
                            />
                          )}
                        </div>
                        {openDepsFor === t.id && (
                          <div
                            className="mx-2 mb-1.5 p-2 rounded-lg flex flex-col gap-1"
                            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                          >
                            <div className="flex items-center gap-2">
                              <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                                前置任务（勾选 = 必须先完成）
                              </span>
                              <button
                                className="pd-btn pd-btn-sm ml-auto shrink-0 text-[10px] px-1.5 py-0.5 rounded"
                                onClick={() => clearDeps(t.id)}
                                disabled={t.deps.length === 0}
                                style={{ border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
                              >
                                清空（可并行）
                              </button>
                            </div>
                            {list.filter((o) => o.id !== t.id && o.checked).map((o) => (
                              <label key={o.id} className="flex items-center gap-1.5 cursor-pointer">
                                <input
                                  type="checkbox"
                                  className="shrink-0 cursor-pointer"
                                  checked={t.deps.includes(o.id)}
                                  onChange={() => toggleDep(t.id, o.id)}
                                  style={{ accentColor: 'var(--accent)' }}
                                />
                                <span
                                  className="shrink-0 text-[10px] tabular-nums"
                                  style={{ color: 'var(--text-secondary)' }}
                                >
                                  T{orderMap.get(o.id)}
                                </span>
                                <span className="truncate text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                                  {o.title || '未命名任务'}
                                </span>
                              </label>
                            ))}
                            {cycleHintId === t.id && (
                              <span className="text-[10px]" style={{ color: 'var(--status-danger, #ef4444)' }}>
                                会造成循环依赖，已忽略
                              </span>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}

              {noSubmitTask && (
                <span className="text-[10px]" style={{ color: 'var(--status-danger, #ef4444)' }}>
                  至少保留一个任务
                </span>
              )}
            </div>

            {/* 预览摘要（由当前提交清单导出，随清单改动刷新） */}
            {definition && (
              <>
                <div
                  className="rounded-lg p-3 shrink-0"
                  style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                >
                  <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {stages.length} 个阶段 · {nodeCount} 个节点 · {edgeCount} 条连接
                    {refreshing && ' · 预览更新中…'}
                  </div>
                </div>

                <button
                  className="pd-btn flex items-center gap-1 px-2 py-1 rounded text-[10px] self-start shrink-0"
                  style={{ border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
                  onClick={() => setShowJson((v) => !v)}
                  title="查看转换后的工作流定义原文"
                >
                  {showJson ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                  查看完整 JSON
                </button>
                {showJson && (
                  <pre
                    className="shrink-0 rounded-lg p-3 text-[11px] leading-relaxed"
                    style={{
                      maxHeight: 260,
                      overflow: 'auto',
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-primary)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    {JSON.stringify(definition, null, 2)}
                  </pre>
                )}
              </>
            )}
          </div>
        )}

        {/* 操作区 */}
        <div className="flex justify-end gap-2 shrink-0">
          <button
            className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
            style={{
              border: '1px solid var(--border)',
              background: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
            }}
            onClick={onClose}
            disabled={creating}
          >
            取消
          </button>
          <button
            className="pd-btn pd-btn-primary px-3 py-1.5 text-xs rounded transition-colors"
            disabled={creating || loading || !!error || !definition || submitTasks.length === 0}
            onClick={handleCreate}
            title={submitTasks.length === 0 ? '至少保留一个任务' : '创建后可立即编辑或运行'}
          >
            {creating ? '创建中…' : '创建并打开编辑器'}
          </button>
        </div>
      </div>
    </div>
  );
}
