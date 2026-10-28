/**
 * FeedbackDialog — 全局意见反馈弹窗（App 级挂载一次）。
 *
 * 两个入口共用同一实现（见 `feedbackStore.openFeedback`）：
 * - 通知中心：每条错误/警告通知的「反馈」按钮（带通知标题、明细与时间点）；
 * - 设置 › 关于 › 「意见反馈」（空白表单）。
 *
 * 设计要点：
 * - **必须登录**：没有账号归属的反馈在平台用户中心不可见、也无法邮件回访；
 * - 日志片段来自后端日志文件的倒序读取（`read_recent_logs`），不新造存储；
 * - 附带的日志默认只取最近的错误/警告，且排除环境探测噪音（按 target 前缀）；
 * - 草稿存 localStorage，关闭 / 重启都不丢；提交成功后清空。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  CheckCircle2,
  Copy,
  Loader2,
  LogIn,
  MessageSquare,
  X,
} from 'lucide-react';

import { useAccountStore } from '../../stores/accountStore';
import { useFeedbackStore, type FeedbackCategory, type FeedbackPreset } from '../../stores/feedbackStore';

const DRAFT_KEY = 'pilotdesk.feedback.draft';
const MAX_LOG_LINES = 8;
/** 问题描述上限：与后端 `MAX_CONTENT_CHARS` 保持一致，倒逼用户精简描述 */
const MAX_CONTENT_CHARS = 500;

/**
 * 环境探测类日志的 target 前缀：版本查询、启动命令探测、ACP 握手探测、安装流程。
 *
 * 这些是**预期内**的高频噪音（未安装的 Agent 每次启动都会刷一遍），
 * 默认不进反馈摘要；需要排查环境问题时可在「检测日志」页查看。
 */
const PROBE_TARGETS = [
  'pilotdesk_lib::commands::env',
  'pilotdesk_lib::agent::lifecycle',
  'pilotdesk_lib::commands::agent_market',
  'pilotdesk::commands::env',
  'pilotdesk::agent::lifecycle',
  'pilotdesk::commands::agent_market',
];

const CATEGORIES: Array<{ key: FeedbackCategory; label: string }> = [
  { key: 'bug', label: '问题反馈' },
  { key: 'suggestion', label: '功能建议' },
  { key: 'other', label: '其他' },
];

/** 环境信息里用户可选的分项 */
interface EnvItems {
  app: boolean;
  system: boolean;
  tools: boolean;
  agents: boolean;
}

const DEFAULT_ENV_ITEMS: EnvItems = { app: true, system: true, tools: true, agents: true };

interface Draft {
  category: FeedbackCategory;
  title: string;
  content: string;
  email: string;
  includeLogs: boolean;
  /** 是否附带环境信息（整块开关） */
  includeEnv: boolean;
  /** 环境信息内逐项开关 */
  envItems: EnvItems;
}

interface LogEntry {
  ts: number;
  level: string;
  target: string;
  message: string;
}

interface LogPage {
  entries: LogEntry[];
  coverageStart: number;
  scannedFiles: number;
}

interface SubmitResult {
  id: string;
  receivedAt: number;
  status: string;
}

/** 随反馈提交的环境信息（与后端 `read_feedback_env` 同构，用于向用户预览） */
interface EnvAgent {
  agentType: string;
  displayName: string;
  transport: string;
  version?: string;
  installed?: boolean;
  available?: boolean;
}

interface EnvSummary {
  appType: string;
  appVersion: string;
  channel: string;
  os: string;
  arch: string;
  nodeVersion?: string;
  npmVersion?: string;
  gitVersion?: string;
  pythonVersion?: string;
  agents: EnvAgent[];
}

const OS_LABELS: Record<string, string> = {
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
};
const CHANNEL_LABELS: Record<string, string> = {
  dev: '开发构建',
  stable: '正式构建',
};
/** 应用形态：Web 版 / 移动版接入同一平台时按此区分 */
const APP_TYPE_LABELS: Record<string, string> = {
  desktop: '桌面端',
  web: 'Web 端',
  mobile: '移动端',
};

const EMPTY_DRAFT: Draft = {
  category: 'bug',
  title: '',
  content: '',
  email: '',
  includeLogs: true,
  includeEnv: true,
  envItems: { ...DEFAULT_ENV_ITEMS },
};

function loadDraft(): Draft {
  try {
    const raw = localStorage.getItem(DRAFT_KEY);
    if (!raw) return { ...EMPTY_DRAFT, envItems: { ...DEFAULT_ENV_ITEMS } };
    const parsed = JSON.parse(raw) as Partial<Draft>;
    return {
      category:
        parsed.category === 'suggestion' || parsed.category === 'other' ? parsed.category : 'bug',
      title: typeof parsed.title === 'string' ? parsed.title : '',
      content: typeof parsed.content === 'string' ? parsed.content : '',
      email: typeof parsed.email === 'string' ? parsed.email : '',
      includeLogs: parsed.includeLogs !== false,
      includeEnv: parsed.includeEnv !== false,
      envItems: {
        app: parsed.envItems?.app !== false,
        system: parsed.envItems?.system !== false,
        tools: parsed.envItems?.tools !== false,
        agents: parsed.envItems?.agents !== false,
      },
    };
  } catch {
    return { ...EMPTY_DRAFT, envItems: { ...DEFAULT_ENV_ITEMS } };
  }
}

function newRequestId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function shortTarget(target: string): string {
  const parts = target.split('::');
  return parts[parts.length - 1] || target;
}

/** 环境信息的一个可勾选分项：勾选框 + 分项名 + 实际将发送的取值（可折行） */
function EnvOption({
  label,
  checked,
  onToggle,
  value,
}: {
  label: string;
  checked: boolean;
  onToggle: (next: boolean) => void;
  value?: string;
}) {
  return (
    <label className="flex items-start gap-2 cursor-pointer select-none">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onToggle(e.target.checked)}
        className="shrink-0"
        style={{ width: 13, height: 13, marginTop: 1 }}
      />
      <span className="flex-1 min-w-0 flex items-start gap-2">
        <span
          className="text-[10px] shrink-0"
          style={{ color: checked ? 'var(--text-secondary)' : 'var(--text-tertiary)', width: 96 }}
        >
          {label}
        </span>
        <span
          className="flex-1 min-w-0 break-all text-[10px]"
          style={{ color: 'var(--text-tertiary)', opacity: checked ? 1 : 0.5 }}
        >
          {value ?? ''}
        </span>
      </span>
    </label>
  );
}

export function FeedbackDialog() {
  const open = useFeedbackStore((s) => s.open);
  const preset = useFeedbackStore((s) => s.preset);
  const close = useFeedbackStore((s) => s.close);

  const account = useAccountStore((s) => s.account);
  const loggingIn = useAccountStore((s) => s.loggingIn);
  const login = useAccountStore((s) => s.login);

  const [draft, setDraft] = useState<Draft>(loadDraft);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [logsLoading, setLogsLoading] = useState(false);
  const [logsError, setLogsError] = useState('');
  const [coverageStart, setCoverageStart] = useState(0);
  const [focusTs, setFocusTs] = useState(0);
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<SubmitResult | null>(null);
  const [copied, setCopied] = useState(false);
  const [loginHint, setLoginHint] = useState(false);
  const [env, setEnv] = useState<EnvSummary | null>(null);
  const [envLoading, setEnvLoading] = useState(false);
  const [envError, setEnvError] = useState('');

  /** 幂等键：同一次填写在失败重试时复用，避免平台重复建单 */
  const requestIdRef = useRef('');
  /** 已应用过的预填对象，防止同一次打开重复覆盖用户编辑 */
  const appliedPresetRef = useRef<FeedbackPreset | null>(null);
  /** 账号邮箱只预填一次，之后尊重用户改动（包括清空） */
  const emailSeededRef = useRef(false);
  /** 登录成功后自动续提交（用户点过「登录并继续」） */
  const autoSubmitRef = useRef(false);
  const submitRef = useRef<() => void>(() => {});

  // 草稿落盘：关闭弹窗 / 切页 / 重启都不丢
  useEffect(() => {
    try {
      localStorage.setItem(DRAFT_KEY, JSON.stringify(draft));
    } catch {
      // localStorage 不可用（隐私模式等）不影响填写
    }
  }, [draft]);

  // 预填：从通知中心进来时带入标题/明细/分类
  useEffect(() => {
    if (!open) return;
    if (preset && appliedPresetRef.current !== preset) {
      appliedPresetRef.current = preset;
      setDraft((d) => ({
        ...d,
        category: preset.category ?? d.category,
        title: preset.title ? preset.title.slice(0, 80) : d.title,
        content: preset.detail ? preset.detail.slice(0, MAX_CONTENT_CHARS) : d.content,
      }));
      setFocusTs(preset.focusTs ?? 0);
      setResult(null);
    }
  }, [open, preset]);

  // 账号邮箱预填（仅首次）
  useEffect(() => {
    if (emailSeededRef.current || !account?.email) return;
    emailSeededRef.current = true;
    setDraft((d) => (d.email ? d : { ...d, email: account.email }));
  }, [account?.email]);

  // 打开时按聚焦时间点加载日志；清除聚焦后重新加载
  useEffect(() => {
    if (!open) return;
    let alive = true;
    // loading/error 的重置放到微任务里：effect 体同步 setState 会引发级联渲染
    queueMicrotask(() => {
      if (!alive) return;
      setLogsLoading(true);
      setLogsError('');
    });
    void invoke<LogPage>('read_recent_logs', {
      query: {
        levels: ['warn', 'error'],
        excludeTargets: PROBE_TARGETS,
        sinceTs: 0,
        limit: 40,
        focusTs,
        focusWindowSecs: 180,
      },
    })
      .then((page) => {
        if (!alive) return;
        setLogs(page.entries);
        setCoverageStart(page.coverageStart);
      })
      .catch((e) => {
        if (!alive) return;
        setLogs([]);
        setLogsError(String(e));
      })
      .finally(() => {
        if (alive) setLogsLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [open, focusTs]);

  // 打开（或重新勾选环境信息）时读取「将随反馈提交的环境信息」，
  // 让用户先看到发送内容再决定提交哪几项
  useEffect(() => {
    if (!open || !draft.includeEnv) return;
    let alive = true;
    queueMicrotask(() => {
      if (!alive) return;
      setEnvLoading(true);
      setEnvError('');
    });
    void invoke<EnvSummary>('read_feedback_env')
      .then((summary) => {
        if (!alive) return;
        setEnv(summary);
      })
      .catch((e) => {
        if (!alive) return;
        setEnv(null);
        setEnvError(String(e));
      })
      .finally(() => {
        if (alive) setEnvLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [open, draft.includeEnv]);

  // 默认勾选最近 8 条
  useEffect(() => {
    const next: Record<string, boolean> = {};
    logs.slice(0, MAX_LOG_LINES).forEach((e, i) => {
      next[`${e.ts}:${i}`] = true;
    });
    queueMicrotask(() => setSelected(next));
  }, [logs]);

  // Esc 关闭（提交中禁用）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !submitting) close();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, submitting, close]);

  const selectedLogs = useMemo(
    () => logs.filter((e, i) => selected[`${e.ts}:${i}`]).slice(0, MAX_LOG_LINES),
    [logs, selected],
  );

  const doSubmit = useCallback(async () => {
    if (submitting) return;
    setError('');
    setLoginHint(false);

    if (!account) {
      setError('提交反馈需要登录平台账号（用于在用户中心查看进度、接收回复）');
      setLoginHint(true);
      return;
    }
    if (!draft.title.trim()) {
      setError('请填写标题');
      return;
    }
    if (!draft.content.trim()) {
      setError('请填写问题描述');
      return;
    }
    if (!requestIdRef.current) requestIdRef.current = newRequestId();

    setSubmitting(true);
    try {
      const res = await invoke<SubmitResult>('submit_feedback', {
        input: {
          category: draft.category,
          title: draft.title.trim(),
          content: draft.content.trim(),
          contactEmail: draft.email.trim() || null,
          clientRequestId: requestIdRef.current,
          logs: draft.includeLogs
            ? selectedLogs.map((e) => ({
                ts: e.ts,
                level: e.level,
                target: e.target,
                message: e.message,
              }))
            : [],
          // 环境信息整块不勾选时传 null，后端不采集也不发送任何环境字段
          env: draft.includeEnv ? { ...draft.envItems } : null,
          locale: navigator.language,
        },
      });
      requestIdRef.current = '';
      setResult(res);
      setDraft({ ...EMPTY_DRAFT, email: account.email ?? '' });
      try {
        localStorage.removeItem(DRAFT_KEY);
      } catch {
        // 忽略
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  }, [account, draft, selectedLogs, submitting]);

  // 供「登录并继续」在账号就绪后调用最新版本
  useEffect(() => {
    submitRef.current = () => void doSubmit();
  }, [doSubmit]);

  // 登录完成后自动续提交一次
  useEffect(() => {
    if (!account || !autoSubmitRef.current) return;
    autoSubmitRef.current = false;
    submitRef.current();
  }, [account]);

  if (!open) return null;

  const startLogin = async () => {
    autoSubmitRef.current = true;
    try {
      await login();
    } catch (e) {
      autoSubmitRef.current = false;
      setError(String(e));
    }
  };

  const toggleLog = (key: string) => {
    setSelected((prev) => {
      const on = !prev[key];
      if (on && Object.values(prev).filter(Boolean).length >= MAX_LOG_LINES) return prev;
      return { ...prev, [key]: on };
    });
  };

  const copyId = async () => {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.id);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // 剪贴板不可用时不提示
    }
  };

  const selectedCount = selectedLogs.length;
  const titleLeft = 80 - draft.title.length;

  return (
    <div
      className="fixed inset-0 z-[135] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={() => {
        if (!submitting) close();
      }}
    >
      <div
        className="rounded-xl w-full mx-4 flex flex-col"
        style={{
          maxWidth: 640,
          maxHeight: '84vh',
          backgroundColor: 'var(--bg-secondary)',
          border: '1px solid var(--border)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div
          className="flex items-center gap-2 px-5 h-11 shrink-0"
          style={{ borderBottom: '1px solid var(--border)' }}
        >
          <MessageSquare size={13} style={{ color: 'var(--accent)' }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            意见反馈
          </span>
          {preset?.title && (
            <span
              className="text-[10px] px-1.5 py-0.5 rounded-full"
              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
              title={`已从通知预填：${preset.title}`}
            >
              已从通知预填
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={() => {
              if (!submitting) close();
            }}
            disabled={submitting}
            className="pd-btn p-1 rounded transition-colors disabled:opacity-40"
            style={{ color: 'var(--text-secondary)' }}
            title="关闭（内容会保留为草稿）"
          >
            <X size={13} />
          </button>
        </div>

        {/* 提示横幅：固定在顶部行。
            放在表单滚动区内时，内容一长（日志列表撑高）就会被滚到可视区外，
            而提交失败恰恰是用户最需要看到提示的时刻。 */}
        {!result && error && (
          <div className="px-5 pt-3 shrink-0">
            <div
              className="flex items-center gap-2 text-[11px] px-2.5 py-1.5 rounded-lg"
              style={{ backgroundColor: 'rgba(239,68,68,0.08)', color: '#EF4444' }}
            >
              <span className="flex-1 min-w-0 break-words">{error}</span>
              {loginHint && (
                <button
                  onClick={() => void startLogin()}
                  disabled={loggingIn}
                  className="pd-btn shrink-0 flex items-center gap-1 text-[11px] px-2 py-1 rounded transition-colors disabled:opacity-50"
                  style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                >
                  {loggingIn ? <Loader2 size={11} className="animate-spin" /> : <LogIn size={11} />}
                  {loggingIn ? '登录中…' : '登录并继续'}
                </button>
              )}
            </div>
          </div>
        )}

        {result ? (
          /* 成功回执 */
          <div className="px-5 py-10 flex flex-col items-center gap-3">
            <CheckCircle2 size={30} style={{ color: '#10B981' }} />
            <div className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
              反馈已提交
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                回执编号
              </span>
              <span
                className="text-xs font-mono px-2 py-0.5 rounded"
                style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)' }}
              >
                {result.id}
              </span>
              <button
                onClick={() => void copyId()}
                className="pd-btn p-1 rounded transition-colors"
                style={{ color: copied ? '#10B981' : 'var(--text-tertiary)' }}
                title="复制编号"
              >
                <Copy size={11} />
              </button>
            </div>
            <p className="text-[11px] text-center leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              可在平台用户中心的反馈记录中查看处理进度；
              <br />
              若填写了邮箱，回复时会同时通过邮件通知你。
            </p>
            <div className="flex gap-2 mt-2">
              <button
                onClick={() => setResult(null)}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{
                  border: '1px solid var(--border)',
                  background: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                }}
              >
                再提交一条
              </button>
              <button
                onClick={close}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                关闭
              </button>
            </div>
          </div>
        ) : (
          <>
            {/* 表单 */}
            <div className="px-5 py-4 overflow-y-auto pd-scroll-stable flex flex-col gap-3" style={{ minHeight: 0 }}>
              {/* 分类 */}
              <div className="flex items-center gap-1.5">
                {CATEGORIES.map(({ key, label }) => {
                  const active = draft.category === key;
                  return (
                    <button
                      key={key}
                      onClick={() => setDraft((d) => ({ ...d, category: key }))}
                      className="pd-btn text-[11px] px-2.5 py-1 rounded-full transition-colors"
                      style={{
                        border: `1px solid ${active ? 'var(--accent)' : 'var(--border)'}`,
                        backgroundColor: active ? 'var(--accent-light)' : 'transparent',
                        color: active ? 'var(--accent)' : 'var(--text-tertiary)',
                      }}
                    >
                      {label}
                    </button>
                  );
                })}
              </div>

              {/* 标题 */}
              <div>
                <div className="flex items-center justify-between mb-1">
                  <label className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                    标题
                  </label>
                  <span className="text-[10px]" style={{ color: titleLeft < 0 ? '#EF4444' : 'var(--text-tertiary)' }}>
                    {titleLeft}
                  </span>
                </div>
                <input
                  value={draft.title}
                  onChange={(e) => setDraft((d) => ({ ...d, title: e.target.value.slice(0, 80) }))}
                  placeholder="一句话概括问题或建议"
                  className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-field)',
                    border: '1px solid var(--border)',
                    color: 'var(--text-primary)',
                  }}
                />
              </div>

              {/* 描述 */}
              <div>
                <div className="flex items-center justify-between mb-1">
                  <label className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                    问题描述
                  </label>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {draft.content.length} / {MAX_CONTENT_CHARS}
                  </span>
                </div>
                <textarea
                  value={draft.content}
                  onChange={(e) =>
                    setDraft((d) => ({
                      ...d,
                      content: e.target.value.slice(0, MAX_CONTENT_CHARS),
                    }))
                  }
                  rows={5}
                  placeholder="如何复现？期望是什么？实际发生了什么？"
                  className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none resize-none leading-relaxed"
                  style={{
                    backgroundColor: 'var(--bg-field)',
                    border: '1px solid var(--border)',
                    color: 'var(--text-primary)',
                  }}
                />
              </div>

              {/* 邮箱 */}
              <div>
                <label className="text-[11px] block mb-1" style={{ color: 'var(--text-secondary)' }}>
                  联系方式（选填）
                </label>
                <input
                  value={draft.email}
                  onChange={(e) => setDraft((d) => ({ ...d, email: e.target.value }))}
                  placeholder={account?.email ? `默认使用账号邮箱 ${account.email}` : '邮箱或其它联系方式'}
                  className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-field)',
                    border: '1px solid var(--border)',
                    color: 'var(--text-primary)',
                  }}
                />
              </div>

              {/* 日志片段 */}
              <div
                className="rounded-lg p-2.5"
                style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
              >
                <label className="block cursor-pointer select-none">
                  <span className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={draft.includeLogs}
                      onChange={(e) => setDraft((d) => ({ ...d, includeLogs: e.target.checked }))}
                      className="shrink-0"
                      style={{ width: 13, height: 13, margin: 0 }}
                    />
                    <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      附加最近的错误日志（帮助定位问题）
                    </span>
                  </span>
                  <span
                    className="block text-[10px] mt-0.5 pl-[21px]"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    仅包含后端错误/警告，已排除环境探测日志并脱敏；提交前可逐条查看。
                  </span>
                </label>

                {draft.includeLogs && (
                  <div className="mt-2 pl-[21px]">
                    <div className="flex items-center gap-2 mb-1.5">
                      <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        {logsLoading
                          ? '读取中…'
                          : `已选 ${selectedCount} / ${MAX_LOG_LINES} 条，共 ${logs.length} 条`}
                      </span>
                      {focusTs > 0 && (
                        <button
                          onClick={() => setFocusTs(0)}
                          className="text-[10px] transition-colors hover:opacity-80"
                          style={{ color: 'var(--accent)' }}
                          title="当前只显示通知时间前后 3 分钟的日志"
                        >
                          已聚焦通知时间 · 查看全部时间
                        </button>
                      )}
                      <div className="flex-1" />
                      {coverageStart > 0 && (
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          可见范围自 {formatTime(coverageStart)}
                        </span>
                      )}
                    </div>

                    {logsError ? (
                      <div className="text-[10px]" style={{ color: '#EF4444' }}>
                        读取日志失败：{logsError}
                      </div>
                    ) : logs.length === 0 && !logsLoading ? (
                      <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        {focusTs > 0 ? '通知时间附近没有错误日志，可查看全部时间' : '最近没有错误或警告日志'}
                      </div>
                    ) : (
                      <div
                        className="max-h-40 overflow-y-auto pd-scroll-stable rounded"
                        style={{ border: '1px solid var(--border)' }}
                      >
                        {logs.map((e, i) => {
                          const key = `${e.ts}:${i}`;
                          const checked = !!selected[key];
                          return (
                            <label
                              key={key}
                              className="flex items-start gap-2 px-2 py-1 cursor-pointer select-none"
                              style={{
                                borderBottom: i === logs.length - 1 ? 'none' : '1px solid var(--border)',
                                backgroundColor: checked ? 'var(--accent-light)' : 'transparent',
                              }}
                            >
                              <input
                                type="checkbox"
                                checked={checked}
                                onChange={() => toggleLog(key)}
                                disabled={!checked && selectedCount >= MAX_LOG_LINES}
                                className="shrink-0"
                                style={{ width: 13, height: 13, marginTop: 1 }}
                              />
                              <span className="flex-1 min-w-0">
                                <span className="flex items-center gap-1.5">
                                  <span
                                    className="text-[9px] px-1 rounded shrink-0"
                                    style={{
                                      color: e.level === 'error' ? '#EF4444' : '#F59E0B',
                                      backgroundColor:
                                        e.level === 'error' ? 'rgba(239,68,68,0.12)' : 'rgba(245,158,11,0.12)',
                                    }}
                                  >
                                    {e.level}
                                  </span>
                                  <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                                    {formatTime(e.ts)}
                                  </span>
                                  <span
                                    className="text-[10px] truncate"
                                    style={{ color: 'var(--text-tertiary)' }}
                                    title={e.target}
                                  >
                                    {shortTarget(e.target)}
                                  </span>
                                </span>
                                <span
                                  className="block text-[10px] mt-0.5"
                                  style={{
                                    color: 'var(--text-secondary)',
                                    whiteSpace: 'pre-wrap',
                                    wordBreak: 'break-word',
                                  }}
                                >
                                  {e.message}
                                </span>
                              </span>
                            </label>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}
              </div>

              {/* 环境信息预览：把实际会随反馈发送的内容摊开给用户看 */}
              <div
                className="rounded-lg p-2.5"
                style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
              >
                <label className="block cursor-pointer select-none">
                  <span className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={draft.includeEnv}
                      onChange={(e) => setDraft((d) => ({ ...d, includeEnv: e.target.checked }))}
                      className="shrink-0"
                      style={{ width: 13, height: 13, margin: 0 }}
                    />
                    <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      环境信息（帮助判断是否环境问题）
                    </span>
                  </span>
                  <span
                    className="block text-[10px] mt-0.5 pl-[21px]"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    仅含版本与状态，不含启动参数、连接配置与密钥；可逐项选择是否提供。
                  </span>
                </label>

                {draft.includeEnv && (
                  <div className="mt-2 pl-[21px] flex flex-col gap-1">
                    {envLoading ? (
                      <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        读取中…
                      </span>
                    ) : envError ? (
                      <span className="text-[10px]" style={{ color: '#EF4444' }}>
                        读取失败（提交时仍会附带已勾选项）：{envError}
                      </span>
                    ) : null}

                    <EnvOption
                      label="应用类型与版本"
                      checked={draft.envItems.app}
                      onToggle={(v) => setDraft((d) => ({ ...d, envItems: { ...d.envItems, app: v } }))}
                      value={
                        env
                          ? `${APP_TYPE_LABELS[env.appType] ?? env.appType} · v${env.appVersion} · ${
                              CHANNEL_LABELS[env.channel] ?? env.channel
                            }`
                          : undefined
                      }
                    />
                    <EnvOption
                      label="系统与架构"
                      checked={draft.envItems.system}
                      onToggle={(v) =>
                        setDraft((d) => ({ ...d, envItems: { ...d.envItems, system: v } }))
                      }
                      value={env ? `${OS_LABELS[env.os] ?? env.os} ${env.arch}` : undefined}
                    />
                    <EnvOption
                      label="依赖工具版本"
                      checked={draft.envItems.tools}
                      onToggle={(v) => setDraft((d) => ({ ...d, envItems: { ...d.envItems, tools: v } }))}
                      value={
                        env
                          ? [
                              env.nodeVersion && `Node ${env.nodeVersion}`,
                              env.npmVersion && `npm ${env.npmVersion}`,
                              env.gitVersion && `Git ${env.gitVersion}`,
                              env.pythonVersion && `Python ${env.pythonVersion}`,
                            ]
                              .filter(Boolean)
                              .join(' · ') || '未检测到 Node / npm / Git / Python'
                          : undefined
                      }
                    />
                    <EnvOption
                      label={`已启用 Agent${env ? `（${env.agents.length}）` : ''}`}
                      checked={draft.envItems.agents}
                      onToggle={(v) => setDraft((d) => ({ ...d, envItems: { ...d.envItems, agents: v } }))}
                      value={
                        env
                          ? env.agents.length === 0
                            ? '未启用任何 Agent'
                            : env.agents
                                .map((a) => {
                                  const ver = a.version ? ` ${a.version}` : '';
                                  const state =
                                    a.available === true
                                      ? '可用'
                                      : a.installed === false
                                        ? '未安装'
                                        : a.available === false
                                          ? '不可用'
                                          : '';
                                  return `${a.displayName}（${a.transport}）${ver}${state ? ` · ${state}` : ''}`;
                                })
                                .join('；')
                          : undefined
                      }
                    />
                  </div>
                )}
              </div>
            </div>

            {/* 底部动作 */}
            <div
              className="flex items-center justify-end gap-2 px-5 py-3 shrink-0"
              style={{ borderTop: '1px solid var(--border)' }}
            >
              <button
                onClick={() => {
                  if (!submitting) close();
                }}
                disabled={submitting}
                className="pd-btn px-3 py-1.5 text-xs rounded transition-colors disabled:opacity-50"
                style={{
                  border: '1px solid var(--border)',
                  background: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                }}
              >
                取消
              </button>
              <button
                onClick={() => void doSubmit()}
                disabled={submitting}
                className="pd-btn flex items-center gap-1.5 px-3 py-1.5 text-xs rounded transition-colors disabled:opacity-60"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                {submitting && <Loader2 size={11} className="animate-spin" />}
                {submitting ? '提交中…' : '提交反馈'}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
