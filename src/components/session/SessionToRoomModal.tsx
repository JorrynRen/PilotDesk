// 会话 → 群聊弹窗：打开时拉取将要创建的房间（议题 + 候选任务清单）做预览，
// 清单可勾选/可编辑（本地清单是唯一事实来源），并在同一面板里配置成员与负责人后创建房间，
// 连同清单一起回传后端，交由调用方切到群聊模式并选中该房间。
// 清单有两个互斥来源（两态切换，两边都可自由编辑）：来源清单（export 候选）/ 模型提炼（后端专用调用）。
// 转换规则在后端（会话计划 + 会话消息，and 合并 → 房间任务）；前端只负责清单编辑、预览刷新、配置、报错与确认。
// 成员字段与 agentConfig 组装口径与群聊「新建房间」弹窗（GroupChatPage.CreateRoomModal）逐字一致。

import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Plus, Users, X, XCircle } from 'lucide-react';
import {
  useSessionStore,
  type PlanCandidateOrigin,
  type PlanTaskInput,
  type RoomPlan,
  type SessionPlanSource,
} from '../../stores/sessionStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { isApiSession } from '../../utils/sessionType';
import type { CreateGroupChatRoomInput, GroupChatParticipantInput } from '../../types/groupchat';

interface SessionToRoomModalProps {
  open: boolean;
  sessionId: string;
  /** 当前会话标题（默认房间标题，也写进说明文案让用户确认转换对象没弄错） */
  sessionTitle: string;
  onClose: () => void;
  /** 创建成功回调：参数为新房间 id（由调用方负责刷新列表、选中房间并切到群聊模式） */
  onCreated: (roomId: string) => void;
}

/** 参与者草稿（字段与 CreateRoomModal 一致；id 建草稿时即确定，供「负责人」下拉绑定） */
interface DraftParticipant {
  id: string;
  type: 'api' | 'cli';
  displayName: string;
  systemRole: string;
  provider: string;
  model: string;
  agentType: string;
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
}

/** 来源小标签：仅 todos/messages/ai 展示（edited 是回传口径，不展示） */
const ORIGIN_LABEL: Partial<Record<PlanCandidateOrigin, string>> = {
  todos: '计划',
  messages: '消息',
  ai: 'AI',
};

function genId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/** 候选清单（房间任务带序号）→ 本地清单：按 no 排序落成本地行（suggested 决定默认勾选，不按 origin 过滤） */
function toEditableTasks(tasks: RoomPlan['tasks'] | undefined): EditableTask[] {
  return [...(tasks ?? [])]
    .sort((a, b) => a.no - b.no)
    .map((t) => ({
      id: genId('task'),
      title: t.title,
      detail: t.detail,
      suggested: t.suggested,
      checked: t.suggested,
      origin: t.origin,
      deps: [],
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
function originGroups(tasks: Array<{ origin?: PlanCandidateOrigin }> | undefined): PlanCandidateOrigin[] {
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

export function SessionToRoomModal({ open, sessionId, sessionTitle, onClose, onCreated }: SessionToRoomModalProps) {
  const exportRoomPlan = useSessionStore((s) => s.exportRoomPlan);
  const promoteToRoom = useSessionStore((s) => s.promoteToRoom);
  const extractTasks = useSessionStore((s) => s.extractTasks);
  const { providers, fetchProviders } = useApiProviderStore();
  const { agents, getDisplayName } = useAgentRegistry();

  const [plan, setPlan] = useState<RoomPlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  /** 「来源清单」重新导出中（不阻塞编辑） */
  const [switching, setSwitching] = useState(false);
  /** 「模型提炼」请求中 */
  const [extracting, setExtracting] = useState(false);
  /** 预览失败（后端中文文案）：弹窗内红色提示且禁用创建 */
  const [previewError, setPreviewError] = useState<string | null>(null);
  /** 表单校验 / 创建失败 */
  const [error, setError] = useState<string | null>(null);

  /** 本地任务清单：唯一事实来源（由 export 候选或模型提炼结果初始化） */
  const [list, setList] = useState<EditableTask[]>([]);
  /** 来源说明（source/ignoredCount 由 export 结果给出） */
  const [source, setSource] = useState<SessionPlanSource | null>(null);
  const [ignoredCount, setIgnoredCount] = useState(0);
  /** 本会话里用户已确认过的需求条数（答复已并入总目标，运行期无需再确认） */
  const [confirmedCount, setConfirmedCount] = useState(0);
  /** 当前候选清单里实际出现的来源分组（模式初始化时确定，编辑不改变来源说明） */
  const [origins, setOrigins] = useState<PlanCandidateOrigin[]>([]);
  /** 清单来源模式：来源清单（默认）/ 模型提炼 —— 两态互斥，仅决定清单从哪来 */
  const [listMode, setListMode] = useState<ListMode>('source');
  /** 清单改动后的预览刷新中（不阻塞编辑） */
  const [refreshing, setRefreshing] = useState(false);
  /** 待聚焦的行（新增任务后聚焦到它） */
  const [focusId, setFocusId] = useState<string | null>(null);
  /** 当前展开依赖面板的行 id（行内就地展开，不用 portal） */
  const [openDepsFor, setOpenDepsFor] = useState<string | null>(null);
  /** 最近一次被拒绝的成环勾选所在行 id（行下方红字提示） */
  const [cycleHintId, setCycleHintId] = useState<string | null>(null);

  const [title, setTitle] = useState('');
  const [topic, setTopic] = useState('');
  const [directorProvider, setDirectorProvider] = useState('');
  const [directorModel, setDirectorModel] = useState('');
  const [drafts, setDrafts] = useState<DraftParticipant[]>([]);
  /** 负责人：'' = 不指定（自动分配，主持人可改派），否则为某个 api/cli 参与者的 id */
  const [assigneeId, setAssigneeId] = useState('');
  /** 用户是否改过清单：未改动前不做防抖重导出（避免来源被无谓刷成"已编辑"） */
  const dirtyRef = useRef(false);
  /**
   * 依赖是否已被用户/模型显式设置：false = 顺序调整时重建链（顺序仍紧凑）；
   * true = 保持既有依赖不变（尊重用户/模型的显式设置）
   */
  const depsTouchedRef = useRef(false);
  /** 事件处理器里读取最新清单（成环判定等需在 setList 之外同步计算） */
  const listRef = useRef<EditableTask[]>([]);
  /** 用户是否手改过议题：手改后预览刷新不再覆盖（避免吞掉用户输入） */
  const topicTouchedRef = useRef(false);
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

  /** 统一执行者下拉选项：空值 = 不指定，其余为各参与者的显示名 */
  const assigneeOptions = useMemo(
    () => [
      { value: '', label: '不指定（自动分配，主持人可改派）' },
      ...drafts.map((d) => ({
        value: d.id,
        label: d.displayName || (d.type === 'api' ? 'API 参与者' : 'CLI 参与者'),
      })),
    ],
    [drafts],
  );

  useEffect(() => {
    if (!open) return;
    fetchProviders();
  }, [open, fetchProviders]);

  // 每次打开都重新导出预览（会话内容可能已变化），同时按当前会话重置标题/议题与默认成员。
  // 状态重置用「渲染期修正」（React adjust-during-render）而不是 effect 内同步 setState —— 后者会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），且会让紧随其后的"防抖重导出"effect 读到上一会话的清单。
  // 初始用 undefined 哨兵，保证"挂载即打开"时也重置一次（与原 effect 首跑对齐）。
  // ref 只能在 effect 里重置（渲染期写 ref 会被 `react-hooks/refs` 判违规）。
  const openKey = open ? `${sessionId}|${sessionTitle}` : null;
  const [openKeyPrev, setOpenKeyPrev] = useState<string | null | undefined>(undefined);
  if (openKeyPrev !== openKey) {
    setOpenKeyPrev(openKey);
    if (openKey !== null) {
      // 默认成员：director/user 固定 id；再按会话类型给一个可直接用的 Agent 参与者
      // （API 会话 → api 参与者，预填会话的 provider/model；CLI 会话 → cli 参与者，预填会话的 agentType）。
      const session = useSessionStore.getState().sessions.find((s) => s.id === sessionId);
      const agentType = session?.agentType ?? '';
      if (isApiSession(agentType)) {
        setDirectorProvider(session?.apiProvider ?? '');
        setDirectorModel(session?.apiModel ?? '');
        setDrafts([{
          id: genId('api'),
          type: 'api',
          displayName: getDisplayName('api'),
          systemRole: '',
          provider: session?.apiProvider ?? '',
          model: session?.apiModel ?? '',
          agentType: '',
        }]);
      } else {
        setDirectorProvider('');
        setDirectorModel('');
        setDrafts([{
          id: genId('cli'),
          type: 'cli',
          displayName: getDisplayName(agentType),
          systemRole: '',
          provider: '',
          model: '',
          agentType,
        }]);
      }

      setPlan(null);
      setPreviewError(null);
      setError(null);
      setAssigneeId('');
      setTitle(sessionTitle || '');
      setTopic('');
      setList([]);
      setSource(null);
      setIgnoredCount(0);
      setConfirmedCount(0);
      setOrigins([]);
      setOpenDepsFor(null);
      setCycleHintId(null);
      setListMode('source');
      setSwitching(false);
      setExtracting(false);
      setRefreshing(false);
      setLoading(true);
    }
  }

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    depsTouchedRef.current = false;
    dirtyRef.current = false;
    topicTouchedRef.current = false;
    reqSeqRef.current += 1;
    (async () => {
      try {
        const p = await exportRoomPlan(sessionId, null);
        if (cancelled) return;
        setPlan(p);
        setTitle(p.title || sessionTitle || '');
        setTopic(p.topic || '');
        setSource(p.source);
        setIgnoredCount(p.ignoredCount ?? 0);
        setConfirmedCount(p.confirmedCount ?? 0);
        setOrigins(originGroups(p.tasks));
        // 首次打开：用后端候选清单初始化本地清单（suggested 决定默认勾选），依赖默认串成链
        setList(chainDeps(toEditableTasks(p.tasks)));
      } catch (err) {
        if (cancelled) return;
        const message = errorMessage(err);
        setPreviewError(message);
        showToast(message, 'error');
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [open, sessionId, sessionTitle, exportRoomPlan, getDisplayName]);

  // 勾选/编辑/增删后（防抖 400ms）用当前提交清单再导出一次，只刷新预览（议题），不触碰本地清单
  // （来源说明不随之变化：它描述的是候选来源分组，不因用户编辑而变）
  useEffect(() => {
    if (!open || !dirtyRef.current) return;
    const tasks = submitRef.current;
    // 空清单后端会直接拒绝（弹窗内已有提示且禁用创建），这里静默跳过
    if (tasks.length === 0) return;
    const timer = setTimeout(async () => {
      const seq = ++reqSeqRef.current;
      setRefreshing(true);
      try {
        const p = await exportRoomPlan(sessionId, tasks);
        if (seq !== reqSeqRef.current) return;
        // 议题已被用户手改时不覆盖
        if (!topicTouchedRef.current) setTopic(p.topic || '');
      } catch (err) {
        // 预览刷新失败不打断编辑：保留上一份预览，提交时后端错误仍会展示
        console.warn('Failed to refresh room plan preview:', err);
      } finally {
        if (seq === reqSeqRef.current) setRefreshing(false);
      }
    }, 400);
    return () => clearTimeout(timer);
  }, [open, sessionId, exportRoomPlan, submitKey]);

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

  // Esc 关闭（创建过程中不响应，避免半途打断）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !creating) onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, creating, onClose]);

  const cliAgents = useMemo(
    () => agents.filter((a) => a.agentType !== 'api' && a.isEnabled),
    [agents],
  );

  const modelsOf = (providerId: string) =>
    providers.find((p) => p.id === providerId)?.models ?? [];

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

  function editTaskTitle(id: string, taskTitle: string) {
    dirtyRef.current = true;
    setList((l) => l.map((t) => (t.id === id ? { ...t, title: taskTitle } : t)));
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
    const id = genId('task');
    const next = [...listRef.current, { id, title: '', detail: '', suggested: true, checked: true, deps: [] }];
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
      const p = await exportRoomPlan(sessionId, null);
      setPlan(p);
      setSource(p.source);
      setIgnoredCount(p.ignoredCount ?? 0);
      setConfirmedCount(p.confirmedCount ?? 0);
      setOrigins(originGroups(p.tasks));
      setList(chainDeps(toEditableTasks(p.tasks)));
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
      // target='room'：群聊主持人可以向用户确认，后端提示词因此不同（不产出确认类任务）
      const res = await extractTasks(sessionId, 'room');
      reqSeqRef.current += 1;
      // 先建行拿到本地 id，再把后端返回的 1-based deps 位置映射成本地行 id
      const src = res.tasks ?? [];
      const rows: EditableTask[] = src.map((t) => ({
        id: genId('task'),
        title: t.title,
        detail: t.detail,
        suggested: t.suggested,
        checked: t.suggested,
        origin: 'ai' as const,
        deps: [],
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

  function addDraft(type: 'api' | 'cli') {
    // 新增参与者插入列表开头，避免用户滚动到底部去填写。
    setDrafts((d) => [
      { id: genId(type), type, displayName: '', systemRole: '', provider: '', model: '', agentType: '' },
      ...d,
    ]);
  }

  function updateDraft(idx: number, patch: Partial<DraftParticipant>) {
    setDrafts((d) => d.map((x, i) => (i === idx ? { ...x, ...patch } : x)));
  }

  function removeDraft(idx: number) {
    // 被删的参与者正好是负责人时清空选择，避免提交已不存在的 id（后端会直接拒绝）
    if (drafts[idx]?.id === assigneeId) setAssigneeId('');
    setDrafts((d) => d.filter((_, i) => i !== idx));
  }

  async function handleCreate() {
    setError(null);
    if (!title.trim()) {
      setError('请填写房间标题');
      return;
    }
    if (submitTasks.length === 0) {
      setError('至少保留一个任务');
      return;
    }
    if (!directorProvider || !directorModel) {
      setError('请选择 Director 的模型（提供商 + 模型）');
      return;
    }

    const participants: GroupChatParticipantInput[] = [];
    participants.push({
      id: 'director',
      participantType: 'director',
      agentConfig: JSON.stringify({ provider: directorProvider, model: directorModel }),
      displayName: '主持人',
      systemRole: '协调/裁决',
    });

    for (const d of drafts) {
      if (!d.displayName.trim()) {
        setError('参与者显示名不能为空');
        return;
      }
      if (d.type === 'api') {
        if (!d.provider || !d.model) {
          setError(`参与者「${d.displayName}」需选择提供商与模型`);
          return;
        }
        participants.push({
          id: d.id,
          participantType: 'api',
          agentConfig: JSON.stringify({ provider: d.provider, model: d.model }),
          displayName: d.displayName.trim(),
          systemRole: d.systemRole.trim() || '参与者',
        });
      } else {
        if (!d.agentType) {
          setError(`参与者「${d.displayName}」需选择 CLI Agent 类型`);
          return;
        }
        participants.push({
          id: d.id,
          participantType: 'cli',
          agentConfig: JSON.stringify({ agent_type: d.agentType }),
          displayName: d.displayName.trim(),
          systemRole: d.systemRole.trim() || '参与者',
        });
      }
    }

    participants.push({
      id: 'user',
      participantType: 'user',
      agentConfig: '{}',
      displayName: '我',
      systemRole: '用户',
    });

    const input: CreateGroupChatRoomInput = {
      title: title.trim(),
      topic: topic.trim(),
      participants,
      directorId: 'director',
      // 与新建房间弹窗的默认一致：允许主持人按需自动补充 CLI 参与者
      allowAutoCli: 1,
    };

    try {
      setCreating(true);
      const room = await promoteToRoom(sessionId, input, assigneeId || null, submitTasks);
      showToast('群聊已创建', 'success');
      onCreated(room.roomId);
    } catch (err) {
      // 失败保留弹窗，用户可调整成员/负责人后重试
      showToast(errorMessage(err), 'error');
    } finally {
      setCreating(false);
    }
  }

  if (!open) return null;

  // 显示序号（T1、T2…）按当前显示顺序实时重排；依赖文案也用它
  const orderMap = new Map(list.map((t, i) => [t.id, i + 1]));

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={() => { if (!creating) onClose(); }}
    >
      <div
        className="w-[620px] max-h-[80vh] max-w-[calc(100vw-32px)] flex flex-col rounded-xl p-4 pb-5 gap-3"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between shrink-0">
          <span className="text-sm font-medium flex items-center gap-1.5" style={{ color: 'var(--text-primary)' }}>
            <Users size={14} style={{ color: 'var(--accent)' }} />
            转为群聊
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

        <span className="text-[10px] leading-relaxed shrink-0" style={{ color: 'var(--text-tertiary)' }}>
          把会话「{sessionTitle || '未命名会话'}」的任务清单转为群聊房间，由多 Agent 协作执行。
          勾选并编辑下列任务后创建，创建后自动进入该房间，可继续调整成员或直接开始讨论。
        </span>

        {/* 加载中 */}
        {loading && (
          <div className="flex-1 flex items-center justify-center py-10">
            <div className="pilotdesk-spinner" />
            <span className="ml-2 text-xs" style={{ color: 'var(--text-secondary)' }}>正在生成预览...</span>
          </div>
        )}

        {!loading && (
          <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-3 px-1 pb-1 pd-scroll-stable">
            {/* 预览失败：展示后端文案（可读中文） */}
            {previewError && (
              <div
                className="rounded-lg p-3 text-xs leading-relaxed shrink-0"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  border: '1px solid var(--border)',
                  color: 'var(--status-danger, #ef4444)',
                }}
              >
                {previewError}
              </div>
            )}

            {!previewError && plan && (
              <>

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
                              <input
                                ref={(el) => {
                                  if (el) inputRefs.current.set(t.id, el);
                                  else inputRefs.current.delete(t.id);
                                }}
                                className="flex-1 min-w-0 bg-transparent text-[11px] leading-relaxed outline-none"
                                style={{ color: t.checked ? 'var(--text-primary)' : 'var(--text-tertiary)' }}
                                value={t.title}
                                onChange={(e) => editTaskTitle(t.id, e.target.value)}
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
                            </div>
                            {openDepsFor === t.id && (
                              <div
                                className="mx-2 mb-1.5 p-2 rounded-lg flex flex-col gap-1"
                                style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
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
                  {submitTasks.length === 0 && (
                    <span className="text-[10px]" style={{ color: 'var(--status-danger, #ef4444)' }}>
                      至少保留一个任务
                    </span>
                  )}
                </div>

                {/* 标题 / 议题（默认取预览值，可编辑） */}
                <label className="flex flex-col gap-1 text-xs shrink-0" style={{ color: 'var(--text-secondary)' }}>
                  房间标题
                  <input
                    className="px-3 py-2 rounded-lg text-xs outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    value={title}
                    onChange={(e) => setTitle(e.target.value)}
                    placeholder="例如：PilotDesk 架构评审"
                  />
                </label>

                <label className="flex flex-col gap-1 text-xs shrink-0" style={{ color: 'var(--text-secondary)' }}>
                  讨论议题
                  <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                    默认取会话标题与摘要，可自行修改；它会作为房间的目标锚点，也是本次创建任务的共同目标。
                    {refreshing && '（预览更新中…）'}
                  </span>
                  <textarea
                    className="px-3 py-2 rounded-lg text-xs outline-none resize-none"
                    rows={3}
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    value={topic}
                    onChange={(e) => { topicTouchedRef.current = true; setTopic(e.target.value); }}
                    placeholder="留空时以进入房间后的第一条指令作为目标"
                  />
                </label>

                {/* 主持人模型（规划与裁决）—— 与新建房间弹窗同口径 */}
                <div className="flex flex-col gap-1 text-xs shrink-0" style={{ color: 'var(--text-secondary)' }}>
                  主持人模型（规划与裁决）
                  <div className="flex gap-2">
                    <Select
                      className="flex-1"
                      value={directorProvider}
                      onChange={(v) => { setDirectorProvider(v); setDirectorModel(''); }}
                      placeholder="选择提供商"
                      options={[
                        { value: '', label: '选择提供商' },
                        ...providers.map((p) => ({ value: p.id, label: p.name })),
                      ]}
                    />
                    <Select
                      className="flex-1"
                      value={directorModel}
                      onChange={setDirectorModel}
                      placeholder="选择模型"
                      disabled={!directorProvider}
                      options={[
                        { value: '', label: '选择模型' },
                        ...modelsOf(directorProvider).map((m) => ({ value: m, label: m })),
                      ]}
                    />
                  </div>
                </div>

                {/* 成员配置 */}
                <div className="flex flex-col gap-2 shrink-0">
                  <div className="flex items-center justify-between">
                    <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>参与者（可添加 API / CLI Agent）</span>
                    <div className="flex gap-1">
                      <button
                        className="pd-btn pd-btn-sm"
                        onClick={() => addDraft('api')}
                        style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
                      >
                        <Plus size={11} /> API
                      </button>
                      <button
                        className="pd-btn pd-btn-sm"
                        onClick={() => addDraft('cli')}
                        style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
                      >
                        <Plus size={11} /> CLI
                      </button>
                    </div>
                  </div>

                  {drafts.length === 0 && (
                    <div className="px-3 py-2 rounded-lg text-[11px]" style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-tertiary)' }}>
                      尚未添加参与者。至少可保留 Director + 用户，也可添加多个 API/CLI 参与者。
                    </div>
                  )}

                  {drafts.map((d, i) => (
                    <div
                      key={d.id}
                      className="rounded-lg p-3 flex flex-col gap-2"
                      style={{ border: '1px solid var(--border)', backgroundColor: 'var(--bg-primary)' }}
                    >
                      <div className="flex gap-2 items-center">
                        <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
                          {d.type === 'api' ? 'API' : 'CLI'}
                        </span>
                        <input
                          className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
                          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                          placeholder="显示名（如 Hermes Agent）"
                          value={d.displayName}
                          onChange={(e) => updateDraft(i, { displayName: e.target.value })}
                        />
                        <button className="pd-btn pd-btn-sm" onClick={() => removeDraft(i)} style={{ color: 'var(--text-tertiary)' }}>
                          <XCircle size={13} />
                        </button>
                      </div>
                      <input
                        className="px-3 py-2 rounded-lg text-xs outline-none"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                        placeholder="角色（如 架构师 / 质疑者 / 执行者）"
                        value={d.systemRole}
                        onChange={(e) => updateDraft(i, { systemRole: e.target.value })}
                      />
                      {d.type === 'api' ? (
                        <div className="flex gap-2">
                          <Select
                            className="flex-1"
                            value={d.provider}
                            onChange={(v) => updateDraft(i, { provider: v, model: '' })}
                            placeholder="选择提供商"
                            options={[
                              { value: '', label: '选择提供商' },
                              ...providers.map((p) => ({ value: p.id, label: p.name })),
                            ]}
                          />
                          <Select
                            className="flex-1"
                            value={d.model}
                            onChange={(v) => updateDraft(i, { model: v })}
                            placeholder="选择模型"
                            disabled={!d.provider}
                            options={[
                              { value: '', label: '选择模型' },
                              ...modelsOf(d.provider).map((m) => ({ value: m, label: m })),
                            ]}
                          />
                        </div>
                      ) : (
                        <Select
                          value={d.agentType}
                          onChange={(v) => updateDraft(i, { agentType: v })}
                          placeholder="选择 CLI Agent 类型"
                          options={[
                            { value: '', label: '选择 CLI Agent 类型' },
                            ...cliAgents.map((a) => ({ value: a.agentType, label: a.displayName || a.agentType })),
                          ]}
                        />
                      )}
                    </div>
                  ))}
                </div>

                {/* 统一执行者：本弹窗最底部的字段，下拉固定向上展开 —— 向下展开时选项会被弹窗/视口裁掉 */}
                <div className="flex flex-col gap-1 text-xs shrink-0" style={{ color: 'var(--text-secondary)' }}>
                  统一执行者（可选）
                  <span className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                    角色分工：主持人负责规划与裁决、并在运行期分派任务；这里可选一个统一执行者 —— 留空则按成员自动分配（每个任务轮转一个 Agent 执行者，主持人仍可在讨论中改派）；指定后本次创建的所有任务都交给该参与者执行。
                  </span>
                  <Select
                    value={assigneeId}
                    onChange={setAssigneeId}
                    placeholder="不指定（自动分配，主持人可改派）"
                    disabled={creating}
                    prefer="up"
                    options={assigneeOptions}
                  />
                </div>

                {error && (
                  <div
                    className="px-3 py-2 rounded-lg text-[11px] shrink-0"
                    style={{ color: 'var(--status-danger, #ef4444)', backgroundColor: 'var(--status-danger-bg, rgba(239,68,68,0.1))' }}
                  >
                    {error}
                  </div>
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
            disabled={creating || loading || !!previewError || !plan || submitTasks.length === 0}
            onClick={handleCreate}
            title={submitTasks.length === 0 ? '至少保留一个任务' : '创建后自动进入该群聊房间'}
          >
            {creating ? '创建中…' : '创建并进入群聊'}
          </button>
        </div>
      </div>
    </div>
  );
}
