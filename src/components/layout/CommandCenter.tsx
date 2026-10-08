/**
 * CommandCenter — 全局指挥中心
 *
 * 把「什么在跑、什么在等我、这段时间花了多少」聚到一处，并给出最近会话。
 * 两种形态（见下方 variant）：会话默认页的无会话态内嵌常驻内容，以及工作流页「待处理」
 * 按钮打开的居中模态；首次使用额外显示一次性使用指引（启动台职责，不再单独做第二个入口）。
 *
 * 数据全部来自现有 store/命令（不新增后端接口）：待处理取工作流的审批/待输入实时登记表，
 * 进行中取工作流实例与群聊房间，成本速览取近 7 天三维归因。
 * 刷新统一由 `commandCenterStore.refreshCenter()` 触发（模态态另由 `openCenter()` 打开时一并调用），
 * 本组件只读状态（不在 effect 里同步 setState）。
 */
import { useNavigate } from 'react-router-dom';
import { useEffect, useState, type ReactNode } from 'react';
import {
  LayoutDashboard,
  X,
  RefreshCw,
  ShieldAlert,
  AlertTriangle,
  MessageSquare,
  Users,
  Workflow as WorkflowIcon,
  Loader2,
  ArrowRight,
  Activity,
  BarChart3,
  History,
  HelpCircle,
} from 'lucide-react';
import { useTerminal } from '../../TerminalManager';
import { useSessionStore } from '../../stores/sessionStore';
import { useWorkflowStore } from '../../stores/workflowStore';
import type { PendingHumanInput } from '../../types/workflow';
import { useGroupChatStore } from '../../stores/groupChatStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { useCommandCenterStore, COMMAND_CENTER_USAGE_DAYS } from '../../stores/commandCenterStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { isWorkflowSession } from '../../utils/sessionType';

/** 进行中状态（工作流实例 / 群聊房间）：终态不进「进行中」。 */
const ACTIVE_INSTANCE_STATUS = ['pending', 'running', 'paused'];
const ACTIVE_ROOM_STATUS = ['running', 'paused'];

const INSTANCE_STATUS_LABEL: Record<string, string> = {
  pending: '待启动',
  running: '运行中',
  paused: '已暂停',
};

const ROOM_STATUS_LABEL: Record<string, string> = {
  running: '运行中',
  paused: '已暂停',
};

/** 归因维度展示名（key 与后端 UsageDimension.key 对应，顺序固定：会话 → 群聊 → 工作流 → 知识库）。 */
const DIMENSION_LABEL: Record<string, string> = {
  session: '会话',
  groupchat: '群聊',
  workflow: '工作流',
  knowledge: '知识库',
};

const MAX_ACTIVE_ROWS = 5;
const MAX_RECENT_SESSIONS = 5;

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}m`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

/** 相对时间（入参为秒级时间戳，与后端 `utils::now()` 一致）。 */
function fmtAgo(sec?: number): string {
  if (!sec) return '';
  const diff = Date.now() / 1000 - sec;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/**
 * 区块：**自包含卡片**（描边 + 圆角 + 标题带），在内容区的两列网格里各占一格。
 *
 * 与旧版（`borderTop` 通栏堆叠）的区别：改成卡片后各区块可并排 —— 内容少的区块（最近会话、
 * 成本速览）一行放两个，整页更矮，会话默认页内嵌时不必滚动。宽内容区块（如「待处理」「进行中」
 * 里带按钮的待办卡）用 `span` 占满整行。
 */
function Section({
  title,
  icon,
  count,
  span,
  children,
}: {
  title: string;
  icon: ReactNode;
  count?: number;
  /** 占满整行（两列时） */
  span?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className="min-w-0 overflow-hidden rounded-lg flex flex-col"
      style={{
        backgroundColor: 'var(--bg-primary)',
        border: '1px solid var(--border)',
        gridColumn: span ? '1 / -1' : undefined,
      }}
    >
      <div
        className="flex items-center gap-1.5 px-3 py-[6px]"
        style={{ backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
      >
        <span className="shrink-0 flex items-center" style={{ color: 'var(--accent)' }}>{icon}</span>
        <span className="text-[11px] font-semibold" style={{ color: 'var(--text-primary)' }}>{title}</span>
        {count !== undefined && count > 0 && (
          <span className="text-[10px] px-1.5 rounded-full" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
            {count}
          </span>
        )}
      </div>
      <div className="px-3 py-2.5">{children}</div>
    </div>
  );
}

function EmptyHint({ text }: { text: string }) {
  return (
    <div className="text-[11px] py-1" style={{ color: 'var(--text-tertiary)' }}>{text}</div>
  );
}

/** 路径/文件名等需要等宽展示的片段。 */
function PathText({ children }: { children: ReactNode }) {
  return (
    <span className="font-mono text-[10px]" style={{ color: 'var(--text-primary)' }}>{children}</span>
  );
}

/** 向导里的「配置项」行：状态徽标 + 名称 + 说明 + 一个或多个跳转入口。 */
function WizardRow({
  label,
  ready,
  hint,
  actions,
}: {
  label: string;
  ready: boolean;
  hint: string;
  actions: { label: string; onClick: () => void }[];
}) {
  return (
    <div className="flex items-center gap-2">
      <span
        className="shrink-0 rounded text-[10px]"
        style={{
          padding: '1px 5px',
          backgroundColor: ready ? 'rgba(16,185,129,0.12)' : 'var(--bg-tertiary)',
          color: ready ? '#10B981' : 'var(--text-tertiary)',
        }}
      >
        {ready ? '已就绪' : '未配置'}
      </span>
      <span className="shrink-0 text-[11px]" style={{ color: 'var(--text-primary)' }}>{label}</span>
      <span className="flex-1 min-w-0 truncate text-[10px]" style={{ color: 'var(--text-tertiary)' }} title={hint}>
        {hint}
      </span>
      <span className="shrink-0 flex items-center gap-1.5">
        {actions.map((a, i) => (
          <span key={a.label} className="flex items-center gap-1.5">
            {i > 0 && <span style={{ color: 'var(--border)' }}>|</span>}
            <button onClick={a.onClick} className="text-[10px]" style={{ color: 'var(--accent)' }}>
              {a.label}
            </button>
          </span>
        ))}
      </span>
    </div>
  );
}

/** 向导步骤序号。 */
function StepIndex({ n }: { n: number }) {
  return (
    <span
      className="shrink-0 flex items-center justify-center rounded-full text-[9px]"
      style={{ width: 14, height: 14, backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
    >
      {n}
    </span>
  );
}

/**
 * 首次使用指引的内容（数据驱动，便于统一排版层级）：
 * 每组渲染成一张卡片，组内每条 = 术语胶囊 + 说明，避免"标题与条目同级"的一长串。
 */
const GUIDE_GROUPS: { title: string; items: { term: string; desc: ReactNode }[] }[] = [
  {
    title: '五种工作模式',
    items: [
      { term: '会话', desc: '单 Agent 干活（问答、写代码、改文件），带工具、审批与文件改动历史' },
      { term: '群聊', desc: '多 Agent 动态任务编排：主持人拆任务、按依赖并行推进，执行中可干预' },
      { term: '工作流', desc: '多 Agent 静态任务编排 + 能力拓展：可视化拖拽 + 可定时触发 + 可复用（群聊结论亦可导出成工作流供复用）' },
      { term: '终端', desc: '本地 Shell 直通，可手动跑 claude / codex 等 CLI Agent 和其他命令；切换模式不丢会话' },
      { term: '门户标签页', desc: '把常用网页或本地文件、目录挂成标签实现快捷直达（设置 → 门户标签页），常驻不重载' },
    ],
  },
  {
    title: '常用功能',
    items: [
      {
        term: '灵感库',
        desc: '收藏灵感 / 提示词（可打标签）并一键发到会话或终端；入口在右栏「灵感」页签',
      },
      {
        term: '插件',
        desc: (
          <>
            扩展右栏面板、命令、工作流节点与 Agent 会话 API（需在 manifest 声明权限）；安装在{' '}
            <PathText>{'<配置目录>\\plugins\\<插件id>\\'}</PathText>
            （Windows：<PathText>{'%APPDATA%\\PilotDesk\\plugins\\<插件id>\\'}</PathText>），在右栏「插件」页签安装与管理
          </>
        ),
      },
      {
        term: '记忆 · 项目',
        desc: (
          <>
            <PathText>MEMORY.md</PathText>，位于<b>当前工作目录</b>下（按项目一份），存该项目的约定与经验；
            在「设置 → 记忆管理 → 项目记忆」编辑，或直接改该文件
          </>
        ),
      },
      {
        term: '记忆 · 用户',
        desc: (
          <>
            <PathText>USER.md</PathText>，位于 PilotDesk 配置目录（Windows：
            <PathText>{'%APPDATA%\\PilotDesk\\USER.md'}</PathText>），跨项目生效；在「设置 → 记忆管理 → 用户偏好」编辑，或直接改该文件
          </>
        ),
      },
      {
        term: '记忆 · KV',
        desc: '全局 KV 记忆（存本地数据库，无独立文件）：按 fact / preference / skill / event 分类，可标重要、可搜索；在「设置 → 记忆管理 → 全局 KV 记忆」维护，或由 Agent 用记忆工具自动沉淀',
      },
      {
        term: '知识库',
        desc: (
          <>
            结构化知识沉淀：一个库 = 名称 / 描述 / 自定义专属字段，同一条目可属于多个库；内容来自投喂文件（含云文档）、
            AI 生成、工作流产出「存为知识」与对话沉淀，先进「待确认」核对后才入库；在顶栏「知识库」模式管理与检索，
            命中条目按需注入上下文，原文默认放在 <PathText>{'<全局工作区>\\Knowledge\\files\\'}</PathText>
            （根目录可在「设置 → 知识库」更换）
          </>
        ),
      },
    ],
  },
];

/**
 * 指挥中心的两种形态：
 * - `modal`（默认）：App 级挂载的居中弹层（工作流页「待处理」按钮触发）；
 * - `inline`：嵌在会话默认页（无选中会话时）的常驻内容，去掉遮罩/居中/关闭键，滚动交给外层布局。
 * 两态共用同一份数据读取与区块渲染，只有外壳与开合门槛不同。
 */
export function CommandCenter({ variant = 'modal' }: { variant?: 'modal' | 'inline' }) {
  const inline = variant === 'inline';
  const open = useCommandCenterStore((s) => s.open);
  const setOpen = useCommandCenterStore((s) => s.setOpen);
  const closeCenter = useCommandCenterStore((s) => s.closeCenter);
  const refreshCenter = useCommandCenterStore((s) => s.refreshCenter);
  const guideSeen = useCommandCenterStore((s) => s.guideSeen);
  const markGuideSeen = useCommandCenterStore((s) => s.markGuideSeen);
  const usage = useCommandCenterStore((s) => s.usage);
  const loadingUsage = useCommandCenterStore((s) => s.loadingUsage);

  const { setMode } = useTerminal();
  const navigate = useNavigate();

  const sessions = useSessionStore((s) => s.sessions);
  const instances = useWorkflowStore((s) => s.instances);
  const pendingInputs = useWorkflowStore((s) => s.pendingInputs);
  const pendingApprovals = useWorkflowStore((s) => s.pendingApprovals);
  const respondToolApproval = useWorkflowStore((s) => s.respondToolApproval);
  const respondHumanInput = useWorkflowStore((s) => s.respondHumanInput);
  const cancelWorkflow = useWorkflowStore((s) => s.cancelWorkflow);
  const loadPendingApprovals = useWorkflowStore((s) => s.loadPendingApprovals);

  /**
   * 待处理区的就地操作状态。
   *
   * 为什么把裁决放在这里而不是工作流页：同一批待办此前在工作流页（页面级堆叠，位置还随 tab
   * 漂移）、通知中心、指挥中心各显示一份——现在收敛到指挥中心一处处理，其余位置只放进出口。
   */
  const [pendingActionKey, setPendingActionKey] = useState<string | null>(null);
  /** 待人工输入的草稿（key = `${executionId}:${nodeId}`） */
  const [inputDrafts, setInputDrafts] = useState<Record<string, string>>({});
  const rooms = useGroupChatStore((s) => s.rooms);
  const roomsActorAlive = useGroupChatStore((s) => s.roomsActorAlive);

  // ── 使用向导：Agent 集成是否已就绪（决定能否开始首次会话）──
  const providers = useApiProviderStore((s) => s.providers);
  const { agents: registryAgents, getEnabledAgentTypes } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const { createAgentSession } = useAgentEvent();
  const [starting, setStarting] = useState(false);
  // API 就绪 = 有提供商已配 Key 且至少一个模型；CLI 就绪 = 有已启用且本机已安装的 CLI Agent
  const apiProvider = providers.find((p) => p.apiKeySet && p.models.length > 0);
  const cliAgentType = getEnabledAgentTypes().find(
    (t) => (envInfo?.agentVersions?.[t] ?? null) !== null,
  );
  const cliAgentName = registryAgents.find((a) => a.agentType === cliAgentType)?.displayName;
  const agentReady = Boolean(apiProvider) || Boolean(cliAgentType);
  /**
   * 主动重新打开使用引导（不落盘，仅本次面板存活期间有效）。
   *
   * 背景：引导"看过一次就不再出现"是刻意的（新用户不被反复打扰），但**没有回来的路**——
   * 用户之后想再对照一遍步骤，只能靠清 localStorage。现在引导收起后，
   * 会在它原本的位置（待处理之前）留一行常驻入口，点开即还原完整内容。
   * 让"已看过"变成"默认收起、随时可展开"，而不是"永久消失"。
   */
  const [guideForcedOpen, setGuideForcedOpen] = useState(false);
  // 未配置时向导常驻（即使用户已点过「知道了」——配置是使用前提，不能因为关掉指引就找不到了）
  const showGuide = !guideSeen || !agentReady || guideForcedOpen;
  /**
   * 是否展示「完整引导」。
   *
   * 与 showGuide 的区别：未配置 Agent 时引导块常驻，但那时只留「使用向导」这一块；
   * 首次（含刚打开时的自动展示）与用户主动展开时，才连同功能说明一起给全。
   * 之前只有「首次」一条路径，所以主动展开时只能看到半截内容。
   */
  const guideShowFull = !guideSeen || guideForcedOpen;

  /** 收起引导：首次点「知道了」要落盘（下次不再自动弹），任何时候都要清掉主动展开的标记 */
  const dismissGuide = () => {
    if (!guideSeen) markGuideSeen();
    setGuideForcedOpen(false);
  };

  // Esc 关闭（与确认弹窗一致）。监听器只在打开期间挂载，回调里才 setState。
  // 走 closeCenter：首次关闭时给出「入口在这里」指引，避免新用户关掉后找不到入口。
  // 内嵌态不参与 Esc：它不是弹层，按 Esc 没有"关掉"的语义。
  useEffect(() => {
    if (!open || inline) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closeCenter();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, inline, closeCenter]);

  // 内嵌态挂载即拉一次数据（模态态由 openCenter 负责）。
  // 刷新落到 store，不碰组件 state，故不触发「effect 内 setState」的级联渲染问题。
  useEffect(() => {
    if (inline) void refreshCenter();
  }, [inline, refreshCenter]);

  if (!open && !inline) return null;

  // ── 跳转：先切模式/页面，再关面板 ──
  const goSession = (sessionId?: string) => {
    if (sessionId) void useSessionStore.getState().selectSession(sessionId);
    setMode('session');
    setOpen(false);
  };
  const goRoom = (roomId: string) => {
    void useGroupChatStore.getState().selectRoom(roomId);
    setMode('groupchat');
    setOpen(false);
  };
  const goWorkflow = () => { setMode('workflow'); setOpen(false); };
  // 向导的「去配置」：API 提供商列表 / CLI Agent 集成配置 / 环境检测与安装
  const goApiConfig = () => { navigate('/settings?tab=api'); setOpen(false); };
  const goAgentConfig = () => { navigate('/settings?tab=agents'); setOpen(false); };
  const goEnvConfig = () => { navigate('/settings?tab=environment'); setOpen(false); };
  // 精准跳转：设置页 → API 集成配置 → 用量统计（apiTab 由 URL 指定）
  const goUsageStats = () => { navigate('/settings?tab=api&apiTab=usage'); setOpen(false); };
  const goMemory = () => { navigate('/settings?tab=memory'); setOpen(false); };

  /** 批准 / 拒绝一次工具审批（就地裁决；失败提示与列表刷新由 store 负责） */
  const decideApproval = async (callId: string, approved: boolean) => {
    setPendingActionKey(callId);
    try {
      await respondToolApproval(callId, approved);
    } finally {
      setPendingActionKey(null);
    }
  };

  /** 提交一次人工输入（就地） */
  const submitPendingInput = async (item: PendingHumanInput) => {
    const key = `${item.executionId}:${item.nodeId}`;
    const value = (inputDrafts[key] ?? '').trim();
    if (!value) return;
    setPendingActionKey(key);
    try {
      await respondHumanInput(item.executionId, item.nodeId, value);
      setInputDrafts((prev) => { const next = { ...prev }; delete next[key]; return next; });
    } finally {
      setPendingActionKey(null);
    }
  };

  /** 停掉一个卡住的执行（失效审批的唯一出口：等待者已随进程消失，只能停） */
  const stopExecution = async (executionId: string) => {
    setPendingActionKey(executionId);
    try {
      await cancelWorkflow(executionId);
      await loadPendingApprovals();
    } finally {
      setPendingActionKey(null);
    }
  };

  /** 审批参数预览：单行截断（完整内容在 title 里） */
  const previewArgs = (raw: string) => (raw.length > 56 ? `${raw.slice(0, 56)}…` : raw);

  /** 向导第 ② 步：按已就绪的集成方式直接建一个会话并进入（API 优先，其次 CLI）。 */
  const startFirstSession = async () => {
    if (starting || !agentReady) return;
    setStarting(true);
    const store = useSessionStore.getState();
    try {
      if (apiProvider) {
        const session = await store.createSession(
          'api', undefined, null, apiProvider.id, apiProvider.models[0], undefined, undefined,
        );
        await store.selectSession(session.id);
      } else if (cliAgentType) {
        const session = await store.createSession(cliAgentType, undefined, null);
        await createAgentSession(session.id, cliAgentType);
        await store.selectSession(session.id);
      }
      setMode('session');
      setOpen(false);
    } catch {
      // 创建失败不打断面板：用户也可直接在会话默认页顶部的输入框开始
    } finally {
      setStarting(false);
    }
  };

  // ── 待处理：工具审批（进程重启残留的 stale 只读展示）+ 等待人工输入 ──
  const activeApprovals = pendingApprovals.filter((a) => !a.stale);
  const staleApprovals = pendingApprovals.filter((a) => a.stale);
  const pendingCount = activeApprovals.length + pendingInputs.length;

  /**
   * 待处理事项的**来源**：这条待办到底来自哪个模块、哪个任务。
   *
   * 待办本身只带 executionId / nodeId，光有节点名（如「T3 评估结果」）看不出是在处理谁的事——
   * 用户可能同时开着几个定时任务。这里按 executionId 反查执行实例，用触发方式 + 定义名拼出
   * 「定时触发「每日热点」」这类可识别的来源（触发方式用词与下面「进行中」一行保持一致）；
   * 实例已被清理时退回执行号短码，不编造内容。
   */
  const pendingSource = (executionId?: string): string => {
    if (!executionId) return '来源未知';
    const inst = instances.find((i) => i.id === executionId);
    if (!inst) return `工作流执行 ${executionId.slice(0, 8)}`;
    const name = inst.definitionName || inst.definitionId;
    const kind = inst.trigger === 'cron' ? '定时触发' : inst.trigger === 'event' ? '事件触发' : '工作流';
    return `${kind}「${name}」`;
  };

  // ── 进行中 ──
  const activeInstances = instances.filter((i) => ACTIVE_INSTANCE_STATUS.includes(i.status));
  const activeRooms = rooms.filter((r) => ACTIVE_ROOM_STATUS.includes(r.status));

  // ── 最近会话（排除工作流节点自动创建的内部会话）──
  const recentSessions = sessions
    .filter((s) => !isWorkflowSession(s.origin))
    .slice()
    .sort((a, b) => (b.updatedAt || 0) - (a.updatedAt || 0))
    .slice(0, MAX_RECENT_SESSIONS);

  const dimensions = usage?.dimensions ?? [];
  const usageTotals = dimensions.reduce(
    (acc, d) => {
      acc.total += d.totals.totalTokens;
      acc.read += d.totals.cacheReadTokens;
      acc.prompt += d.totals.promptTokens;
      acc.write += d.totals.cacheWriteTokens;
      return acc;
    },
    { total: 0, read: 0, prompt: 0, write: 0 },
  );
  const usageRate = usageTotals.prompt + usageTotals.read + usageTotals.write > 0
    ? (usageTotals.read / (usageTotals.prompt + usageTotals.read + usageTotals.write)) * 100
    : 0;

  return (
    <div
      className={inline ? 'flex-1 min-h-0 flex overflow-hidden' : 'fixed inset-0 z-[95] flex items-center justify-center p-6'}
      style={inline ? undefined : { backgroundColor: 'rgba(0,0,0,0.45)' }}
      onClick={inline ? undefined : closeCenter}
    >
      <div
        className={inline ? 'flex-1 min-h-0 flex flex-col overflow-hidden' : 'rounded-xl w-full max-w-[760px] flex flex-col overflow-hidden'}
        style={inline ? undefined : {
          backgroundColor: 'var(--bg-primary)',
          border: '1px solid var(--border)',
          maxHeight: '84vh',
          boxShadow: '0 12px 40px rgba(0,0,0,0.35)',
        }}
        onClick={inline ? undefined : (e) => e.stopPropagation()}
      >
        {/* 头部（内嵌态保留：给出区块身份与刷新入口，只是没有"关闭"可言） */}
        <div className="flex items-center gap-2 px-4 h-10 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <LayoutDashboard size={13} style={{ color: 'var(--accent)' }} />
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>指挥中心</span>
          <div className="flex-1" />
          <button
            onClick={() => void refreshCenter()}
            className="pd-btn p-1 rounded transition-colors"
            style={{ color: 'var(--text-secondary)' }}
            title="刷新"
          >
            {loadingUsage ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
          </button>
          {!inline && (
            <button
              onClick={closeCenter}
              className="pd-btn p-1 rounded transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="关闭"
            >
              <X size={12} />
            </button>
          )}
        </div>

        {/* 内容区：按**容器自身宽度**决定列数 —— 一列最小 320px，够宽就自动并成两列，
            且最多两列（`calc(50% - gap/2)` 保证两列恰好占满，不会出现第三列）。
            用容器宽度而不是窗口宽度：内嵌在会话默认页时容器比窗口窄得多，
            按窗口断点会硬塞两列，半列放不下区块内容就会看起来互相压叠。

            `gridAutoRows: max-content` 是必须的，不是美化：本容器高度被 flex 定死，内容总是高于它，
            容器内**没有剩余空间**；而 `auto` 行的基准尺寸取「项目的最小贡献」，卡片带 `overflow:hidden`
            时该值为 0（只剩 1px 上下边框）→ 行轨道被压成 2px，卡片溢出自己的行、被后来的行压住
            （即"板块重叠"）。`max-content` 让行的基准尺寸直接取内容高，不再依赖有无剩余空间；
            超出容器的部分交给 `overflow-y-auto` 滚动。 */}
        <div
          className="flex-1 min-h-0 overflow-y-auto pd-scroll-stable p-3 grid gap-3"
          style={{
            // min(…, 100%) 兜底：容器窄到不足 320px 时列宽跟着收，别让卡片横向溢出被裁掉
            gridTemplateColumns: 'repeat(auto-fit, minmax(min(max(320px, calc(50% - 6px)), 100%), 1fr))',
            gridAutoRows: 'max-content',
            alignContent: 'start',
            alignItems: 'start',
          }}
        >
          {/* 使用指引 + 使用向导（启动台职责）
              层级：指引块（主色底）→ 卡片 → 卡内「步骤 / 术语胶囊 + 说明」
              未配置 Agent 时向导常驻，配置完成后整块收起成一行常驻入口（见下方 else 分支） */}
          {showGuide ? (
            <div
              className="col-span-full min-w-0 overflow-hidden rounded-lg px-3 py-2.5 space-y-2"
              style={{ backgroundColor: 'var(--accent-light)', border: '1px solid var(--border)' }}
            >
              <div className="flex items-center justify-between">
                <span className="text-[11px] font-medium" style={{ color: 'var(--accent)' }}>
                  开始使用（顶栏可切换工作模式）
                </span>
                {(!guideSeen || guideForcedOpen) && (
                  <button
                    onClick={dismissGuide}
                    className="pd-btn px-2 py-0.5 rounded text-[10px] shrink-0"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                  >
                    {guideSeen ? '收起' : '知道了'}
                  </button>
                )}
              </div>

              {/* 使用向导：① 配置 Agent（前提）→ ② 开始首次会话 */}
              <div
                className="rounded-lg px-2.5 py-2"
                style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--accent)' }}
              >
                <div className="flex items-center gap-1.5 mb-2">
                  <span style={{ width: 2, height: 9, borderRadius: 1, backgroundColor: 'var(--accent)' }} />
                  <span className="text-[10px] font-semibold shrink-0" style={{ color: 'var(--text-primary)' }}>
                    使用向导
                  </span>
                  <span className="flex-1" style={{ height: 1, backgroundColor: 'var(--border)' }} />
                  {agentReady && <span className="text-[10px] shrink-0" style={{ color: '#10B981' }}>已就绪</span>}
                </div>

                <div className="flex items-start gap-2">
                  <StepIndex n={1} />
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
                      配置 Agent（使用前提，二选一或都配）
                    </div>
                    <div className="mt-1 space-y-1">
                      <WizardRow
                        label="API Agent"
                        ready={Boolean(apiProvider)}
                        hint={
                          apiProvider
                            ? `${apiProvider.name} · ${apiProvider.models[0]}`
                            : '设置 → API集成配置：添加提供商 + API Key + 至少一个模型'
                        }
                        actions={[{ label: '集成配置 ›', onClick: goApiConfig }]}
                      />
                      <WizardRow
                        label="CLI Agent"
                        ready={Boolean(cliAgentType)}
                        hint={
                          cliAgentType
                            ? `${cliAgentName || cliAgentType}`
                            : '先「环境检测」安装 CLI 工具，再到「集成配置」启用'
                        }
                        actions={[
                          { label: '集成配置 ›', onClick: goAgentConfig },
                          { label: '环境检测与安装 ›', onClick: goEnvConfig },
                        ]}
                      />
                    </div>
                  </div>
                </div>

                {/* 第②步与第①步的各行同构：左侧步骤号/名称、中间说明、右侧操作入口同行 */}
                <div className="flex items-center gap-2 mt-2.5">
                  <StepIndex n={2} />
                  <span className="shrink-0 text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
                    开始首次会话
                  </span>
                  <span
                    className="flex-1 min-w-0 truncate text-[10px]"
                    style={{ color: 'var(--text-tertiary)' }}
                    title={agentReady ? '进入会话：可直接让 Agent 问答、改文件、跑命令' : '请先完成第 1 步配置'}
                  >
                    {agentReady ? '进入会话：可直接让 Agent 问答、改文件、跑命令' : '请先完成第 1 步配置'}
                  </span>
                  <button
                    onClick={() => void startFirstSession()}
                    disabled={!agentReady || starting}
                    className="pd-btn shrink-0 px-2 py-0.5 rounded text-[10px]"
                    style={{
                      backgroundColor: agentReady ? 'var(--accent)' : 'var(--bg-tertiary)',
                      color: agentReady ? '#fff' : 'var(--text-tertiary)',
                      cursor: agentReady ? 'pointer' : 'not-allowed',
                    }}
                  >
                    {starting ? '正在创建…' : '开始首次会话 ›'}
                  </button>
                </div>
              </div>

              {/* 功能说明：首次展示，以及用户主动展开引导时一并展示
                  （未配置 Agent 的常驻态只留「使用向导」，避免这一大片长期占屏）；
                  左栏（五种工作模式）条目更短，按 2:3 分栏给它更窄的一列，右栏拿到更多行宽 */}
              {guideShowFull && (
                <div
                  className="grid gap-2"
                  style={{ gridTemplateColumns: 'minmax(0, 2fr) minmax(0, 3fr)' }}
                >
                  {GUIDE_GROUPS.map((group) => (
                    <div
                      key={group.title}
                      className="rounded-lg px-2.5 py-2"
                      style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--accent)' }}
                    >
                      {/* 组标题：主色竖条 + 标题 + 延伸细分隔线（与条目明显区分） */}
                      <div className="flex items-center gap-1.5 mb-1.5">
                        <span style={{ width: 2, height: 9, borderRadius: 1, backgroundColor: 'var(--accent)' }} />
                        <span className="text-[10px] font-semibold shrink-0" style={{ color: 'var(--text-primary)' }}>
                          {group.title}
                        </span>
                        <span className="flex-1" style={{ height: 1, backgroundColor: 'var(--border)' }} />
                      </div>
                      <ul className="space-y-1">
                        {group.items.map((it) => (
                          <li key={it.term} className="flex items-start gap-2">
                            {/* 术语胶囊：定宽居中，短词长词都能对齐成一列 */}
                            <span
                              className="shrink-0 rounded text-[10px]"
                              style={{
                                minWidth: 64,
                                textAlign: 'center',
                                padding: '1px 4px',
                                backgroundColor: 'var(--bg-tertiary)',
                                color: 'var(--text-primary)',
                              }}
                            >
                              {it.term}
                            </span>
                            <span className="flex-1 text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
                              {it.desc}
                            </span>
                          </li>
                        ))}
                      </ul>
                    </div>
                  ))}
                </div>
              )}

              {guideShowFull && (
                <div className="flex items-center justify-between">
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    三层记忆都会自动注入模型上下文（KV 记忆按需检索）。
                  </span>
                  <button onClick={goMemory} className="text-[10px] shrink-0" style={{ color: 'var(--accent)' }}>
                    去记忆管理 ›
                  </button>
                </div>
              )}
            </div>
          ) : (
            /* 引导收起后的常驻入口：留在它原本出现的位置（待处理之前），
               否则「看过一次」等于永久消失，想再对照步骤只能清 localStorage */
            <button
              onClick={() => setGuideForcedOpen(true)}
              className="col-span-full min-w-0 w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left transition-opacity hover:opacity-90"
              style={{ backgroundColor: 'var(--accent-light)', border: '1px solid var(--border)' }}
              title="展开使用引导：配置 Agent 与各功能说明"
            >
              <HelpCircle size={12} className="shrink-0" style={{ color: 'var(--accent)' }} />
              <span className="text-[11px] font-medium flex-1 truncate" style={{ color: 'var(--accent)' }}>
                开始使用
              </span>
              <span className="text-[10px] shrink-0" style={{ color: 'var(--accent)' }}>
                查看使用引导 ›
              </span>
            </button>
          )}

          {/* 待处理（占满整行：待办卡内含审批按钮与参数预览，塞进半列会挤） */}
          <Section title="待处理" icon={<ShieldAlert size={11} />} count={pendingCount} span>
            {pendingCount === 0 && staleApprovals.length === 0 && (
              <EmptyHint text="暂无待处理事项。" />
            )}
            <div className="space-y-1.5">
              {activeApprovals.map((a) => {
                const busy = pendingActionKey === a.callId;
                const source = pendingSource(a.executionId);
                return (
                  <div
                    key={a.callId}
                    className="px-2.5 py-2 rounded-lg"
                    style={{ backgroundColor: 'rgba(245,158,11,0.08)', border: '1px solid var(--border)' }}
                  >
                    <div className="flex items-center gap-2">
                      <ShieldAlert size={12} className="shrink-0" style={{ color: '#F59E0B' }} />
                      <div className="flex-1 min-w-0">
                        <div className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>
                          工具审批 · {a.nodeLabel}
                        </div>
                        <div
                          className="text-[10px] truncate"
                          style={{ color: 'var(--text-tertiary)' }}
                          title={`${source} · ${a.toolName} · 风险 ${a.risk} · ${a.arguments}`}
                        >
                          {source} · {a.toolName} · 风险 {a.risk} · {fmtAgo(a.createdAt)}
                        </div>
                      </div>
                      <button onClick={goWorkflow} className="text-[10px] shrink-0" style={{ color: 'var(--accent)' }}>
                        查看 ›
                      </button>
                    </div>
                    <div className="flex items-center gap-1.5 mt-1.5 pl-5 min-w-0">
                      <button
                        disabled={busy}
                        onClick={() => void decideApproval(a.callId, true)}
                        className="text-[10px] px-2 py-0.5 rounded shrink-0 disabled:opacity-50"
                        style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                      >
                        批准
                      </button>
                      <button
                        disabled={busy}
                        onClick={() => void decideApproval(a.callId, false)}
                        className="text-[10px] px-2 py-0.5 rounded shrink-0 disabled:opacity-50"
                        style={{ border: '1px solid var(--border)', color: 'var(--status-danger)' }}
                      >
                        拒绝
                      </button>
                      <span className="text-[10px] truncate font-mono" style={{ color: 'var(--text-tertiary)' }} title={a.arguments}>
                        {previewArgs(a.arguments)}
                      </span>
                    </div>
                  </div>
                );
              })}
              {pendingInputs.map((p) => {
                const key = `${p.executionId}:${p.nodeId}`;
                const busy = pendingActionKey === key;
                const draft = inputDrafts[key] ?? '';
                const source = pendingSource(p.executionId);
                return (
                  <div
                    key={key}
                    className="px-2.5 py-2 rounded-lg"
                    style={{ backgroundColor: 'rgba(245,158,11,0.08)', border: '1px solid var(--border)' }}
                  >
                    <div className="flex items-center gap-2">
                      <AlertTriangle size={12} className="shrink-0" style={{ color: '#F59E0B' }} />
                      <div className="flex-1 min-w-0">
                        <div className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>
                          等待人工输入 · {p.nodeLabel}
                        </div>
                        <div
                          className="text-[10px] truncate"
                          style={{ color: 'var(--text-tertiary)' }}
                          title={`${source} · ${p.prompt || '（无提示语）'}`}
                        >
                          {source} · {p.prompt || '（无提示语）'} · {fmtAgo(p.createdAt)}
                        </div>
                      </div>
                      <button onClick={goWorkflow} className="text-[10px] shrink-0" style={{ color: 'var(--accent)' }}>
                        查看 ›
                      </button>
                    </div>
                    <div className="flex items-center gap-1.5 mt-1.5 pl-5">
                      <input
                        value={draft}
                        onChange={(e) => setInputDrafts((prev) => ({ ...prev, [key]: e.target.value }))}
                        onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) void submitPendingInput(p); }}
                        placeholder="填写内容后提交"
                        className="flex-1 min-w-0 px-2 py-1 text-[11px] rounded outline-none"
                        style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                      />
                      <button
                        disabled={busy || !draft.trim()}
                        onClick={() => void submitPendingInput(p)}
                        className="text-[10px] px-2 py-0.5 rounded shrink-0 disabled:opacity-50"
                        style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                      >
                        提交
                      </button>
                    </div>
                  </div>
                );
              })}
              {staleApprovals.map((a) => (
                <div
                  key={a.callId}
                  className="flex items-center gap-2 px-2.5 py-2 rounded-lg"
                  style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                >
                  <ShieldAlert size={12} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      已失效的审批 · {a.nodeLabel}
                    </div>
                    <div className="text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }} title={pendingSource(a.executionId)}>
                      {pendingSource(a.executionId)} · {a.toolName} · 进程重启后不可再裁决，只能停止该执行
                    </div>
                  </div>
                  <button
                    disabled={pendingActionKey === a.executionId}
                    onClick={() => void stopExecution(a.executionId)}
                    className="text-[10px] shrink-0 px-2 py-0.5 rounded disabled:opacity-50"
                    style={{ border: '1px solid var(--border)', color: 'var(--status-danger)' }}
                  >
                    停止执行
                  </button>
                </div>
              ))}
            </div>
          </Section>

          {/* 进行中（占满整行：执行/房间行内含状态与来源，半列会挤） */}
          <Section title="进行中" icon={<Activity size={11} />} count={activeInstances.length + activeRooms.length} span>
            {activeInstances.length === 0 && activeRooms.length === 0 && (
              <EmptyHint text="暂无进行中的工作流执行或群聊房间。" />
            )}
            <div className="space-y-1.5">
              {activeInstances.slice(0, MAX_ACTIVE_ROWS).map((i) => (
                <div
                  key={i.id}
                  className="flex items-center gap-2 px-2.5 py-2 rounded-lg cursor-pointer transition-opacity hover:opacity-80"
                  style={{ backgroundColor: 'var(--bg-tertiary)' }}
                  onClick={goWorkflow}
                  title="进入工作流页查看"
                >
                  <WorkflowIcon size={12} className="shrink-0" style={{ color: 'var(--accent)' }} />
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>
                      {i.definitionName || '未命名工作流'}
                    </div>
                    <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      {INSTANCE_STATUS_LABEL[i.status] ?? i.status}
                      {/* 自动触发的标出来源：用户没点过运行，得知道是谁在跑 */}
                      {i.trigger === 'cron' ? ' · 定时触发' : i.trigger === 'event' ? ' · 事件触发' : ''}
                      {' · 开始于 '}{fmtAgo(i.startedAt)}
                    </div>
                  </div>
                  <ArrowRight size={11} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                </div>
              ))}
              {activeInstances.length > MAX_ACTIVE_ROWS && (
                <EmptyHint text={`还有 ${activeInstances.length - MAX_ACTIVE_ROWS} 个执行，进入工作流页查看全部。`} />
              )}
              {activeRooms.slice(0, MAX_ACTIVE_ROWS).map((r) => {
                const staleRun = r.status === 'running' && !roomsActorAlive[r.id];
                return (
                  <div
                    key={r.id}
                    className="flex items-center gap-2 px-2.5 py-2 rounded-lg cursor-pointer transition-opacity hover:opacity-80"
                    style={{ backgroundColor: 'var(--bg-tertiary)' }}
                    onClick={() => goRoom(r.id)}
                    title="进入该群聊房间"
                  >
                    <Users size={12} className="shrink-0" style={{ color: 'var(--accent)' }} />
                    <div className="flex-1 min-w-0">
                      <div className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>{r.title}</div>
                      <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        {staleRun ? '待恢复（进程已退出）' : ROOM_STATUS_LABEL[r.status] ?? r.status} · 更新于 {fmtAgo(r.updatedAt)}
                      </div>
                    </div>
                    <ArrowRight size={11} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                  </div>
                );
              })}
              {activeRooms.length > MAX_ACTIVE_ROWS && (
                <EmptyHint text={`还有 ${activeRooms.length - MAX_ACTIVE_ROWS} 个房间，进入群聊页查看全部。`} />
              )}
            </div>
          </Section>

          {/* 最近会话（与成本速览同行：会话在左、用量在右） */}
          <Section title="最近会话" icon={<History size={11} />}>
            {recentSessions.length === 0 ? (
              <EmptyHint text="暂无会话。" />
            ) : (
              <div className="space-y-1">
                {recentSessions.map((s) => (
                  <div
                    key={s.id}
                    className="flex items-center gap-2 px-2.5 py-1.5 rounded-lg cursor-pointer transition-opacity hover:opacity-80"
                    style={{ backgroundColor: 'var(--bg-tertiary)' }}
                    onClick={() => goSession(s.id)}
                    title={s.title || s.id}
                  >
                    <MessageSquare size={11} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                    <span className="text-[11px] flex-1 min-w-0 truncate" style={{ color: 'var(--text-primary)' }}>
                      {s.title || '未命名会话'}
                    </span>
                    <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                      {fmtAgo(s.updatedAt)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </Section>

          {/* 成本速览 */}
          <Section title={`成本速览（近 ${COMMAND_CENTER_USAGE_DAYS} 天）`} icon={<BarChart3 size={11} />}>
            {dimensions.length === 0 ? (
              <EmptyHint text={loadingUsage ? '加载中…' : '暂无用量数据。'} />
            ) : (
              <div className="space-y-1.5">
                {dimensions.map((d) => (
                  <div key={d.key} className="flex items-center gap-2 px-2.5 py-1.5 rounded-lg" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                    <span className="text-[11px] flex-1 min-w-0 truncate" style={{ color: 'var(--text-primary)' }}>
                      {DIMENSION_LABEL[d.key] ?? d.key}
                    </span>
                    <span className="text-[10px] shrink-0" style={{ color: 'var(--text-secondary)' }}>
                      {fmtTokens(d.totals.totalTokens)} token
                    </span>
                    <span className="text-[10px] shrink-0 w-14 text-right" style={{ color: 'var(--text-tertiary)' }}>
                      命中 {d.totals.cacheHitRate.toFixed(1)}%
                    </span>
                  </div>
                ))}
                <div className="flex items-center justify-between gap-2 px-2.5 pt-1">
                  <span className="text-[10px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                    合计 {fmtTokens(usageTotals.total)} token · 命中 {usageRate.toFixed(1)}%
                  </span>
                  <button onClick={goUsageStats} className="text-[10px] shrink-0" style={{ color: 'var(--accent)' }}>
                    查看完整用量 ›
                  </button>
                </div>
                <div className="text-[10px] px-2.5" style={{ color: 'var(--text-tertiary)' }}>
                  仅统计经宿主发起的 API 调用；CLI Agent（终端/插件）自行计费，不计入。
                </div>
              </div>
            )}
          </Section>
        </div>
      </div>
    </div>
  );
}
