import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import { Send, Square, Zap, Brain, GraduationCap, Cpu, ChevronUp, ClipboardList, ImagePlus, FileText, Paperclip, FolderOpen, X, Plus, MemoryStick } from 'lucide-react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { ChatMode, Session, Attachment } from '../../types';
import { MODE_LABELS, MODE_COLORS, getModePrompt } from '../../types';
import { InspirationPicker } from '../input/InspirationPicker';
import { SkillPicker } from '../input/SkillPicker';
import { VoiceInputButton } from '../input/VoiceInputButton';
import { ProjectMemoryDialog } from '../panels/ProjectMemoryDialog';
import { SecurityModeSelector, type SecurityModeValue } from '../security/SecurityModeSelector';
import { Select, type SelectGroup } from '../common/Select';
import { SessionUsageBar } from './SessionUsageBar';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useSessionStore } from '../../stores/sessionStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { useSessionDraftStore, GLOBAL_CWD, cliChoice, apiChoice, parseChoice } from '../../stores/sessionDraftStore';

import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useEnvInfo } from '../../hooks/useEnvInfo';


const MODE_ICONS: Record<ChatMode, typeof Send> = {
  native: Send,
  fast: Zap,
  think: Brain,
  expert: GraduationCap,
  plan: ClipboardList,
};

interface InputBarProps {
  session: Session | null;
  /**
   * 发送消息。**允许返回 Promise<boolean>**：false = 发送未成功（例如新建会话失败），
   * 此时输入框内容不清空，避免用户白打一遍。
   */
  onSend: (message: string, mode: ChatMode, attachments?: Attachment[]) => Promise<boolean> | void;
  /**
   * 无会话时"惰性建会话"：父级按草稿参数（工作目录 + 会话方式）创建并选中会话。
   * 返回新会话（失败/参数不完整返回 null → 本次发送中止且保留输入）。
   */
  onEnsureSession?: () => Promise<Session | null>;
  onStop?: () => void;
  isGenerating?: boolean;
  streamingStatus?: string;
  pendingInput?: string | null;
  onPendingConsumed?: () => void;
  /** 会话安全模式（显示在「文件」按钮右侧；None 时不渲染） */
  securityMode?: SecurityModeValue;
  onSecurityModeChange?: (v: SecurityModeValue) => void;
}

const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg']);
const MAX_ATTACHMENTS = 8;

function isImageName(name: string): boolean {
  const ext = name.split('.').pop()?.toLowerCase() || '';
  return IMAGE_EXTS.has(ext);
}

function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(new Error('读取文件失败'));
    reader.readAsDataURL(file);
  });
}

interface PendingAttachment {
  kind: 'image' | 'file';
  name: string;
  mime: string;
  path?: string;
  data?: string;
}

export function InputBar({ session, onSend, onEnsureSession, onStop, isGenerating, pendingInput, onPendingConsumed, securityMode, onSecurityModeChange }: InputBarProps) {
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<ChatMode>('native');
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  /**
   * 尚未落盘的附件（无会话时选择/拖拽/粘贴的都先放这里）：
   * 建会话需要工作目录，而"无会话"正是快捷开始路径，所以先把附件留在内存，
   * 等会话建好（发送时）再落盘到该会话的 attachments 目录。
   */
  const [pending, setPending] = useState<PendingAttachment[]>([]);
  const [sending, setSending] = useState(false);
  /**
   * 发送中的**同步**标记：Enter 走的是 keydown 闭包，可能落后于 React state，
   * 只看 `sending` 挡不住连按两次 Enter（第二次会在"建会话"还没返回时又发一次）。
   * ref 在所有闭包间共享，才是可靠的防重闸。
   */
  const sendingRef = useRef(false);
  const [showAttachMenu, setShowAttachMenu] = useState(false);
  const attachMenuRef = useRef<HTMLDivElement>(null);
  const [showCwdMenu, setShowCwdMenu] = useState(false);
  const cwdMenuRef = useRef<HTMLDivElement>(null);
  const [dragActive, setDragActive] = useState(false);
  const [draggingFiles, setDraggingFiles] = useState<{ name: string; kind: 'image' | 'file' }[]>([]);
  const [showInspirationPicker, setShowInspirationPicker] = useState(false);
  const [showSkillPicker, setShowSkillPicker] = useState(false);
  /** 项目记忆预览弹窗（原右侧面板「记忆」tab：只有只读预览，改弹窗不占 tab 位） */
  const [showMemoryDialog, setShowMemoryDialog] = useState(false);
  const [showModeDropdown, setShowModeDropdown] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const pickerAnchorRef = useRef<HTMLDivElement>(null);
  const modeDropdownRef = useRef<HTMLDivElement>(null);
  const inputBarRef = useRef<HTMLDivElement>(null);
  const attachmentsRef = useRef<Attachment[]>([]);

  // 同步最新附件到 ref，供拖拽/选择/粘贴超限判断使用
  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);
  // 同上：待落盘附件也走 ref，异步回调里读到的才是最新值
  const pendingRef = useRef<PendingAttachment[]>([]);
  useEffect(() => {
    pendingRef.current = pending;
  }, [pending]);

  // ── 会话方式 / 工作目录（无会话时的"快捷开始"草稿）──
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();
  const { getEnabledAgentTypes, getDisplayName, getTheme, fetchAgents } = useAgentRegistry();
  const { envInfo } = useEnvInfo();
  const draftChoice = useSessionDraftStore((s) => s.choice);
  const draftCwd = useSessionDraftStore((s) => s.cwd);
  const setDraftChoice = useSessionDraftStore((s) => s.setChoice);
  const setDraftCwd = useSessionDraftStore((s) => s.setCwd);
  const startNewSession = useSessionStore((s) => s.startNewSession);

  // 首次进入会话模式时把两份数据源拉起来（指挥中心等处也会拉，重复调用无害）
  useEffect(() => {
    fetchProviders().catch(() => {});
    fetchAgents().catch(() => {});
  }, [fetchProviders, fetchAgents]);

  /** 已安装且启用的 CLI Agent 类型（未安装的不列，避免选中后发送即失败） */
  const installedClis = useMemo(
    () => getEnabledAgentTypes().filter((t) => t !== 'api' && (envInfo?.agentVersions?.[t] ?? null) !== null),
    [getEnabledAgentTypes, envInfo],
  );

  /** API Agent 分组（已配 Key 且配了模型的提供商）：无会话新建与已有会话切换模型共用同一份 */
  const apiChoiceGroups: SelectGroup[] = useMemo(() => {
    // 只列"已配 Key 且配了模型"的提供商：没配 Key 的选中后发送必然报错
    const apiOptions = apiProviders
      .filter((p) => p.apiKeySet && p.models.length > 0)
      .flatMap((p) => p.models.map((m) => ({ value: apiChoice(p.id, m), label: `${p.name} · ${m}` })));
    return apiOptions.length > 0 ? [{ label: 'API Agent', options: apiOptions }] : [];
  }, [apiProviders]);

  /** 会话方式选择器的分组：CLI Agent / API Agent（组头即一级，选项即二级） */
  const choiceGroups: SelectGroup[] = useMemo(() => {
    const cliOptions = installedClis.map((t) => ({ value: cliChoice(t), label: getDisplayName(t) }));
    const groups: SelectGroup[] = [];
    if (cliOptions.length > 0) groups.push({ label: 'CLI Agent', options: cliOptions });
    groups.push(...apiChoiceGroups);
    return groups;
  }, [installedClis, apiChoiceGroups, getDisplayName]);

  // 默认会话方式：沿用上次选择；没有（或选项已失效，比如提供商/模型被删）则回落到第一个可用项
  // （优先 API，其次本机已装 CLI）。目录不设默认（必选项，由用户显式选定）。
  useEffect(() => {
    if (choiceGroups.length === 0) return;
    const available = choiceGroups.some((g) => g.options.some((o) => o.value === draftChoice));
    if (available) return;
    const first = choiceGroups.find((g) => g.label === 'API Agent')?.options[0] ?? choiceGroups[0].options[0];
    if (first) setDraftChoice(first.value);
  }, [draftChoice, choiceGroups, setDraftChoice]);

  /**
   * 无会话时的发送前置条件（**三者非空才可发送**）：工作目录 + 会话方式 + 消息本身。
   * 这样 CLI 未安装 / API 未配 Key 的报错在发送前就被挡住了（选项列表本身只列可用项）。
   */
  const draftReady = Boolean(draftCwd) && parseChoice(draftChoice) !== null;
  const blockReason = choiceGroups.length === 0
    ? '暂无可用的会话方式：请先在「设置」里配置 API 提供商或安装 CLI Agent'
    : !draftCwd
      ? '请先选择会话工作目录'
      : parseChoice(draftChoice) === null
        ? '请先选择会话方式（CLI Agent / API Agent）'
        : '';
  /** 同一拦截原因的**短文案**：占位行只有一行高，长句会被裁掉（见下方 placeholder 注释） */
  const blockHint = choiceGroups.length === 0
    ? '请先在「设置」配置 API 提供商或 CLI Agent'
    : !draftCwd
      ? '请先选择会话工作目录'
      : parseChoice(draftChoice) === null
        ? '请先选择会话方式'
        : '';
  /** 草稿目录的展示名（末级目录名 / 全局工作空间） */
  const draftCwdLabel = draftCwd === GLOBAL_CWD
    ? '全局工作空间'
    : draftCwd
      ? draftCwd.split(/[\\/]/).pop() || draftCwd
      : '';
  /** 草稿里的 agent 类型（无会话时给技能面板用：CLI 类型或 api） */
  const draftAgentType = parseChoice(draftChoice)?.agentType ?? '';
  /** 当前目录是否就是"全局工作空间"（会话下 = cwd 为空；草稿下 = 哨兵值），菜单里给该项打标 */
  const cwdIsGlobal = session ? !session.cwd : draftCwd === GLOBAL_CWD;

  // Load prompt descriptions for tooltip
  const [modeDescriptions, setModeDescriptions] = useState<Record<string, string>>({});
  useEffect(() => {
    (async () => {
      const descs: Record<string, string> = {};
      for (const m of ['native', 'fast', 'think', 'expert'] as const) {
        const p = await getModePrompt(m);
        if (p && p.trim() !== '') descs[m] = p;
        else descs[m] = '原生模式，使用默认对话风格';
      }
      setModeDescriptions(descs);
    })();
  }, []);
  useEffect(() => {
    if (!pendingInput) return;
    // 推到微任务：effect 体内同步 setState 会被 `react-hooks/set-state-in-effect` 判为级联渲染。
    // 同一个任务、早于绘制，行为一致（textarea 在本次提交已挂载，聚焦时序不变）。
    void Promise.resolve().then(() => {
      setInput(pendingInput);
      onPendingConsumed?.();
      textareaRef.current?.focus();
    });
  }, [pendingInput, onPendingConsumed]);

  // Close mode dropdown on outside click
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (modeDropdownRef.current && !modeDropdownRef.current.contains(e.target as Node)) {
        setShowModeDropdown(false);
      }
    };
    if (showModeDropdown) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [showModeDropdown]);

  // 「附件」「工作目录」两个小菜单的点击外部关闭（两者互斥，同时只开一个）
  useEffect(() => {
    if (!showAttachMenu && !showCwdMenu) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (showAttachMenu && !attachMenuRef.current?.contains(t)) setShowAttachMenu(false);
      if (showCwdMenu && !cwdMenuRef.current?.contains(t)) setShowCwdMenu(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [showAttachMenu, showCwdMenu]);

  /**
   * 工具栏宽度自适应：默认宽度（窗口里左侧有会话列表、右侧可能开面板）常常不够放
   * 「消息模式 + 技能 + 记忆 + 附件 + 安全模式 + 工作目录 + 会话方式 + 语音 + 发送」，
   * 之前会被压扁变形。这里按容器实际宽度降级（而不是硬挤）：
   * - compact（< 760px，按各控件实测宽度算出的临界值）：技能/记忆/附件/语音只留图标；
   * - 工作目录与会话方式**不设固定宽度**（响应式）：按内容自然展开（全屏时完整显示），
   *   空间不足才收缩到下限，截断由 truncate 兜底、完整值在 title 里；
   * - 再窄则由 flex-wrap 换行，保证任何宽度都不变形。
   * 语音按钮的位置来自"移除灵感按钮（右栏面板可替代）+ 目录/会话方式改响应式"腾出的空间。
   */
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const el = toolbarRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? el.clientWidth;
      setCompact(w < 760);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Auto-resize textarea：随内容增长（上限 200px）；**空输入必须回到默认高度**——
  // 默认两行由 rows={2} + .pd-composer-input 的 min-height 共同保证，这里清掉内联高度即可。
  // （此前只按 scrollHeight 写死内联高度，容器化后这层"多余空白"会变成一个明显偏高的输入框
  //（窄宽度时量出的 200px 会一直粘住，因为更新只依赖 input）。）
  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    if (!textarea.value) {
      textarea.style.height = ''; // 交回 CSS（默认两行 min-height）
      return;
    }
    textarea.style.height = 'auto';
    textarea.style.height = Math.min(textarea.scrollHeight, 200) + 'px';
  }, [input]);

  /** 把待落盘附件交给后端（选择/拖拽走路径复制，粘贴走字节兜底） */
  const saveToSession = useCallback(async (sessionId: string, items: PendingAttachment[]): Promise<Attachment[]> => {
    if (items.length === 0) return [];
    try {
      return await invoke<Attachment[]>('save_attachments', { sessionId, items });
    } catch (e) {
      showToast(`附件保存失败: ${errorMessage(e)}`, 'error');
      return [];
    }
  }, []);

  /**
   * 发送。无会话时先按草稿"惰性建会话"（父级负责 createSession + 选中 + CLI sidecar），
   * 再把待落盘附件落到新会话目录，最后交给父级发送。
   * 任何一步失败都**保留输入内容**（onSend 返回 false / 建会话返回 null 即中止）。
   */
  const handleSendInternal = useCallback(async () => {
    if (sendingRef.current) return; // 防重复：连按 Enter / Enter+点发送 都只发一次
    const trimmed = input.trim();
    const hasAttachments = attachments.length > 0 || pending.length > 0;
    if (!trimmed && !hasAttachments) return;
    if (!session && !draftReady) {
      showToast(blockReason, 'info');
      return;
    }
    sendingRef.current = true;
    setSending(true);
    try {
      let sess = session;
      if (!sess) {
        if (!onEnsureSession) return;
        sess = await onEnsureSession();
        if (!sess) return; // 建会话失败：保留输入与附件
      }
      // 待落盘附件：会话已就绪，落盘到该会话的 attachments 目录
      let finalAtts = attachments;
      if (pending.length > 0) {
        const saved = await saveToSession(sess.id, pending);
        pendingRef.current = [];
        setPending([]);
        finalAtts = [...finalAtts, ...saved].slice(0, MAX_ATTACHMENTS);
        setAttachments(finalAtts);
      }
      const ok = await onSend(trimmed, mode, finalAtts.length > 0 ? finalAtts : undefined);
      if (ok === false) return; // 发送未成功：不动输入框
      setInput('');
      setAttachments([]);
      pendingRef.current = [];
      setPending([]);
      if (textareaRef.current) {
        textareaRef.current.style.height = 'auto';
      }
    } finally {
      sendingRef.current = false;
      setSending(false);
    }
  }, [input, mode, attachments, pending, session, draftReady, blockReason, onSend, onEnsureSession, saveToSession]);

  /**
   * 新会话：把输入区切回"快捷开始"草稿态，并把当前会话的目录与会话方式带成草稿默认值
   * （同一目录继续追问新话题是最常见场景，省掉两次重选）。不建会话——首次发送时才建。
   * 附件按会话隔离，旧会话未发送的附件由"会话切换"副作用统一清理。
   */
  const handleStartNewSession = useCallback(() => {
    if (session) {
      setDraftCwd(session.cwd || GLOBAL_CWD);
      setDraftChoice(
        session.agentType === 'api' && session.apiProvider && session.apiModel
          ? apiChoice(session.apiProvider, session.apiModel)
          : cliChoice(session.agentType),
      );
    }
    startNewSession();
    // 切回草稿态后直接把光标交回输入框：用户按 Ctrl+N 就是为了立刻打字
    textareaRef.current?.focus();
  }, [session, setDraftCwd, setDraftChoice, startNewSession]);

  // 键盘快捷键：Ctrl+I/Ctrl+K 开关面板；Ctrl+N 新会话；Enter 发送（Shift+Enter 换行）。
  // 依赖里带上 handleSendInternal（它自身依赖齐了），避免闭包落后于最新的发送状态。
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Ctrl+N for new session
      if (e.ctrlKey && e.key === 'n') {
        e.preventDefault();
        handleStartNewSession();
        return;
      }
      // Ctrl+I for inspiration picker
      if (e.ctrlKey && e.key === 'i') {
        e.preventDefault();
        setShowInspirationPicker((v) => !v);
        setShowSkillPicker(false);
        return;
      }
      // Ctrl+K for skill picker
      if (e.ctrlKey && e.key === 'k') {
        e.preventDefault();
        setShowSkillPicker((v) => !v);
        setShowInspirationPicker(false);
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        void handleSendInternal();
      }
    },
    [handleSendInternal, handleStartNewSession],
  );

  /**
   * 收附件：已有会话 → 立即落盘；无会话（快捷开始路径）→ 先留在内存，
   * 发送时随新会话一起落盘，避免"选了附件却因为没有会话而被拒"。
   */
  const addFromPaths = useCallback(async (paths: string[], forceKind?: 'image' | 'file') => {
    const items: PendingAttachment[] = paths.map((p) => {
      const name = p.split(/[\\/]/).pop() || 'file';
      const kind: 'image' | 'file' = forceKind ?? (isImageName(name) ? 'image' : 'file');
      return { kind, name, mime: '', path: p };
    });

    const current = attachmentsRef.current.length + pendingRef.current.length;
    const room = MAX_ATTACHMENTS - current;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${items.length} 个`, 'info');
      return;
    }

    const take = items.slice(0, room);
    if (session) {
      const saved = await saveToSession(session.id, take);
      if (saved.length > 0) {
        setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
      }
    } else if (take.length > 0) {
      setPending((prev) => [...prev, ...take].slice(0, MAX_ATTACHMENTS));
    }

    const overflow = items.length - take.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [session, saveToSession]);

  const handlePickImages = useCallback(async () => {
    const selected = await openDialog({
      multiple: true,
      filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'] }],
    });
    if (!selected) return;
    await addFromPaths(Array.isArray(selected) ? selected : [selected], 'image');
  }, [addFromPaths]);

  const handlePickFiles = useCallback(async () => {
    const selected = await openDialog({ multiple: true });
    if (!selected) return;
    await addFromPaths(Array.isArray(selected) ? selected : [selected], 'file');
  }, [addFromPaths]);

  const updateSessionCwd = useSessionStore((s) => s.updateSessionCwd);
  const updateSessionModel = useSessionStore((s) => s.updateSessionModel);

  /**
   * 工作目录按钮：
   * - 已有会话 → 切换该会话的项目目录（下一条消息生效）；
   * - 无会话 → 写"快捷开始"草稿目录（不建会话：会话在首次发送时才创建，避免产生空会话）。
   *
   * 「使用全局工作空间」两侧都要有：新会话写哨兵值（创建时交后端解析），已有会话写空串
   * （运行期同样按默认工作区解析）——空串而不是绝对路径，之后改全局工作区设置会话会跟着变。
   */
  const handlePickProjectDir = useCallback(async (useGlobal = false) => {
    try {
      if (useGlobal) {
        if (session) {
          await updateSessionCwd(session.id, '');
          showToast('已切换为全局工作空间，下一条消息起生效', 'info');
        } else {
          setDraftCwd(GLOBAL_CWD);
          showToast('新会话将使用全局工作空间', 'info');
        }
        return;
      }
      const selected = await openDialog({ directory: true, multiple: false, title: '选择会话工作目录' });
      if (!selected || typeof selected !== 'string') return;
      if (session) {
        await updateSessionCwd(session.id, selected);
        showToast('项目目录已切换，下一条消息起生效', 'info');
      } else {
        setDraftCwd(selected);
      }
    } catch (e) {
      showToast(`选择工作目录失败: ${errorMessage(e)}`, 'error');
    }
  }, [session, updateSessionCwd, setDraftCwd]);

  /** 已有会话当前模型在选择器里的 value（`api:提供商::模型`） */
  const sessionChoice = session && session.agentType === 'api'
    ? apiChoice(session.apiProvider ?? '', session.apiModel ?? '')
    : '';

  /**
   * 已有会话的模型选项：只保留 API 分组（CLI 会话不在会话内换类型），并在候选表之外补上
   * **当前**这一项——提供商被删或模型被改名后，原值不在候选里，Select 会显示成占位符，
   * 用户就看不到自己在用什么模型了。
   */
  const sessionModelGroups: SelectGroup[] = useMemo(() => {
    if (!session || session.agentType !== 'api') return apiChoiceGroups;
    if (apiChoiceGroups.some((g) => g.options.some((o) => o.value === sessionChoice))) return apiChoiceGroups;
    const provider = apiProviders.find((p) => p.id === session.apiProvider);
    const label = `${provider?.name ?? `${session.apiProvider ?? ''}（提供商已失效）`} · ${session.apiModel ?? ''}（当前）`;
    return [{ label: '当前模型', options: [{ value: sessionChoice, label }] }, ...apiChoiceGroups];
  }, [session, sessionChoice, apiChoiceGroups, apiProviders]);

  /** 切换会话模型：仅 API 会话可用，切换后对**后续**消息生效（后端比较后再写） */
  const handleSwitchModel = useCallback(async (choice: string) => {
    if (!session || choice === sessionChoice) return;
    const parsed = parseChoice(choice);
    if (!parsed || parsed.agentType !== 'api' || !parsed.apiProvider || !parsed.apiModel) return;
    try {
      const applied = await updateSessionModel(session.id, parsed.apiProvider, parsed.apiModel);
      showToast(
        applied ? '会话模型已切换，下一条消息起生效' : '会话模型已被别处修改，已同步为最新值',
        'info',
      );
    } catch (e) {
      showToast(`切换模型失败: ${errorMessage(e)}`, 'error');
    }
  }, [session, sessionChoice, updateSessionModel]);

  const handlePaste = useCallback(async (e: React.ClipboardEvent) => {
    const files: File[] = [];
    const items = e.clipboardData?.items;
    if (items) {
      for (const item of Array.from(items)) {
        if (item.kind === 'file') {
          const f = item.getAsFile();
          if (f) files.push(f);
        }
      }
    }
    if (files.length === 0) return;
    e.preventDefault();
    const items2 = await Promise.all(files.map(async (f) => {
      const dataUrl = await fileToDataUrl(f);
      const mime = dataUrl.split(';')[0].split(':')[1] || f.type || '';
      const base64 = dataUrl.split(',')[1] || '';
      return {
        kind: (f.type.startsWith('image/') ? 'image' : 'file') as 'image' | 'file',
        name: f.name || 'pasted',
        mime,
        data: base64,
      };
    }));
    const current = attachmentsRef.current.length + pendingRef.current.length;
    const room = MAX_ATTACHMENTS - current;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${items2.length} 个`, 'info');
      return;
    }

    const take = items2.slice(0, room);
    if (session) {
      const saved = await saveToSession(session.id, take);
      if (saved.length > 0) {
        setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
      }
    } else if (take.length > 0) {
      // 无会话：先留在内存，发送时随新会话一起落盘
      setPending((prev) => [...prev, ...take].slice(0, MAX_ATTACHMENTS));
    }

    const overflow = items2.length - take.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [session, saveToSession]);

  // 拖拽文件落盘（悬停 InputBar 区时高亮附件区并虚拟展示，松开后落盘）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const isOverInputBar = (x: number, y: number): boolean => {
      const el = inputBarRef.current;
      if (!el) return false;
      const scale = window.devicePixelRatio || 1;
      const r = el.getBoundingClientRect();
      const lx = x / scale;
      const ly = y / scale;
      return lx >= r.left && lx <= r.right && ly >= r.top && ly <= r.bottom;
    };

    (async () => {
      try {
        unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          const type = payload.type;

          if (type === 'enter') {
            setDraggingFiles(
              payload.paths.map((p) => {
                const name = p.split(/[\\/]/).pop() || 'file';
                return { name, kind: isImageName(name) ? 'image' : 'file' } as const;
              }),
            );
            setDragActive(isOverInputBar(payload.position.x, payload.position.y));
          } else if (type === 'over') {
            setDragActive(isOverInputBar(payload.position.x, payload.position.y));
          } else if (type === 'leave') {
            setDragActive(false);
            setDraggingFiles([]);
          } else if (type === 'drop') {
            if (isOverInputBar(payload.position.x, payload.position.y)) {
              addFromPaths(payload.paths);
            }
            setDragActive(false);
            setDraggingFiles([]);
          }
        });
        if (disposed) unlisten();
      } catch (e) {
        console.error('[InputBar] 注册拖拽监听失败:', e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addFromPaths]);

  const removeAttachment = useCallback(async (idx: number) => {
    const target = attachments[idx];
    setAttachments((prev) => prev.filter((_, i) => i !== idx));
    if (target?.path && session) {
      try {
        await invoke('delete_attachment', { sessionId: session.id, path: target.path });
      } catch (e) {
        console.warn('[InputBar] 删除附件文件失败:', e);
      }
    }
  }, [attachments, session]);

  /** 移除"待落盘"附件（无会话时收进来的，尚未落盘，直接丢内存即可） */
  const removePending = useCallback((idx: number) => {
    setPending((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  // 会话切换时清理旧会话未发送的附件（删除落盘文件 + 清空状态）
  const prevSessionIdRef = useRef<string | null>(session?.id ?? null);
  useEffect(() => {
    const currentId = session?.id ?? null;
    const prevId = prevSessionIdRef.current;
    if (prevId === currentId) return;

    const old = attachmentsRef.current;
    old.forEach((att) => {
      if (att.path && prevId) {
        invoke('delete_attachment', { sessionId: prevId, path: att.path }).catch(() => {});
      }
    });
    setAttachments([]);
    // 待落盘附件同样作废（它们是为"上一个草稿"选的）
    pendingRef.current = [];
    setPending([]);
    prevSessionIdRef.current = currentId;
  }, [session?.id]);

  const attachCount = attachments.length + pending.length;
  /**
   * 容器内发送按钮的激活/禁用判据：有内容（文本或附件）+ 会话或草稿就绪 + 不在发送中。
   * 与 handleSendInternal 的门禁同源，避免按钮亮着却发不出去。
   */
  const canSend = (input.trim().length > 0 || attachCount > 0) && (Boolean(session) || draftReady) && !sending;
  const imageCount = [...pending, ...attachments].filter((a) => a.kind === 'image').length;
  const fileCount = [...pending, ...attachments].filter((a) => a.kind === 'file').length;

  // Placeholder based on context：无会话时是"快捷开始"。
  // 占位文案必须**一行放得下**：文本域只有一行高，超出的部分会被直接裁掉
  //（此前把 4-5 条快捷键 + 整句拦截原因全塞进来 → "提示显示不全"）。
  // 因此占位只留最短必要信息；完整快捷键在各按钮的悬浮提示里，
  // 拦截原因与全部快捷键挂在 textarea 的 title 上，悬停即可看全。
  const agentLabel = session ? getDisplayName(session.agentType) : '';
  const shortcutHint = 'Enter 发送 · Shift+Enter 换行';
  const fullHint = !session
    ? `${blockReason || '输入消息开始新会话'}（${shortcutHint} · Ctrl+I 灵感 · Ctrl+K 技能）`
    : `向 ${agentLabel} 发送消息（${shortcutHint} · Ctrl+I 灵感 · Ctrl+K 技能 · Ctrl+N 新会话）`;
  const placeholder = !session
    ? blockHint || `输入消息开始新会话…（${shortcutHint}）`
    : `向 ${agentLabel} 发送消息...（${shortcutHint}）`;

  /**
   * 语音输入的回退目标：设置 › 语音识别 未指定专用转写模型时，用「当前会话方式选择的模型」——
   * 已有 API 会话用会话的 provider/model；草稿态用草稿里选的 API 会话方式；
   * CLI 会话没有 API 提供商 → 返回 null（按钮会提示去设置里指定转写模型）。
   * 录音/转写/插文本都封装在 VoiceInputButton 里（群聊输入栏复用同一个组件）。
   */
  const voiceFallback = useMemo(() => {
    if (session) {
      if (session.agentType === 'api' && session.apiProvider && session.apiModel) {
        return { providerId: session.apiProvider, model: session.apiModel };
      }
      return null;
    }
    const parsed = parseChoice(draftChoice);
    if (parsed?.agentType === 'api' && parsed.apiProvider && parsed.apiModel) {
      return { providerId: parsed.apiProvider, model: parsed.apiModel };
    }
    return null;
  }, [session, draftChoice]);

  return (
    <div className="shrink-0" ref={inputBarRef}>
      {/* 等待提示条 */}
      {isGenerating && (
        <div className="flex items-center gap-2 px-4 py-1.5" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          <div className="flex items-center gap-1">
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '0ms' }} />
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '150ms' }} />
            <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: session ? getTheme(session.agentType).color : 'var(--text-tertiary)', animationDelay: '300ms' }} />
          </div>
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
            {'思考中...'}
          </span>
        </div>
      )}

      {/* Input area：容器化输入区（书写边界、聚焦态、拖拽态都挂在容器上，见 .pd-composer）。
          工具栏已并入本容器、与发送键同一行（文本域独占首行，工具组与发送键共享次行）。
          与消息列表之间只留 12px 安全距离（原先靠一条 border-top 分隔，已去掉）。
          底部留白与用量行**配成一组**：API 会话下方紧跟用量行（自带 4px 上内边距），
          这里只留 4px → 视觉间隙 8px，读作输入区的页脚；CLI 会话没有用量行，留回 12px，
          避免输入框紧贴状态栏。 */}
      <div className={session && session.agentType === 'api' ? 'px-4 pt-3 pb-1' : 'px-4 pt-3 pb-3'}>
        <div
          className="pd-composer"
          data-dragging={dragActive}
          data-disabled={!session && !draftReady}
        >
          <div className="pd-composer-body" ref={pickerAnchorRef}>
            {/* 附件预览：图片缩略图 + 文件 chip（拖拽悬停时由容器整体提示"可放置"） */}
            {(attachCount > 0 || dragActive) && (
              <div className="pd-composer-att">
              {/* 待落盘附件（无会话时选的）：用本地路径预览；粘贴的字节只有名字，显示成文件 chip */}
              {pending.map((att, idx) => (
                att.kind === 'image' && att.path ? (
                  <div key={`pending-${idx}-${att.name}`} className="pd-att-thumb" title="发送时会保存到新会话的附件目录">
                    <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                    <button onClick={() => removePending(idx)} className="pd-att-x" title="移除附件"><X size={9} /></button>
                  </div>
                ) : (
                  <div key={`pending-${idx}-${att.name}`} className="pd-att-file" title="发送时会保存到新会话的附件目录">
                    <FileText size={12} style={{ color: 'var(--text-secondary)', flexShrink: 0 }} />
                    <span className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>{att.name}</span>
                    <button onClick={() => removePending(idx)} className="pd-att-x" title="移除附件"><X size={9} /></button>
                  </div>
                )
              ))}
              {attachments.map((att, idx) => (
                att.kind === 'image' ? (
                  <div key={`${idx}-${att.path}`} className="pd-att-thumb" title={att.name}>
                    <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                    <button onClick={() => void removeAttachment(idx)} className="pd-att-x" title="移除附件"><X size={9} /></button>
                  </div>
                ) : (
                  <div key={`${idx}-${att.path}`} className="pd-att-file" title={att.name}>
                    <FileText size={12} style={{ color: 'var(--text-secondary)', flexShrink: 0 }} />
                    <span className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>{att.name}</span>
                    <button onClick={() => void removeAttachment(idx)} className="pd-att-x" title="移除附件"><X size={9} /></button>
                  </div>
                )
              ))}
              {/* 拖拽中的虚拟附件占位 */}
              {dragActive && draggingFiles.map((f, i) => (
                <div
                  key={`drag-${i}-${f.name}`}
                  className={f.kind === 'image' ? 'pd-att-thumb' : 'pd-att-file'}
                  style={{ borderStyle: 'dashed', borderColor: 'var(--accent)', opacity: 0.7 }}
                >
                  {f.kind === 'image' ? (
                    <ImagePlus size={16} style={{ color: 'var(--accent)' }} />
                  ) : (
                    <>
                      <FileText size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                      <span className="text-[11px] truncate" style={{ color: 'var(--text-secondary)' }}>{f.name}</span>
                    </>
                  )}
                </div>
              ))}
            </div>
          )}

            <textarea
              ref={textareaRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              onPaste={handlePaste}
              placeholder={placeholder}
              title={fullHint}
              rows={2}
              className="pd-composer-input"
            />

            {/* Picker panels */}
          {showInspirationPicker && (
            <InspirationPicker
              onSelect={(content) => {
                const ta = textareaRef.current;
                if (ta) {
                  const cursorPos = ta.selectionStart ?? ta.value.length;
                  const text = ta.value;
                  const before = text.slice(0, cursorPos);
                  const after = text.slice(cursorPos);
                  setInput(before + content + after);
                  requestAnimationFrame(() => {
                    ta.focus();
                    ta.selectionStart = ta.selectionEnd = cursorPos + content.length;
                  });
                } else {
                  setInput((prev) => prev + content);
                }
                setShowInspirationPicker(false);
              }}
              onClose={() => setShowInspirationPicker(false)}
            />
          )}
          {showSkillPicker && (
            <SkillPicker
              agentType={session?.agentType ?? draftAgentType}
              onSelect={(name) => {
                const ta = textareaRef.current;
                if (ta) {
                  const cursorPos = ta.selectionStart ?? ta.value.length;
                  const text = ta.value;
                  const before = text.slice(0, cursorPos);
                  const after = text.slice(cursorPos);
                  const insertion = `@${name} `;
                  setInput(before + insertion + after);
                  // Restore cursor position after insertion
                  requestAnimationFrame(() => {
                    ta.focus();
                    ta.selectionStart = ta.selectionEnd = cursorPos + insertion.length;
                  });
                } else {
                  setInput((prev) => prev + `@${name} `);
                }
                setShowSkillPicker(false);
              }}
              onClose={() => setShowSkillPicker(false)}
            />
          )}
          </div>

          {/* 主操作行：生成中显示停止，其余显示发送（按钮独占右下角一行，见 .pd-composer-actions） */}
          <div className="pd-composer-actions">
            {/* 工具栏：模式 / 技能 / 记忆 / 附件 / 安全模式 / 目录 / 会话方式 / 语音 —— 与发送键同排。
                按容器宽度降级（见 compact）；宽度不足时工具组自身换行，发送键始终贴同行右端 */}
            <div ref={toolbarRef} className="pd-composer-tools">
              {/* 新会话：选中了会话时给一条回"快捷开始"的路（否则只能去左侧列表点新建弹窗） */}
              {session && (
                <button
                  onClick={handleStartNewSession}
                  className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0"
                  style={{
                    color: 'var(--text-secondary)',
                    backgroundColor: 'var(--bg-tertiary)',
                    border: '1px solid var(--border)',
                    height: '24px',
                  }}
                  title="新建会话（Ctrl+N）：回到快捷开始，沿用当前会话的工作目录与会话方式"
                >
                  <Plus size={12} />
                  {!compact && '新会话'}
                </button>
              )}

              {/* Mode dropdown */}
              <div className="relative shrink-0" ref={modeDropdownRef}>
                <button
                  onClick={() => setShowModeDropdown((v) => !v)}
                  className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: MODE_COLORS[mode],
                    border: '1px solid var(--border)',
                    height: '24px',
                  }}
                  title={modeDescriptions[mode] || '加载中...'}
                >
                  {(() => {
                    const Icon = MODE_ICONS[mode];
                    return <Icon size={11} />;
                  })()}
                  {MODE_LABELS[mode]}
                  <ChevronUp size={11} style={{ color: 'var(--text-secondary)' }} />
                </button>
                {showModeDropdown && (
                  <div
                    className="absolute left-0 bottom-full mb-1 py-1 rounded-lg shadow-lg z-50"
                    style={{
                      backgroundColor: 'var(--bg-panel)',
                      border: '1px solid var(--border)',
                      minWidth: '110px',
                    }}
                  >
                    {(Object.keys(MODE_LABELS) as ChatMode[]).map((m) => {
                      const Icon = MODE_ICONS[m];
                      const isActive = mode === m;
                      return (
                        <button
                          key={m}
                          onClick={() => { setMode(m); setShowModeDropdown(false); }}
                          className="flex items-center gap-2 w-full px-3 py-1.5 text-xs transition-colors text-left"
                          style={{
                            color: isActive ? MODE_COLORS[m] : 'var(--text-primary)',
                            backgroundColor: isActive ? `${MODE_COLORS[m]}11` : 'transparent',
                          }}
                          title={modeDescriptions[m] || '加载中...'}
                        >
                          <Icon size={12} />
                          <span className="flex-1">{MODE_LABELS[m]}</span>
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>

              {/* 技能按钮（窄宽度下只留图标，标题里写全称）。
                  无会话时也能用：往输入框插文本，不依赖会话；技能列表按草稿选的 agent 类型取。
                  —— 灵感按钮已移除：右栏「灵感」面板本就支持把内容送进输入框（pendingInputStore），
                  这里再放一个纯属重复占位；快捷键 Ctrl+I 仍保留，需要就地搜索时照旧可用 */}
              <button
                onClick={() => { setShowSkillPicker((v) => !v); setShowInspirationPicker(false); }}
                className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0"
                style={{
                  color: showSkillPicker ? getTheme(session?.agentType ?? draftAgentType).color : 'var(--text-secondary)',
                  backgroundColor: showSkillPicker ? 'var(--border)' : 'transparent',
                }}
                title="技能列表 (Ctrl+K)"
              >
                <Cpu size={12} />
                {!compact && '技能'}
              </button>
              {/* 项目记忆：只读预览当前会话工作区的 MEMORY.md（无会话时为兜底记忆根，弹窗内会写明）。
                  与 技能 同排 —— 都是"看一眼/取一下"的会话级入口，不常驻占位。
                  图标用 MemoryStick（memory=记忆）并与 设置 › 记忆管理 一致：同排的模式下拉里 Brain 是「思考」模式，
                  两边都用大脑会让人分不清 */}
              <button
                onClick={() => setShowMemoryDialog(true)}
                className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0"
                style={{
                  color: showMemoryDialog ? 'var(--accent)' : 'var(--text-secondary)',
                  backgroundColor: showMemoryDialog ? 'var(--border)' : 'transparent',
                }}
                title="项目记忆（当前会话工作区的 MEMORY.md）"
              >
                <MemoryStick size={12} />
                {!compact && '记忆'}
              </button>

              {/* 附件：图片/文件合并为一个回形针按钮（菜单选类型），腾出工具栏宽度给「工作目录 + 会话方式」 */}
              <div className="relative shrink-0" ref={attachMenuRef}>
                <button
                  onClick={() => setShowAttachMenu((v) => !v)}
                  className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
                  style={{
                    color: attachCount > 0 || showAttachMenu ? 'var(--accent)' : 'var(--text-secondary)',
                    backgroundColor: showAttachMenu ? 'var(--border)' : 'transparent',
                  }}
                  title={`添加附件（支持拖拽/粘贴，最多 ${MAX_ATTACHMENTS} 个）`}
                >
                  <Paperclip size={12} />
                  {compact ? attachCount || '' : `附件${attachCount > 0 ? ` ${attachCount}` : ''}`}
                </button>
                {showAttachMenu && (
                  <div
                    className="absolute bottom-full left-0 mb-1 py-1 rounded-lg z-20"
                    style={{
                      backgroundColor: 'var(--bg-secondary)',
                      border: '1px solid var(--border)',
                      boxShadow: '0 8px 24px rgba(0,0,0,0.22)',
                      minWidth: 132,
                    }}
                  >
                    <button
                      onClick={() => { setShowAttachMenu(false); void handlePickImages(); }}
                      className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-primary)' }}
                    >
                      <ImagePlus size={12} />
                      选择图片{imageCount > 0 ? `（${imageCount}）` : ''}
                    </button>
                    <button
                      onClick={() => { setShowAttachMenu(false); void handlePickFiles(); }}
                      className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-primary)' }}
                    >
                      <FileText size={12} />
                      选择文件{fileCount > 0 ? `（${fileCount}）` : ''}
                    </button>
                  </div>
                )}
              </div>

              {/* 会话安全模式选择器（显示在附件按钮右侧） */}
              {securityMode && onSecurityModeChange && (
                <SecurityModeSelector value={securityMode} onChange={onSecurityModeChange} />
              )}

              {/* 工作目录：已有会话 → 切换该会话目录；无会话 → 选定"快捷开始"的草稿目录（必选）。
                  两条路径都给同一个菜单（选目录 / 使用全局工作空间），避免"新建能选全局、已有会话却不能"。
                  宽度三件套（会话方式同）：
                  · flex 1 1 0 —— basis 取 0：参与「是否换行」的预估宽度只有 minWidth，
                    不会因为内容长就先把整行挤到换行；腾出的剩余空间由它瓜分，不够则收缩；
                  · maxWidth: max-content —— 最多只长到自身内容那么宽（内容自适应，
                    不会被拉伸成整行）；多出来的空间留在工具栏右侧（发送键之前）；
                  · minWidth 必须落在**这个 flex 项**上：只写进里面的 button 无效——
                    项自身仍会被压到 96 以下，而 button 保持 96 → 溢出并压住右边一个工具。
                  截断由内层 truncate 兜底，完整值在 title 里 */}
              <div className="relative" ref={cwdMenuRef} style={{ flex: '1 1 0', minWidth: 96, maxWidth: 'max-content' }}>
                <button
                  onClick={() => setShowCwdMenu((v) => !v)}
                  className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors min-w-0 w-full"
                  style={{
                    color: (session ? session.cwd : draftCwd) ? 'var(--text-primary)' : '#F59E0B',
                    border: !session && !draftCwd ? '1px solid rgba(245,158,11,0.5)' : '1px solid transparent',
                    backgroundColor: 'transparent',
                  }}
                  title={
                    session
                      ? session.cwd
                        ? `项目目录：${session.cwd}\n点击可切换（下一条消息生效）`
                        : '当前使用全局工作空间，点击可切换'
                      : draftCwd
                        ? `新会话工作目录：${draftCwd === GLOBAL_CWD ? '全局工作空间' : draftCwd}\n点击可重选`
                        : '请选择新会话的工作目录（必选）'
                  }
                >
                  <FolderOpen size={12} style={{ flexShrink: 0 }} />
                  <span className="truncate">
                    {session
                      ? (session.cwd ? session.cwd.split(/[\\/]/).pop() || session.cwd : '全局工作空间')
                      : (draftCwdLabel || '选择工作目录')}
                  </span>
                </button>
                {showCwdMenu && (
                  <div
                    className="absolute bottom-full left-0 mb-1 py-1 rounded-lg z-20"
                    style={{
                      backgroundColor: 'var(--bg-secondary)',
                      border: '1px solid var(--border)',
                      boxShadow: '0 8px 24px rgba(0,0,0,0.22)',
                      minWidth: 168,
                    }}
                  >
                    <button
                      onClick={() => { setShowCwdMenu(false); void handlePickProjectDir(); }}
                      className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-primary)' }}
                    >
                      <FolderOpen size={12} />
                      选择目录…
                    </button>
                    <button
                      onClick={() => { setShowCwdMenu(false); void handlePickProjectDir(true); }}
                      className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                      style={{ color: 'var(--text-primary)' }}
                    >
                      <FolderOpen size={12} />
                      {cwdIsGlobal ? '使用全局工作空间（当前）' : '使用全局工作空间'}
                    </button>
                  </div>
                )}
              </div>

              {/* 会话方式：无会话时可选（CLI / API 分组二级菜单 + 未选项则不允许发送）；
                  API 会话可直接切换模型（对后续消息生效）；CLI 会话仍只读展示（换类型等于换会话）。
                  宽度规则同「工作目录」：flex 1 1 0（预估宽度只有 minWidth，不挤换行）
                  + maxWidth max-content（最多长到内容宽，不拉伸）+ minWidth 120（收缩下限） */}
              {session && session.agentType === 'api' ? (
                <Select
                  value={sessionChoice}
                  onChange={handleSwitchModel}
                  groups={sessionModelGroups}
                  size="xs"
                  placeholder="选择模型"
                  disabled={sessionModelGroups.length === 0 || isGenerating}
                  style={{ minWidth: 120, maxWidth: 'max-content', flex: '1 1 0' }}
                  panelMinWidth={230}
                  title={
                    isGenerating
                      ? '生成中不能切换模型，生成结束后可切换'
                      : sessionModelGroups.length === 0
                        ? '暂无可切换的模型：请先在「设置」里配置 API 提供商'
                        : '当前会话的模型，切换后对后续消息生效'
                  }
                />
              ) : session ? (
                <span
                  className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs truncate"
                  style={{ color: 'var(--text-tertiary)', height: 24, minWidth: 120, maxWidth: 'max-content', flex: '1 1 0' }}
                  title="当前会话的会话方式（CLI 会话另建会话才能换）"
                >
                  <Cpu size={12} style={{ flexShrink: 0 }} />
                  <span className="truncate">{getDisplayName(session.agentType)}</span>
                </span>
              ) : (
                <Select
                  value={draftChoice}
                  onChange={setDraftChoice}
                  groups={choiceGroups}
                  size="xs"
                  placeholder="选择会话方式"
                  disabled={choiceGroups.length === 0}
                  style={{ minWidth: 120, maxWidth: 'max-content', flex: '1 1 0' }}
                  panelMinWidth={230}
                  title={
                    choiceGroups.length === 0
                      ? '暂无可用的会话方式：请先在「设置」里配置 API 提供商或安装 CLI Agent'
                      : '新会话的会话方式（CLI Agent / API Agent）'
                  }
                />
              )}

              {/* 语音输入：工具栏最后一项（发送键之前）。录音结束自动转写，文本追加进输入框 */}
              <VoiceInputButton
                fallbackTarget={voiceFallback}
                compact={compact}
                onTranscribed={(text) => {
                  // 追加而非覆盖：用户可能已经写了一半再补一句
                  setInput((prev) => (prev.trim() ? `${prev.replace(/\s+$/, '')} ${text}` : text));
                  textareaRef.current?.focus();
                }}
              />
            </div>
            {isGenerating ? (
              <button
                onClick={onStop}
                className="pd-composer-send"
                data-variant="stop"
                title="停止生成"
              >
                <Square size={13} />
              </button>
            ) : (
              <button
                onClick={() => void handleSendInternal()}
                disabled={!canSend}
                className="pd-composer-send"
                data-active={canSend}
                title={!session && !draftReady ? blockReason : '发送'}
              >
                <Send size={13} />
              </button>
            )}
          </div>
        </div>
      </div>

      {session && <SessionUsageBar sessionId={session.id} agentType={session.agentType ?? null} />}

      {showMemoryDialog && <ProjectMemoryDialog onClose={() => setShowMemoryDialog(false)} />}
    </div>
  );
}
