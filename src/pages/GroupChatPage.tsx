import { useEffect, useMemo, useRef, useCallback, useState, memo, type CSSProperties } from 'react';
import {
  Users, Plus, Bot, User, Sparkles, Send, Pause, Play, Square,
  Radio, Paperclip, Scale, ListTodo, CheckCircle, XCircle, AlertCircle, Download, Copy, Trash2, Folder,
  ImagePlus, FileText, X, Clock, Flag, Save, Undo2, Loader2, History, ChevronDown,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { useGroupChatStore } from '../stores/groupChatStore';
import { useApiProviderStore } from '../stores/apiProviderStore';
import { useAgentRegistry } from '../hooks/useAgentRegistry';
import { showToast } from '../utils/toast';
import { useImagePreviewStore } from '../stores/imagePreviewStore';
import { MarkdownRenderer, collapseBlankLines, tightenListGaps, linkifyUrls } from '../components/message/MarkdownRenderer';
import { RoomUsageBar } from '../components/groupchat/RoomUsageBar';
import { ConfirmationCard, parseConfirmation } from '../components/confirmation/ConfirmationCard';
import { ThinkingChain } from '../components/message/ThinkingChain';
import { SecurityModeSelector, type SecurityModeValue } from '../components/security/SecurityModeSelector';
import type { ThinkingChainStep } from '../components/layout/MainPanel';
import type {
  GroupChatParticipant,
  GroupChatParticipantInput,
  GroupChatMessage,
  CreateGroupChatRoomInput,
  GroupChatToolCall,
  GroupChatConfirmationRequest,
  GroupChatConfirmationResponseInput,
  RoomStatus,
} from '../types/groupchat';
import type { Attachment } from '../types';

const STATUS_LABEL: Record<RoomStatus, string> = {
  idle: '待开始',
  running: '讨论中',
  paused: '已暂停',
  finished: '已结束',
  aborted: '已中止',
};

const STATUS_COLOR: Record<RoomStatus, string> = {
  idle: 'var(--text-tertiary)',
  running: 'var(--status-success)',
  paused: 'var(--status-warning, #f59e0b)',
  finished: 'var(--text-tertiary)',
  aborted: 'var(--status-danger)',
};

const TASK_STATUS_LABEL: Record<string, string> = {
  discussing: '讨论中',
  pending: '待执行',
  running: '执行中',
  success: '完成',
  failed: '失败',
  skipped: '跳过',
};

const FALLBACK_COLORS = ['#3b82f6', '#f59e0b', '#ef4444', '#10b981', '#8b5cf6', '#ec4899'];

function genId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

function parseAgentConfig(config: string): Record<string, string> {
  try {
    const v = JSON.parse(config);
    return typeof v === 'object' && v ? v : {};
  } catch {
    return {};
  }
}

function formatTime(sec: number): string {
  if (!sec) return '';
  return new Date(sec * 1000).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
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

/** 消息落库时 attachments 为 JSON 字符串，解析为附件数组供渲染。 */
function parseAttachments(raw: string | undefined | null): Attachment[] {
  if (!raw) return [];
  try {
    const v = JSON.parse(raw);
    return Array.isArray(v) ? (v as Attachment[]) : [];
  } catch {
    return [];
  }
}

/** 解析 confirmation_response 的 extra 结构化回复（{responses:[{itemId,value}]}）；无则 null。 */
function parseResponseExtra(raw: string | undefined | null): GroupChatConfirmationResponseInput[] | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (Array.isArray(v?.responses)) return v.responses as GroupChatConfirmationResponseInput[];
  } catch {
    // ignore
  }
  return null;
}

/** 解析持久化 toolCalls JSON 为聚合展示步骤（与 MessageBubble/会话模式格式对齐）。 */
function parseToolCalls(raw: string | undefined | null, mentionMap: ReadonlyMap<string, string>): ThinkingChainStep[] {
  if (!raw) return [];
  try {
    const chain = JSON.parse(raw);
    if (!Array.isArray(chain)) return [];
    const steps: ThinkingChainStep[] = [];
    for (const s of chain) {
      if (s.type === 'reasoning') {
        steps.push({ id: `r-${s.id || steps.length}`, type: 'reasoning', content: s.content ?? '', timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'tool_start') {
        steps.push({ id: `t-${s.id || steps.length}`, type: 'tool_start', toolName: s.toolName, toolArgs: typeof s.args === 'string' ? renderMentions(s.args, mentionMap) : s.args, timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'tool_result') {
        steps.push({ id: `tr-${s.id || steps.length}`, type: 'tool_result', toolName: s.toolName, toolResult: typeof s.result === 'string' ? renderMentions(s.result, mentionMap) : s.result, toolSuccess: s.success !== false, timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'file_diff') {
        steps.push({ id: `f-${s.id || steps.length}`, type: 'file_diff', filePath: s.filePath, fileDiff: s.fileDiff, timestamp: s.ts || Date.now() / 1000 });
      }
    }
    return steps;
  } catch {
    return [];
  }
}

/** 实时推理文本（agent-reasoning 事件）转聚合展示步骤（仅实时，不落库）。
 *  id 需稳定（基于 speaker），否则每次渲染都生成新 id 会导致 ThinkingChain 整体重挂载，
 *  流式期间产生大量 DOM 重建，拖慢实时显示。 */
function reasoningToSteps(
  speaker: string | undefined,
  text: string | undefined | null,
  mentionMap: ReadonlyMap<string, string>
): ThinkingChainStep[] {
  if (!text) return [];
  // 推理内容中的 [@id] / @id 与消息正文一样需做显示名映射（工具调用 args/result 已处理，推理同样不能漏）
  return [{ id: `reasoning-${speaker}`, type: 'reasoning', content: renderMentions(text, mentionMap), timestamp: Date.now() / 1000 }];
}

/** 实时工具调用（agent-tool-* 事件）转聚合展示步骤。 */
function realtimeToolCallsToSteps(calls: GroupChatToolCall[] | undefined, mentionMap: ReadonlyMap<string, string>): ThinkingChainStep[] {
  if (!calls || calls.length === 0) return [];
  const steps: ThinkingChainStep[] = [];
  for (const c of calls) {
    steps.push({
      id: `${c.toolId}-start`,
      type: 'tool_start',
      toolName: c.toolName,
      toolArgs: typeof c.arguments === 'string' ? renderMentions(c.arguments, mentionMap) : c.arguments,
      timestamp: Date.now() / 1000,
    });
    if (c.status === 'done') {
      steps.push({
        id: `${c.toolId}-result`,
        type: 'tool_result',
        toolName: c.toolName,
        toolResult: typeof c.result === 'string' ? renderMentions(c.result, mentionMap) : c.result,
        toolSuccess: c.success,
        timestamp: Date.now() / 1000,
      });
    }
  }
  return steps;
}

type IconName = 'director' | 'user' | 'agent';

function participantIcon(type: string): IconName {
  if (type === 'user') return 'user';
  if (type === 'director') return 'director';
  return 'agent';
}

function ParticipantAvatar({ icon, color, small }: { icon: IconName; color: string; small?: boolean }) {
  const Icon = icon === 'user' ? User : icon === 'director' ? Sparkles : Bot;
  const size = small ? 10 : 13;
  return (
    <div
      className={`${small ? 'w-5 h-5' : 'w-7 h-7'} rounded-full shrink-0 flex items-center justify-center`}
      style={{ backgroundColor: `${color}22`, color }}
    >
      <Icon size={size} />
    </div>
  );
}

interface DraftParticipant {
  type: 'api' | 'cli';
  displayName: string;
  systemRole: string;
  provider: string;
  model: string;
  agentType: string;
}

function CreateRoomModal({ onClose }: { onClose: () => void }) {
  const createRoom = useGroupChatStore((s) => s.createRoom);
  const loadRooms = useGroupChatStore((s) => s.loadRooms);
  const selectRoom = useGroupChatStore((s) => s.selectRoom);
  const { providers, fetchProviders } = useApiProviderStore();
  const { agents } = useAgentRegistry();

  const [title, setTitle] = useState('');
  const [topic, setTopic] = useState('');
  const [directorProvider, setDirectorProvider] = useState('');
  const [directorModel, setDirectorModel] = useState('');
  const [drafts, setDrafts] = useState<DraftParticipant[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 允许主持人在自动补人时添加 CLI 参与者（默认开启；按任务差异化限制，例如某些任务不希望自动拉起本地 CLI 进程）。
  const [allowAutoCli, setAllowAutoCli] = useState(true);
  // 房间统一产物目录（绝对路径；空=运行时回退 <工作目录>/outputs/<房间标题>/）。
  const [outputDir, setOutputDir] = useState('');

  useEffect(() => {
    fetchProviders();
  }, [fetchProviders]);

  const cliAgents = useMemo(
    () => agents.filter((a) => a.agentType !== 'api' && a.isEnabled),
    [agents],
  );

  const modelsOf = (providerId: string) =>
    providers.find((p) => p.id === providerId)?.models ?? [];

  function addDraft(type: 'api' | 'cli') {
    // 新增参与者插入列表开头，避免用户滚动到底部去填写。
    setDrafts((d) => [
      { type, displayName: '', systemRole: '', provider: '', model: '', agentType: '' },
      ...d,
    ]);
  }

  function updateDraft(idx: number, patch: Partial<DraftParticipant>) {
    setDrafts((d) => d.map((x, i) => (i === idx ? { ...x, ...patch } : x)));
  }

  function removeDraft(idx: number) {
    setDrafts((d) => d.filter((_, i) => i !== idx));
  }

  async function submit() {
    setError(null);
    if (!title.trim()) {
      setError('请填写房间标题');
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
          id: genId('api'),
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
          id: genId('cli'),
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
      allowAutoCli: allowAutoCli ? 1 : 0,
      outputDir: outputDir.trim() || undefined,
    };

    try {
      setSubmitting(true);
      const room = await createRoom(input);
      await loadRooms();
      await selectRoom(room.id);
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.4)' }}
      onClick={onClose}
    >
      <div
        className="w-[560px] max-h-[80vh] rounded-xl flex flex-col overflow-hidden"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 pt-4 pb-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>新建群聊房间</span>
          <button className="pd-btn pd-btn-sm" onClick={onClose} style={{ color: 'var(--text-secondary)' }}>
            <XCircle size={14} />
          </button>
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto px-5 py-4 flex flex-col gap-4">
        <label className="flex flex-col gap-1 text-xs" style={{ color: 'var(--text-secondary)' }}>
          房间标题
          <input
            className="px-3 py-2 rounded-lg text-xs outline-none"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="例如：PilotDesk 架构评审"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs" style={{ color: 'var(--text-secondary)' }}>
          讨论议题（可留空）
          <textarea
            className="px-3 py-2 rounded-lg text-xs outline-none resize-none"
            rows={2}
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={topic}
            onChange={(e) => setTopic(e.target.value)}
            placeholder="留空时以进入房间后的第一条指令作为目标"
          />
        </label>

        <div className="flex flex-col gap-1 text-xs" style={{ color: 'var(--text-secondary)' }}>
          Director 模型（协调/裁决）
          <div className="flex gap-2">
            <select
              className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              value={directorProvider}
              onChange={(e) => { setDirectorProvider(e.target.value); setDirectorModel(''); }}
            >
              <option value="">选择提供商</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
            <select
              className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              value={directorModel}
              onChange={(e) => setDirectorModel(e.target.value)}
              disabled={!directorProvider}
            >
              <option value="">选择模型</option>
              {modelsOf(directorProvider).map((m) => (
                <option key={m} value={m}>{m}</option>
              ))}
            </select>
          </div>
        </div>

        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>参与者（可添加 API / CLI Agent）</span>
              <label
                className="flex items-center gap-1 cursor-pointer"
                title="关闭后，主持人在自动补人时不会添加 CLI Agent 参与者（CLI 手动添加不受影响）"
              >
                <input
                  type="checkbox"
                  checked={allowAutoCli}
                  onChange={(e) => setAllowAutoCli(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>可自动补充CLI Agent</span>
              </label>
            </div>
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
              key={i}
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
                  <select
                    className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    value={d.provider}
                    onChange={(e) => updateDraft(i, { provider: e.target.value, model: '' })}
                  >
                    <option value="">选择提供商</option>
                    {providers.map((p) => (
                      <option key={p.id} value={p.id}>{p.name}</option>
                    ))}
                  </select>
                  <select
                    className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                    value={d.model}
                    onChange={(e) => updateDraft(i, { model: e.target.value })}
                    disabled={!d.provider}
                  >
                    <option value="">选择模型</option>
                    {modelsOf(d.provider).map((m) => (
                      <option key={m} value={m}>{m}</option>
                    ))}
                  </select>
                </div>
              ) : (
                <select
                  className="px-3 py-2 rounded-lg text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  value={d.agentType}
                  onChange={(e) => updateDraft(i, { agentType: e.target.value })}
                >
                  <option value="">选择 CLI Agent 类型</option>
                  {cliAgents.map((a) => (
                    <option key={a.agentType} value={a.agentType}>{a.displayName || a.agentType}</option>
                  ))}
                </select>
              )}
            </div>
          ))}
        </div>

        {error && (
          <div className="px-3 py-2 rounded-lg text-[11px]" style={{ color: 'var(--status-danger)', backgroundColor: 'var(--status-danger-bg, rgba(239,68,68,0.1))' }}>
            {error}
          </div>
        )}
        </div>

        <div className="flex items-center gap-2 px-5 py-4 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
          <span className="text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>产物目录：</span>
          <input
            type="text"
            value={outputDir}
            onChange={(e) => setOutputDir(e.target.value)}
            placeholder="留空=默认 <工作目录>/outputs/<主题目录名>/"
            className="w-62 rounded-md px-2 py-1 text-[11px] outline-none"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)', color: 'var(--text-primary)' }}
          />
          <button
            type="button"
            className="px-2 py-1 rounded-md text-[11px] shrink-0"
            style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
            onClick={async () => {
              const sel = await openDialog({ directory: true, multiple: false, title: '选择产物目录' });
              if (typeof sel === 'string') setOutputDir(sel);
            }}
          >
            选择
          </button>
          <div className="flex-1" />
          <button className="pd-btn pd-btn-sm" onClick={onClose} style={{ color: 'var(--text-secondary)' }}>取消</button>
          <button className="pd-btn pd-btn-sm pd-btn-primary" onClick={submit} disabled={submitting}>
            {submitting ? '创建中…' : '创建并进入'}
          </button>
        </div>
      </div>
    </div>
  );
}

function AddParticipantModal({ onClose }: { onClose: () => void }) {
  const addParticipant = useGroupChatStore((s) => s.addParticipant);
  const { providers, fetchProviders } = useApiProviderStore();
  const { agents } = useAgentRegistry();

  const [type, setType] = useState<'api' | 'cli'>('api');
  const [displayName, setDisplayName] = useState('');
  const [systemRole, setSystemRole] = useState('');
  const [provider, setProvider] = useState('');
  const [model, setModel] = useState('');
  const [agentType, setAgentType] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchProviders();
  }, [fetchProviders]);

  const cliAgents = useMemo(
    () => agents.filter((a) => a.agentType !== 'api' && a.isEnabled),
    [agents],
  );
  const modelsOf = (providerId: string) =>
    providers.find((p) => p.id === providerId)?.models ?? [];

  async function submit() {
    setError(null);
    if (!displayName.trim()) {
      setError('请填写显示名');
      return;
    }
    let agentConfig = '{}';
    if (type === 'api') {
      if (!provider || !model) {
        setError('请选择提供商与模型');
        return;
      }
      agentConfig = JSON.stringify({ provider, model });
    } else {
      if (!agentType) {
        setError('请选择 CLI Agent 类型');
        return;
      }
      agentConfig = JSON.stringify({ agent_type: agentType });
    }
    try {
      setSubmitting(true);
      await addParticipant({
        id: genId(type),
        participantType: type,
        agentConfig,
        displayName: displayName.trim(),
        systemRole: systemRole.trim() || '参与者',
      });
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.4)' }}
      onClick={onClose}
    >
      <div
        className="w-[420px] max-h-[80vh] overflow-y-auto rounded-xl p-5 flex flex-col gap-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>添加参与者</span>
          <button className="pd-btn pd-btn-sm" onClick={onClose} style={{ color: 'var(--text-secondary)' }}>
            <XCircle size={14} />
          </button>
        </div>

        <div className="flex gap-2">
          <button
            className="flex-1 px-3 py-2 rounded-lg text-xs"
            style={{
              backgroundColor: type === 'api' ? 'var(--accent-light)' : 'var(--bg-tertiary)',
              color: type === 'api' ? 'var(--accent)' : 'var(--text-secondary)',
              border: `1px solid ${type === 'api' ? 'var(--accent)' : 'var(--border)'}`,
            }}
            onClick={() => setType('api')}
          >
            API Agent
          </button>
          <button
            className="flex-1 px-3 py-2 rounded-lg text-xs"
            style={{
              backgroundColor: type === 'cli' ? 'var(--accent-light)' : 'var(--bg-tertiary)',
              color: type === 'cli' ? 'var(--accent)' : 'var(--text-secondary)',
              border: `1px solid ${type === 'cli' ? 'var(--accent)' : 'var(--border)'}`,
            }}
            onClick={() => setType('cli')}
          >
            CLI Agent
          </button>
        </div>

        <label className="flex flex-col gap-1 text-xs" style={{ color: 'var(--text-secondary)' }}>
          显示名
          <input
            className="px-3 py-2 rounded-lg text-xs outline-none"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            placeholder="如 Hermes Agent"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs" style={{ color: 'var(--text-secondary)' }}>
          角色
          <input
            className="px-3 py-2 rounded-lg text-xs outline-none"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={systemRole}
            onChange={(e) => setSystemRole(e.target.value)}
            placeholder="如 架构师 / 质疑者 / 执行者"
          />
        </label>

        {type === 'api' ? (
          <div className="flex gap-2">
            <select
              className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              value={provider}
              onChange={(e) => { setProvider(e.target.value); setModel(''); }}
            >
              <option value="">选择提供商</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
            <select
              className="flex-1 px-3 py-2 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              value={model}
              onChange={(e) => setModel(e.target.value)}
              disabled={!provider}
            >
              <option value="">选择模型</option>
              {modelsOf(provider).map((m) => (
                <option key={m} value={m}>{m}</option>
              ))}
            </select>
          </div>
        ) : (
          <select
            className="px-3 py-2 rounded-lg text-xs outline-none"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={agentType}
            onChange={(e) => setAgentType(e.target.value)}
          >
            <option value="">选择 CLI Agent 类型</option>
            {cliAgents.map((a) => (
              <option key={a.agentType} value={a.agentType}>{a.displayName || a.agentType}</option>
            ))}
          </select>
        )}

        {error && (
          <div className="px-3 py-2 rounded-lg text-[11px]" style={{ color: 'var(--status-danger)', backgroundColor: 'var(--status-danger-bg, rgba(239,68,68,0.1))' }}>
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2">
          <button className="pd-btn pd-btn-sm" onClick={onClose} style={{ color: 'var(--text-secondary)' }}>取消</button>
          <button className="pd-btn pd-btn-sm pd-btn-primary" onClick={submit} disabled={submitting}>
            {submitting ? '添加中…' : '添加'}
          </button>
        </div>
      </div>
    </div>
  );
}

const CONCLUSION_SECTIONS: { title: string; icon: LucideIcon; color: string }[] = [
  { title: '已完成', icon: CheckCircle, color: '#10B981' },
  { title: '未完成', icon: Clock, color: '#F59E0B' },
  { title: '失败', icon: XCircle, color: '#EF4444' },
  { title: '最终结论', icon: Flag, color: '#8B5CF6' },
];

/**
 * 把参与者引用替换为 `@显示名`（仅视觉替换，严格精确匹配）。
 * - 支持 `[@id]` 与 `@id` 两种引用格式，替换结果上限均为 `@显示名`；
 * - 裸 id（无 @ 前缀）不做替换，避免误伤正文中与 id 相同的普通字符串（文件内容、代码等）；
 * - 匹配不到（参与者已移出）或显示名与 id 相同时保留原样。
 * 后端消息一律以唯一 id 落库，显示名只在本层做展示映射，避免同名/删减导致的指代歧义。
 */
export function renderMentions(text: string, mentionMap: ReadonlyMap<string, string>): string {
  if (!text || mentionMap.size === 0) return text;
  const re = /\[@([^\]]+)\]|@([\w-]+)/g;
  return text.replace(re, (m, bracketed, bare) => {
    const id = bracketed ?? bare;
    const name = mentionMap.get(id);
    if (!name || name === id) return m;
    return `@${name}`;
  });
}

function parseConclusionSections(content: string): { title: string; body: string }[] {
  const headingRe = /^##\s*(已完成|未完成|失败|最终结论)\s*[:：]?\s*$/;
  const lines = content.split('\n');
  const sections: { title: string; body: string }[] = [];
  let current: { title: string; lines: string[] } | null = null;
  for (const line of lines) {
    const m = line.match(headingRe);
    if (m) {
      if (current) sections.push({ title: current.title, body: current.lines.join('\n').trim() });
      current = { title: m[1], lines: [] };
    } else if (current) {
      current.lines.push(line);
    }
  }
  if (current) sections.push({ title: current.title, body: current.lines.join('\n').trim() });
  return sections;
}

// 结论气泡差异化背景（accent-light）适配：MarkdownRenderer 内代码块/内联 code/引用/表格等
// 元素的背景使用 --bg-tertiary、边框使用 --border 变量；在结论容器作用域内覆盖为与浅紫底协调的值，
// 使元素样式（如代码块背景）随结论气泡配色自动适配。
const CONCLUSION_BG_OVERRIDE = {
  '--bg-tertiary': 'rgba(255, 255, 255, 0.55)',
  '--border': 'rgba(139, 92, 246, 0.30)',
} as unknown as CSSProperties;

function ConclusionContent({ content }: { content: string }) {
  const sections = parseConclusionSections(content);
  if (sections.length === 0) {
    return (
      <div style={CONCLUSION_BG_OVERRIDE}>
        <MarkdownRenderer content={content} />
      </div>
    );
  }
  return (
    <div className="flex flex-col" style={CONCLUSION_BG_OVERRIDE}>
      {sections.map((sec, i) => {
        const cfg = CONCLUSION_SECTIONS.find((s) => s.title === sec.title);
        const Icon = cfg?.icon ?? CheckCircle;
        const color = cfg?.color ?? 'var(--text-primary)';
        return (
          <div key={sec.title}>
            {i > 0 && <hr style={{ border: 'none', borderTop: '1px solid var(--border)', margin: '6px 0' }} />}
            <div className="flex items-center gap-1.5 mb-1">
              <Icon size={14} style={{ color, flexShrink: 0 }} />
              <span className="font-semibold" style={{ color, fontSize: '13px' }}>{sec.title}</span>
            </div>
            <MarkdownRenderer content={sec.body} />
          </div>
        );
      })}
    </div>
  );
}

interface GroupChatMessageItemProps {
  message: GroupChatMessage;
  participant?: GroupChatParticipant;
  color: string;
  isUser: boolean;
  displayName: string;
  /** 参与者 id → 显示名（渲染层提及视觉替换用）。 */
  mentionMap: ReadonlyMap<string, string>;
  /** 该确认请求是否已收到用户回复（数据驱动，可跨会话恢复）。 */
  responded: boolean;
  /** 已提交时的结构化回复（confirmation_response 的 extra.responses）：控件级还原勾选/输入/选择。 */
  submittedResponses?: GroupChatConfirmationResponseInput[];
  onRespond: (requestId: string, responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
}

/** 群聊默认页介绍：未选中会话实例时展示功能能力与使用方法。 */
function GroupChatIntro() {
  return (
    <div className="h-full flex items-center justify-center overflow-y-auto px-6 py-8">
      <div className="max-w-3xl w-full flex flex-col gap-5">
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2 text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            <Users size={15} style={{ color: 'var(--accent)' }} />
            多 Agent 群聊协作
          </div>
          <div className="text-[11px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
            由主持人（Director）统一调度多名 AI 参与者与本地 CLI Agent 组成的群聊，围绕目标分工协作、讨论决策、产出文件。适合方案对比、头脑风暴、多角色内容生产等场景。
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>快速开始</div>
          <ol className="text-[11px] leading-relaxed flex flex-col gap-1" style={{ color: 'var(--text-secondary)' }}>
            <li>1. 点击左上角「新建」，填写房间标题与目标（可直接粘贴需求），选择主持人模型与参与者（可留空由主持人自动组建）；</li>
            <li>2. 主持人理解目标后自动编排任务与议程，有歧义时会向你发起确认（确认项可多选、可选填补充内容）；</li>
            <li>3. 讨论进行中可直接发送消息插话/调整方向，主持人/其他参与者的推理与工具调用可在「实时活动」与思维链中查看；</li>
            <li>4. 产物统一写入右侧面板指定的产物目录；任务结束后可导出为工作流复用。</li>
          </ol>
        </div>

        <div className="flex flex-col gap-1.5">
          <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>要点提示</div>
          <ul className="text-[11px] leading-relaxed flex flex-col gap-1" style={{ color: 'var(--text-secondary)' }}>
            <li>· 发言需先声明「立场：支持 / 反对 / 中立」；提及参与者一律使用 [@id]；</li>
            <li>· 需要调整方向时，直接输入消息即可被主持人重新编排；手动「结束」后发送新消息可重启该会话；</li>
            <li>· 参与者包括云端模型与本地 CLI Agent（CLI Agent 使用自身能力执行，不占用内置工具）。</li>
          </ul>
        </div>

        {/* 模式选择醒目提示：帮助用户在进入群聊前按目标特性选择合适的执行模式（纯文案，无交互）。 */}
        <div
          className="rounded-lg px-3 py-2.5"
          style={{ backgroundColor: 'var(--accent-light)', border: '1px solid var(--accent)' }}
        >
          <div className="flex items-center gap-1.5 mb-1">
            <Sparkles size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />
            <span className="text-xs font-medium" style={{ color: 'var(--accent)' }}>
              如何选择：普通会话 还是 群聊协作？
            </span>
          </div>
          <ul className="text-[11px] leading-relaxed flex flex-col gap-1" style={{ color: 'var(--text-secondary)' }}>
            <li>
              · 简单明确的目标（修 bug、写函数、生成文档、查资料等）建议直接用<b>普通会话</b>：更快、更省；
            </li>
            <li>
              · 需要多方视角 / 评审 / 复杂澄清的目标（方案取舍、设计评审、多角色复盘等）适合用<b>群聊</b>：角色独立、立场可见、由主持人协调收敛；
            </li>
            <li>· 群聊通常比普通会话更慢、token 消耗更高，请按目标特性自行选择模式。</li>
          </ul>
        </div>
      </div>
    </div>
  );
}

const GroupChatMessageItem = memo(function GroupChatMessageItem({
  message,
  participant,
  color,
  isUser,
  displayName,
  mentionMap,
  responded,
  submittedResponses,
  onRespond,
}: GroupChatMessageItemProps) {
  const atts = parseAttachments(message.attachments);
  const confirmation = message.kind === 'confirmation_request' ? parseConfirmation(message.extra) : null;
  const isConclusion = message.kind === 'conclusion';
  // 正文提及的视觉替换（参与者 id → @显示名）；用户消息保留原样（其内容由本人输入，已是显示名）。
  const renderedContent = renderMentions(message.content, mentionMap);
  const renderedConfirmation = confirmation
    ? { ...confirmation, prompt: renderMentions(confirmation.prompt, mentionMap) }
    : null;
  return (
    <div className="flex justify-start px-4 py-1.5">
      <div className="flex w-full items-start gap-2">
        <ParticipantAvatar icon={participantIcon(participant?.participantType ?? 'agent')} color={color} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-0.5">
            <span className="text-[10px] font-medium truncate" style={{ color }}>{displayName}</span>
            <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
              {formatTime(message.timestamp)} · 第{message.round}轮
            </span>
          </div>
          <div
            className={`block w-full rounded-xl text-xs leading-relaxed break-all overflow-hidden${isUser ? ' message-bubble-user' : ''}`}
            style={{
              backgroundColor: isConclusion ? 'var(--accent-light)' : isUser ? 'var(--accent)' : 'var(--bg-secondary)',
              color: isUser ? '#fff' : 'var(--text-primary)',
              border: `1px solid ${isConclusion ? 'var(--accent)' : isUser ? 'transparent' : 'var(--border)'}`,
            }}
          >
            {isConclusion && (
              <div className="flex items-center gap-1.5 px-3 py-2.5" style={{ backgroundColor: 'var(--accent)' }}>
                <Flag size={13} style={{ color: '#fff' }} />
                <span className="font-semibold" style={{ color: '#fff', fontSize: '12px' }}>最终结论</span>
              </div>
            )}
            <div className="px-3 py-2">
            {atts.length > 0 && (
              <div className="flex flex-wrap gap-1.5 mb-1.5">
                {atts.map((att, ai) =>
                  att.kind === 'image' ? (
                    <button
                      key={`${ai}-${att.path}`}
                      onClick={() => useImagePreviewStore.getState().open(convertFileSrc(att.path))}
                      className="block w-16 h-16 rounded-lg overflow-hidden shrink-0 cursor-zoom-in p-0"
                      style={{ border: isUser ? '1px solid rgba(255,255,255,0.35)' : '1px solid var(--border)' }}
                      title={att.name}
                    >
                      <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                    </button>
                  ) : (
                    <div
                      key={`${ai}-${att.path}`}
                      className="flex items-center gap-1.5 px-2 py-1 rounded-md text-[11px] shrink-0 max-w-[220px]"
                      style={{
                        backgroundColor: isUser ? 'rgba(255,255,255,0.18)' : 'var(--bg-tertiary)',
                        color: isUser ? '#fff' : 'var(--text-secondary)',
                      }}
                      title={att.path}
                    >
                      <FileText size={12} style={{ flexShrink: 0 }} />
                      <span className="truncate">{att.name}</span>
                    </div>
                  ),
                )}
              </div>
            )}
            {isUser ? (
              <span className="whitespace-pre-wrap break-all">{message.content}</span>
            ) : isConclusion ? (
              <ConclusionContent content={renderedContent} />
            ) : (
              <>
                <ThinkingChain
                  steps={[
                    // 持久化思考链（主持人/参与者一致，无差异展示）
                    ...(message.reasoningContent
                      ? [{ id: `reasoning-${message.id}`, type: 'reasoning' as const, content: renderMentions(message.reasoningContent, mentionMap), timestamp: 0 }]
                      : []),
                    ...parseToolCalls(message.toolCalls, mentionMap),
                  ]}
                />
                {renderedConfirmation ? (
                  // 确认请求：正文 content 与卡片 prompt 同源（后端落库同一字符串），
                  // 只渲染卡片避免同一文本双份展示。
                  <ConfirmationCard
                    confirmation={renderedConfirmation}
                    responded={responded}
                    submittedResponses={submittedResponses}
                    onSubmit={(responses) => onRespond(renderedConfirmation.requestId, responses)}
                    submittedText="已提交，等待主持人继续…"
                    openHint="需要你的决定，请直接在下方的输入框中回复。"
                  />
                ) : (
                  <MarkdownRenderer content={renderedContent} />
                )}
              </>
            )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
});

/** 群聊页面：rightPanelOpen 由外层 MainLayout 的标题栏折叠按钮控制页内右侧面板显隐 */
export function GroupChatPage({ rightPanelOpen = true }: { rightPanelOpen?: boolean }) {
  const {
    rooms, currentRoomId, participants, messages, stances, tasks, streaming, reasoning,
    currentSpeaker, currentRound, toolCalls, error,
    messagesLoading, loadingEarlier, totalMessages, loadEarlierMessages,
    loadRooms, selectRoom, sendMessage, deleteRoom, removeParticipant,
    respondConfirmation, pause, resume, abort, exportWorkflow,
    setRoomOutputDir, actorAlive, roomsActorAlive,
  } = useGroupChatStore();

  const { getTheme } = useAgentRegistry();
  const [input, setInput] = useState('');
  const [showCreate, setShowCreate] = useState(false);
  // 房间统一产物目录编辑草稿（随当前房间同步；保存后写入房间配置并广播）。
  const [outputDirDraft, setOutputDirDraft] = useState('');
  const [showAdd, setShowAdd] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportJson, setExportJson] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [draggingFiles, setDraggingFiles] = useState<{ name: string; kind: 'image' | 'file' }[]>([]);
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionQuery, setMentionQuery] = useState('');
  const attachmentsRef = useRef<Attachment[]>([]);
  // ── 会话安全模式（本波次有效；默认标准，随发送传参，不持久化）──
  const [securityMode, setSecurityMode] = useState<SecurityModeValue>('standard');

  // ── 群聊文件历史（撤销入口）：当前房间 write_file/edit_file 的修改快照 ──
  interface RoomHistoryEntry {
    id: number;
    sessionId: string;
    filePath: string;
    fileExisted: boolean;
    createdAt: number;
  }
  const [roomHistory, setRoomHistory] = useState<RoomHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyUndoingId, setHistoryUndoingId] = useState<number | null>(null);
  /** 右侧面板当前 tab：讨论（参与者/立场/任务）或文件历史。 */
  const [rightTab, setRightTab] = useState<'overview' | 'stances' | 'files'>('overview');
  /** 立场快照条目展开集合（participantId → 展开）。默认折叠只显示一行。 */
  const [expandedStances, setExpandedStances] = useState<Set<string>>(() => new Set());
  /** 任务条目展开集合（taskId → 展开）。默认折叠只显示一行。 */
  const [expandedTasks, setExpandedTasks] = useState<Set<string>>(() => new Set());
  /** 群聊文件历史开关（持久化于 tool_overrides.groupchat）。 */
  const [fileHistoryEnabled, setFileHistoryEnabled] = useState(true);

  const toggleStance = useCallback((participantId: string) => {
    setExpandedStances((prev) => {
      const next = new Set(prev);
      if (next.has(participantId)) next.delete(participantId);
      else next.add(participantId);
      return next;
    });
  }, []);

  const toggleTask = useCallback((taskId: string) => {
    setExpandedTasks((prev) => {
      const next = new Set(prev);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });
  }, []);

  // 读取群聊文件历史开关状态（切房间/首次加载均刷新）
  useEffect(() => {
    invoke<boolean>('groupchat_get_file_history_enabled')
      .then(setFileHistoryEnabled)
      .catch(() => {});
  }, [currentRoomId]);

  const loadRoomHistory = useCallback(async (roomId: string) => {
    if (!roomId) {
      setRoomHistory([]);
      return;
    }
    setHistoryLoading(true);
    try {
      const page = await invoke<{ total: number; items: RoomHistoryEntry[] }>('list_file_history', {
        sessionId: roomId,
        limit: 50,
        offset: 0,
        pathKeyword: null,
      });
      setRoomHistory(page.items ?? []);
    } catch {
      setRoomHistory([]);
    } finally {
      setHistoryLoading(false);
    }
  }, []);

  // 切房间时加载该房间的文件历史
  useEffect(() => {
    loadRoomHistory(currentRoomId ?? '');
  }, [currentRoomId, loadRoomHistory]);

  // 切入「文件历史」tab 时刷新记录（讨论过程中可能持续产生新写入）
  useEffect(() => {
    if (rightTab === 'files') {
      loadRoomHistory(currentRoomId ?? '');
    }
  }, [rightTab, currentRoomId, loadRoomHistory]);

  // 文件历史更新事件：运行中产生新快照时实时刷新徽标数量（防抖 300ms 合并连续写）。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    (async () => {
      try {
        unlisten = await listen<{ roomId: string }>('file-history-updated', (event) => {
          if (event.payload.roomId !== currentRoomId) return;
          if (timer) clearTimeout(timer);
          timer = setTimeout(() => loadRoomHistory(currentRoomId ?? ''), 300);
        });
      } catch (e) {
        console.warn('[GroupChat] 注册文件历史事件监听失败:', e);
      }
    })();
    return () => {
      if (timer) clearTimeout(timer);
      unlisten?.();
    };
  }, [currentRoomId, loadRoomHistory]);

  const undoRoomHistory = useCallback(async (id: number) => {
    setHistoryUndoingId(id);
    try {
      await invoke('undo_file_history', { historyId: id });
      setRoomHistory((prev) => prev.filter((e) => e.id !== id));
    } catch {
      // ignore
    } finally {
      setHistoryUndoingId(null);
    }
  }, []);

  const inputBarRef = useRef<HTMLDivElement>(null);
  const inputAreaRef = useRef<HTMLTextAreaElement>(null);
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  useEffect(() => {
    loadRooms();
  }, [loadRooms]);

  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  const currentRoom = rooms.find((r) => r.id === currentRoomId) ?? null;

  // 同步产物目录编辑草稿到当前房间配置。
  useEffect(() => {
    setOutputDirDraft(currentRoom?.outputDir ?? '');
  }, [currentRoomId, currentRoom?.outputDir]);

  const participantById = useMemo(() => {
    const map = new Map<string, GroupChatParticipant>();
    for (const p of participants) map.set(p.id, p);
    return map;
  }, [participants]);

  // 立场态度聚合统计（attitude 由后端 LLM 预处理给出，失败时后端文本分类兜底）。
  const stanceStats = useMemo(() => {
    const stats = { agree: 0, disagree: 0, neutral: 0 };
    for (const s of stances) {
      if (s.attitude === 'agree') stats.agree++;
      else if (s.attitude === 'disagree') stats.disagree++;
      else stats.neutral++;
    }
    return stats;
  }, [stances]);

  // 执行阶段正在运行任务的参与者：纯工具调用时无 floor_granted，据此点亮呼吸圆点。
  const executingIds = useMemo(() => {
    const set = new Set<string>();
    for (const t of tasks) {
      if (t.status === 'running' && t.assignee) set.add(t.assignee);
    }
    return set;
  }, [tasks]);

  // 可被 @ 的参与者（仅 api/cli Agent，不含 user/director），按输入过滤。
  const mentionCandidates = useMemo(() => {
    if (!mentionOpen) return [];
    const q = mentionQuery.toLowerCase();
    return participants.filter(
      (p) =>
        (p.participantType === 'api' || p.participantType === 'cli') &&
        (p.displayName.toLowerCase().includes(q) || p.id.toLowerCase().includes(q))
    );
  }, [mentionOpen, mentionQuery, participants]);

  const colorOf = useCallback(
    (p: GroupChatParticipant | undefined, idx: number): string => {
      if (!p) return FALLBACK_COLORS[idx % FALLBACK_COLORS.length];
      if (p.participantType === 'director') return '#8b5cf6';
      if (p.participantType === 'user') return '#22c55e';
      if (p.participantType === 'api') return '#10b981';
      const cfg = parseAgentConfig(p.agentConfig);
      const agentType = cfg.agent_type || cfg.agentType;
      if (agentType) {
        const theme = getTheme(agentType);
        if (theme && theme.color) return theme.color;
      }
      return FALLBACK_COLORS[idx % FALLBACK_COLORS.length];
    },
    [getTheme],
  );

  const sortedMessages = useMemo(
    () => [...messages].sort((a, b) => a.seq - b.seq || a.timestamp - b.timestamp),
    [messages],
  );

  // 已收到用户回复的确认请求 id（confirmation_response.replyTo 指向 confirmation_request 消息 id，
  // 经消息 id → extra.requestId 两步关联），用于跨会话恢复「已提交」态。
  const respondedRequestIds = useMemo(() => {
    const reqIdByMsgId = new Map<string, string>();
    for (const m of messages) {
      if (m.kind === 'confirmation_request') {
        const conf = parseConfirmation(m.extra);
        if (conf?.requestId) reqIdByMsgId.set(m.id, conf.requestId);
      }
    }
    const set = new Set<string>();
    for (const m of messages) {
      if (m.kind === 'confirmation_response' && m.replyTo) {
        const rid = reqIdByMsgId.get(m.replyTo);
        if (rid) set.add(rid);
      }
    }
    return set;
  }, [messages]);

  // requestId → 结构化回复（confirmation_response 的 extra.responses），已提交态恢复控件用。
  const replyResponsesByRequest = useMemo(() => {
    const reqIdByMsgId = new Map<string, string>();
    for (const m of messages) {
      if (m.kind === 'confirmation_request') {
        const conf = parseConfirmation(m.extra);
        if (conf?.requestId) reqIdByMsgId.set(m.id, conf.requestId);
      }
    }
    const map = new Map<string, GroupChatConfirmationResponseInput[]>();
    for (const m of messages) {
      if (m.kind === 'confirmation_response' && m.replyTo) {
        const rid = reqIdByMsgId.get(m.replyTo);
        const responses = parseResponseExtra(m.extra);
        if (rid && responses) map.set(rid, responses);
      }
    }
    return map;
  }, [messages]);

  const displayNameOf = useCallback(
    (id: string) => participantById.get(id)?.displayName ?? id,
    [participantById],
  );

  // 参与者 id → 显示名映射，用于消息正文/工具调用/立场/子任务等提及的视觉替换。
  const mentionMap = useMemo(
    () => new Map(participants.map((p) => [p.id, p.displayName])),
    [participants],
  );

  // 消息窗口顶部栏阶段徽章：由房间状态 + 当前发言者输出状态推导当前阶段。
  const phaseInfo = useMemo(() => {
    const status = currentRoom?.status;
    if (!currentRoom || status === 'idle') return { label: '待开始', color: 'var(--text-tertiary)' };
    if (status === 'finished') return { label: '已结束', color: 'var(--text-tertiary)' };
    if (status === 'paused') return { label: '已暂停', color: 'var(--text-secondary)' };
    if (currentSpeaker) {
      const who = displayNameOf(currentSpeaker);
      const hasText = !!streaming[currentSpeaker];
      const hasSteps =
        reasoningToSteps(currentSpeaker, reasoning[currentSpeaker], mentionMap).length > 0 ||
        realtimeToolCallsToSteps(toolCalls[currentSpeaker], mentionMap).length > 0;
      if (hasText) return { label: `${who} 发言中`, color: 'var(--accent)' };
      if (hasSteps) return { label: `${who} 执行中`, color: 'var(--status-info)' };
      return { label: `${who} 思考中`, color: 'var(--accent)' };
    }
    const anyWork =
      Object.values(toolCalls).some((c) => c.length > 0) ||
      Object.values(reasoning).some((r) => !!r);
    if (anyWork) return { label: '工具执行中', color: 'var(--status-info)' };
    return { label: '调度中', color: 'var(--accent)' };
  }, [currentRoom, currentSpeaker, streaming, reasoning, toolCalls, displayNameOf, mentionMap]);

  // 最早已加载消息在全局列表中的下标（配合 Virtuoso 向上加载更早消息时保持滚动位置）。
  const firstItemIndex = useMemo(
    () => Math.max(0, totalMessages - sortedMessages.length),
    [totalMessages, sortedMessages.length],
  );

  const renderMessage = useCallback(
    (index: number) => {
      const m = sortedMessages[index];
      if (!m) return null;
      const p = participantById.get(m.sender);
      const confirmation = m.kind === 'confirmation_request' ? parseConfirmation(m.extra) : null;
      const responded = confirmation ? respondedRequestIds.has(confirmation.requestId) : false;
      const submittedResponses = responded && confirmation
        ? replyResponsesByRequest.get(confirmation.requestId)
        : undefined;
      return (
        <GroupChatMessageItem
          message={m}
          participant={p}
          color={colorOf(p, index)}
          isUser={m.sender === 'user'}
          displayName={displayNameOf(m.sender)}
          mentionMap={mentionMap}
          responded={responded}
          submittedResponses={submittedResponses}
          onRespond={respondConfirmation}
        />
      );
    },
    [sortedMessages, participantById, colorOf, displayNameOf, mentionMap, respondedRequestIds, replyResponsesByRequest, respondConfirmation],
  );

  // 切房间 / 首屏加载完成后滚动到底部。
  const prevLoadingRef = useRef(true);
  useEffect(() => {
    if (!messagesLoading && prevLoadingRef.current) {
      requestAnimationFrame(() => {
        virtuosoRef.current?.scrollToIndex({ index: 'LAST', align: 'end' });
      });
    }
    prevLoadingRef.current = messagesLoading;
  }, [messagesLoading]);

  // 开始新一段发言时滚动到底部（讨论进行中跟随最新）。
  const prevSpeakerRef = useRef<string | null>(null);
  useEffect(() => {
    if (currentSpeaker && currentSpeaker !== prevSpeakerRef.current) {
      virtuosoRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'smooth' });
    }
    prevSpeakerRef.current = currentSpeaker;
  }, [currentSpeaker]);

  // 思考等待计时：当前发言者就位后每秒 +1（thinkingTick 即等待秒数），驱动「正在思考… Ns」与超时警示刷新。
  const [thinkingTick, setThinkingTick] = useState(0);
  useEffect(() => {
    if (!currentSpeaker) return;
    setThinkingTick(0);
    const t = setInterval(() => setThinkingTick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [currentSpeaker]);

  const Header = () => {
    if (loadingEarlier) {
      return (
        <div className="flex items-center justify-center gap-2 py-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          <div
            className="w-3.5 h-3.5 rounded-full border-2"
            style={{ borderColor: 'var(--border)', borderTopColor: 'var(--accent)', animation: 'pd-spin 0.8s linear infinite' }}
          />
          加载更早消息…
        </div>
      );
    }
    return null;
  };

  // ── 实时流式区（当前发言者输出 + 参与者工具调用）──
  // 不挂在 Virtuoso 的 Footer 上：内联定义组件会让 Virtuoso 每次渲染都重挂载 footer，
  // 流式 token 高频刷新时产生大量 DOM 重建，拖慢实时显示。改为在虚拟列表外独立渲染。
  const streamingEl = currentSpeaker
    ? (() => {
        const streamingText = streaming[currentSpeaker] ?? '';
        const streamingSteps = [
          ...reasoningToSteps(currentSpeaker, reasoning[currentSpeaker], mentionMap),
          ...realtimeToolCallsToSteps(toolCalls[currentSpeaker], mentionMap),
        ];
        if (!streamingText && streamingSteps.length === 0) {
          // 思考占位：当前发言者已就位但尚无任何输出（reasoning/token/tool）时给出可见反馈。
          const elapsed = thinkingTick;
          const stuck = elapsed >= 60;
          return (
            <div className="flex justify-start px-4 py-1.5">
              <div className="flex w-full items-start gap-2">
                <ParticipantAvatar icon="agent" color="#8b5cf6" />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 mb-0.5">
                    <span className="text-[10px] font-medium" style={{ color: 'var(--accent)' }}>
                      {displayNameOf(currentSpeaker)}
                    </span>
                    <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>正在思考</span>
                    <span className="flex items-center gap-0.5">
                      <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite' }} />
                      <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite 0.2s' }} />
                      <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite 0.4s' }} />
                    </span>
                    {elapsed > 0 && (
                      <span className="text-[10px]" style={{ color: stuck ? 'var(--status-warning)' : 'var(--text-tertiary)' }}>{elapsed}s</span>
                    )}
                  </div>
                  {stuck && (
                    <div className="text-[10px]" style={{ color: 'var(--status-warning)' }}>
                      长时间无响应，可暂停或发送消息中断
                    </div>
                  )}
                </div>
              </div>
            </div>
          );
        }
        return (
          <div className="flex justify-start px-4 py-1.5">
            <div className="flex w-full items-start gap-2">
              <ParticipantAvatar icon="agent" color="#8b5cf6" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 mb-0.5">
                  <span className="text-[10px] font-medium" style={{ color: 'var(--accent)' }}>
                    {displayNameOf(currentSpeaker)}
                  </span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>正在发言…</span>
                </div>
                <div
                  className="block w-full px-3 py-2 rounded-xl text-xs leading-relaxed whitespace-pre-wrap min-w-0 break-all"
                  style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                >
                  <ThinkingChain steps={streamingSteps} defaultCollapsed={false} />
                  {streamingText && (
                    <>
                      {/* 流式占位直接显示 markdown 源码：tight 化列表项间空行，与最终渲染的紧凑列表一致 */}
                      {collapseBlankLines(tightenListGaps(streamingText))}
                      <span
                        className="inline-block w-1.5 h-3 ml-0.5 align-middle"
                        style={{ backgroundColor: 'var(--accent)', animation: 'pd-breathe 1.2s ease-in-out infinite' }}
                      />
                    </>
                  )}
                </div>
              </div>
            </div>
          </div>
        );
      })()
    : null;

  const toolEls = Object.entries(toolCalls)
    .filter(([pid, calls]) => (calls.length > 0 || !!reasoning[pid]) && pid !== currentSpeaker)
    .map(([pid, calls]) => {
      const steps = [
        ...reasoningToSteps(pid, reasoning[pid], mentionMap),
        ...realtimeToolCallsToSteps(calls, mentionMap),
      ];
      if (steps.length === 0) return null;
      return (
        <div key={`tool-calls-${pid}`} className="flex justify-start px-4 py-1.5">
          <div className="flex w-full items-start gap-2">
            <div className="w-7 h-7 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 mb-0.5">
                <span className="text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>
                  {displayNameOf(pid)} · 工具调用
                </span>
              </div>
              <ThinkingChain
                steps={steps}
                defaultCollapsed={false}
                liveStepKeys={reasoning[pid] ? new Set([`reasoning-${pid}`]) : undefined}
              />
            </div>
          </div>
        </div>
      );
    });
  const hasLiveActivity = streamingEl !== null || toolEls.length > 0;
  // Actor 存活感知：区分"真 running"（Actor 在跑）与"假 running"（DB 仍标 running，
  // 但进程内无 Actor，如异常退出后未恢复——界面应显示恢复提示而非伪造的"调度/活动中"）。
  const runningAlive = currentRoom?.status === 'running' && actorAlive;
  const staleRunning = currentRoom?.status === 'running' && !actorAlive;
  const statusIsPaused = currentRoom?.status === 'paused';
  // 调度/审阅占位：仅"真运行"的房间显示"主席调度中"脉冲；
  // 覆盖 next_speaker / extract_stance / director_review 的等待空窗，避免消息区空白。
  const schedulingEl =
    (runningAlive || staleRunning) && !currentSpeaker && !hasLiveActivity ? (
      <div className="flex justify-start px-4 py-1.5">
        <div className="flex w-full items-start gap-2">
          <div className="w-7 h-7 shrink-0" />
          <div className="min-w-0 flex-1">
            {runningAlive ? (
              <div className="flex items-center gap-2 mb-0.5">
                <span className="text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>主席调度中</span>
                <span className="flex items-center gap-0.5">
                  <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite' }} />
                  <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite 0.2s' }} />
                  <span className="inline-block w-1 h-1 rounded-full" style={{ backgroundColor: 'var(--accent)', animation: 'pd-typing 1.2s ease-in-out infinite 0.4s' }} />
                </span>
              </div>
            ) : (
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                该房间 Actor 未在运行（上次会话可能已中断），点击右上角「恢复」重新启动讨论。
              </div>
            )}
          </div>
        </div>
      </div>
    ) : null;
  // 实时活动条带：仅"真运行"期间常驻（思考/调度/工具调用都在其中滚动）；假 running 不点亮活动区。
  const showLiveStrip = (hasLiveActivity || schedulingEl !== null) && runningAlive;

  // 主持人呼吸圆点：主持人从不被授予发言权（floor 只授予参与者），其"发言/调度"表现为
  // 调度空窗（无发言者且无其他实时活动）、自身实时推理、或（罕见）被指定为发言者。
  const directorId = participants.find((p) => p.participantType === 'director')?.id;
  const directorBusy =
    runningAlive &&
    (directorId === currentSpeaker ||
      (!!directorId && !!reasoning[directorId]) ||
      (!currentSpeaker && !hasLiveActivity));

  async function handleDeleteRoom(roomId: string) {
    if (!window.confirm('确定删除该群聊房间及其全部消息、任务与参与者数据？')) return;
    try {
      await deleteRoom(roomId);
    } catch (err) {
      console.warn('[GroupChat] 删除失败:', err);
    }
  }

  async function handleRemoveParticipant(participantId: string) {
    if (!window.confirm('确定移除该参与者？')) return;
    try {
      await removeParticipant(participantId);
    } catch (err) {
      console.warn('[GroupChat] 移除参与者失败:', err);
    }
  }

  const handlePickRoomDir = useCallback(async () => {
    if (!currentRoomId) {
      showToast('请先选择或创建一个房间', 'info');
      return;
    }
    try {
      const sel = await openDialog({ directory: true, multiple: false, title: '选择项目目录（产物/工作空间根）' });
      if (!sel || typeof sel !== 'string') return;
      await setRoomOutputDir(sel);
      showToast('项目目录已切换（后续消息以该目录为工作空间根）', 'info');
    } catch (e) {
      showToast(`切换项目目录失败: ${String(e)}`, 'error');
    }
  }, [currentRoomId, setRoomOutputDir]);

  const saveAttachments = useCallback(async (items: PendingAttachment[]): Promise<Attachment[]> => {
    if (!currentRoomId) {
      showToast('请先选择或创建一个房间', 'info');
      return [];
    }
    if (items.length === 0) return [];
    try {
      return await invoke<Attachment[]>('groupchat_save_attachments', { roomId: currentRoomId, items });
    } catch (e) {
      showToast(`附件保存失败: ${String(e)}`, 'error');
      return [];
    }
  }, [currentRoomId]);

  const addFromPaths = useCallback(async (paths: string[], forceKind?: 'image' | 'file') => {
    const items: PendingAttachment[] = paths.map((p) => {
      const name = p.split(/[\\/]/).pop() || 'file';
      const kind: 'image' | 'file' = forceKind ?? (isImageName(name) ? 'image' : 'file');
      return { kind, name, mime: '', path: p };
    });

    const current = attachmentsRef.current;
    const room = MAX_ATTACHMENTS - current.length;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${items.length} 个`, 'info');
      return;
    }

    const toSave = items.slice(0, room);
    const saved = await saveAttachments(toSave);
    if (saved.length > 0) {
      setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
    }

    const overflow = items.length - toSave.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [saveAttachments]);

  // 统一附件选择：不限制类型，落盘时由 isImageName 按扩展名智能区分 图片/文件。
  const handlePickAttachments = useCallback(async () => {
    const selected = await openDialog({ multiple: true });
    if (!selected) return;
    await addFromPaths(Array.isArray(selected) ? selected : [selected]);
  }, [addFromPaths]);

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

    const pending = await Promise.all(files.map(async (f) => {
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

    const current = attachmentsRef.current;
    const room = MAX_ATTACHMENTS - current.length;
    if (room <= 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略本次 ${pending.length} 个`, 'info');
      return;
    }
    const toSave = pending.slice(0, room);
    const saved = await saveAttachments(toSave);
    if (saved.length > 0) {
      setAttachments((prev) => [...prev, ...saved].slice(0, MAX_ATTACHMENTS));
    }
    const overflow = pending.length - toSave.length;
    if (overflow > 0) {
      showToast(`附件最多 ${MAX_ATTACHMENTS} 个，已忽略超出的 ${overflow} 个`, 'info');
    }
  }, [saveAttachments]);

  // 拖拽文件落盘
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
            setDraggingFiles(payload.paths.map((p) => {
              const name = p.split(/[\\/]/).pop() || 'file';
              return { name, kind: isImageName(name) ? 'image' : 'file' } as const;
            }));
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
        console.error('[GroupChat] 注册拖拽监听失败:', e);
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
    if (target?.path && currentRoomId) {
      try {
        await invoke('delete_attachment', { sessionId: currentRoomId, path: target.path });
      } catch (e) {
        console.warn('[GroupChat] 删除附件文件失败:', e);
      }
    }
  }, [attachments, currentRoomId]);

  // 切换房间时清理未发送附件（删除落盘文件 + 清空状态）
  const prevRoomIdRef = useRef<string | null>(currentRoomId);
  useEffect(() => {
    const prevId = prevRoomIdRef.current;
    if (prevId === currentRoomId) return;
    const old = attachmentsRef.current;
    old.forEach((att) => {
      if (att.path && prevId) {
        invoke('delete_attachment', { sessionId: prevId, path: att.path }).catch(() => {});
      }
    });
    setAttachments([]);
    setDragActive(false);
    setDraggingFiles([]);
    prevRoomIdRef.current = currentRoomId;
  }, [currentRoomId]);

  function handleInputChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const value = e.target.value;
    setInput(value);
    // 检测最后一个 @ 后的连续文本作为提及查询词；@ 后遇到空白/再次 @ 则关闭下拉。
    // 单独输入 @（其后为空）也打开下拉，展示全部可 @ 参与者。
    const atIdx = value.lastIndexOf('@');
    if (atIdx >= 0) {
      const after = value.slice(atIdx + 1);
      if (!/[\s@]/.test(after)) {
        setMentionQuery(after);
        setMentionOpen(true);
        return;
      }
    }
    setMentionOpen(false);
  }

  function handleMentionSelect(p: GroupChatParticipant) {
    const atIdx = input.lastIndexOf('@');
    const next = (atIdx >= 0 ? input.slice(0, atIdx) : input) + `@${p.displayName} `;
    setInput(next);
    setMentionOpen(false);
    setMentionQuery('');
  }

  async function handleSend() {
    let content = input.trim();
    const hasAttachments = attachments.length > 0;
    if ((!content && !hasAttachments) || !currentRoomId) return;

    // 解析 @ 提及：匹配 content 中第一个可 @ 参与者（api/cli）的 displayName，
    // 并把命中的 `@显示名` 替换为 `[@id]`（id 唯一引用落库，后端另有兜底归一化）。
    let mention: string | null = null;
    let recipients: string[] = [];
    for (const p of participants) {
      if (p.participantType !== 'api' && p.participantType !== 'cli') continue;
      if (content.includes(`@${p.displayName}`)) {
        mention = p.id;
        recipients = [p.id];
        content = content.replace(`@${p.displayName}`, `[@${p.id}]`);
        break;
      }
    }

    setInput('');
    // 发送后重置输入框高度为单行
    if (inputAreaRef.current) {
      inputAreaRef.current.style.height = 'auto';
    }
    setMentionOpen(false);
    try {
      await sendMessage(content, hasAttachments ? attachments : undefined, mention, recipients, securityMode);
      setAttachments([]);
    } catch (err) {
      console.warn('[GroupChat] 发送失败:', err);
    }
  }

  async function handleExport() {
    if (!currentRoomId) return;
    try {
      setExporting(true);
      const result = await exportWorkflow();
      setExportJson(JSON.stringify(result, null, 2));
    } catch (err) {
      setExportJson(JSON.stringify({ error: String(err) }, null, 2));
    } finally {
      setExporting(false);
    }
  }

  async function handleSaveExport() {
    if (!exportJson) return;
    try {
      const filePath = await saveDialog({
        title: '保存工作流定义',
        defaultPath: '工作流.json',
        filters: [{ name: '工作流文件', extensions: ['json'] }],
      });
      if (!filePath) return;
      await invoke('write_text_file', { path: filePath, content: exportJson });
      showToast('已保存到文件', 'success');
    } catch (err) {
      console.warn('[GroupChat] 保存失败:', err);
      showToast(`保存失败: ${err}`, 'error');
    }
  }

  return (
    <div className="flex-1 flex overflow-hidden">
      {/* ── 左：房间列表（宽度与会话页左侧会话列表 w-[260px] 保持一致） ── */}
      <aside
        className="w-[260px] shrink-0 flex flex-col overflow-hidden"
        style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        <div className="flex items-center px-3 h-10" style={{ borderBottom: '1px solid var(--border)' }}>
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>群聊房间</span>
          <div className="flex-1" />
          <button
            className="pd-btn pd-btn-sm pd-btn-primary"
            title="新建群聊房间"
            style={{ display: 'flex', alignItems: 'center', gap: 4 }}
            onClick={() => setShowCreate(true)}
          >
            <Plus size={11} />
            新建
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-2 space-y-1.5">
          {rooms.length === 0 && (
            <div className="px-3 py-4 text-[11px] text-center" style={{ color: 'var(--text-tertiary)' }}>
              暂无房间，点击「新建」创建。
            </div>
          )}
          {rooms.map((r) => {
            // 仅当 Actor 存活（真运行）才点亮绿点脉冲；DB 残留 running 但无 Actor 的房间显示"待恢复"。
            const actorOn = !!roomsActorAlive[r.id];
            const staleRun = r.status === 'running' && !actorOn;
            const statusLabel = staleRun ? '待恢复' : STATUS_LABEL[r.status];
            const statusColor = staleRun ? 'var(--status-warning, #f59e0b)' : STATUS_COLOR[r.status];
            return (
            <div key={r.id} className="relative group">
              <button
                onClick={() => selectRoom(r.id)}
                className="w-full text-left px-3 py-2 rounded-lg transition-colors flex flex-col"
                style={{
                  backgroundColor: currentRoomId === r.id ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                  border: `1px solid ${currentRoomId === r.id ? 'var(--accent)' : 'var(--border)'}`,
                  alignItems: 'stretch',
                }}
              >
                <div className="flex items-center gap-1.5 text-xs pr-4" style={{ color: 'var(--text-primary)' }}>
                  <Users size={11} className="shrink-0" />
                  <span className="truncate">{r.title}</span>
                </div>
                <div className="flex items-center justify-between gap-1.5 mt-1 text-[10px] whitespace-nowrap" style={{ color: 'var(--text-tertiary)' }}>
                  <span className="truncate">{r.topic}</span>
                  <span className="px-1 rounded shrink-0 flex items-center gap-1" style={{ color: statusColor }}>
                    {r.status === 'running' && actorOn && (
                      <span
                        className="inline-block w-1.5 h-1.5 rounded-full shrink-0"
                        style={{ backgroundColor: statusColor, animation: 'pd-breathe 1.2s ease-in-out infinite' }}
                      />
                    )}
                    {statusLabel}
                  </span>
                </div>
              </button>
              <button
                onClick={(e) => { e.stopPropagation(); handleDeleteRoom(r.id); }}
                className="absolute top-1.5 right-1.5 p-0.5 rounded opacity-0 group-hover:opacity-100 transition-opacity"
                style={{ color: 'var(--text-tertiary)' }}
                title="删除房间"
              >
                <Trash2 size={12} />
              </button>
            </div>
            );
          })}
        </div>
      </aside>

      {/* ── 中：消息流 ── */}
      <main className="flex-1 flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <div className="flex items-center gap-2 px-4 h-10 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>
            {currentRoom?.title || '选择或创建一个群聊房间'}
          </span>
          {currentRoom && (
            <span className="text-[10px] px-1.5 py-0.5 rounded-full" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
              第 {currentRound} 轮
            </span>
          )}
          {currentRoom && (
            <span className="text-[10px] px-1.5 py-0.5 rounded-full" style={{ backgroundColor: 'var(--accent-light)', color: phaseInfo.color }}>
              {phaseInfo.label}
            </span>
          )}
          <div className="flex-1" />
          {currentRoom && (
            <button
              onClick={() => selectRoom('')}
              className="p-1.5 rounded-md hover:opacity-70 transition-opacity shrink-0"
              style={{ color: 'var(--text-tertiary)' }}
              title="关闭当前会话实例"
            >
              <X size={13} />
            </button>
          )}
          {/* 运行/暂停/已中止 房间显示控制按钮；running 分真/假（Actor 存活）：
              真在跑 → 暂停；假 running（Actor 不在，异常退出残留）→ 恢复（重启 Actor 续跑） */}
          {currentRoom && (currentRoom.status === 'running' || currentRoom.status === 'paused' || currentRoom.status === 'aborted') && (
            <>
              <button
                onClick={() => (runningAlive ? pause() : resume())}
                className="pd-btn pd-btn-sm"
                style={{ color: 'var(--text-secondary)' }}
                title={
                  runningAlive ? '暂停讨论'
                  : statusIsPaused ? '继续讨论'
                  : '恢复运行（重新启动房间 Actor）'
                }
              >
                {runningAlive ? <Pause size={11} /> : <Play size={11} />}
                {runningAlive ? '暂停' : (statusIsPaused ? '继续' : '恢复')}
              </button>
              {(runningAlive || statusIsPaused) && (
                <button
                  onClick={() => abort()}
                  className="pd-btn pd-btn-sm"
                  style={{ color: 'var(--status-danger)' }}
                  title="结束讨论"
                >
                  <Square size={11} />
                  结束
                </button>
              )}
            </>
          )}
          {currentRoom && currentRoom.status === 'finished' && (
            <button
              onClick={handleExport}
              className="pd-btn pd-btn-sm"
              style={{ color: 'var(--text-secondary)' }}
              disabled={exporting}
            >
              <Download size={11} />
              {exporting ? '导出中…' : '导出为工作流'}
            </button>
          )}
        </div>

        <div className="flex-1 overflow-hidden">
          {!currentRoomId ? (
            <GroupChatIntro />
          ) : messagesLoading ? (
            <div className="h-full flex flex-col items-center justify-center gap-3 text-xs" style={{ color: 'var(--text-tertiary)' }}>
              <div
                className="w-6 h-6 rounded-full border-2"
                style={{ borderColor: 'var(--border)', borderTopColor: 'var(--accent)', animation: 'pd-spin 0.8s linear infinite' }}
              />
              <span>加载消息中…</span>
            </div>
          ) : (
            <Virtuoso
              ref={virtuosoRef}
              className="h-full"
              data={sortedMessages}
              firstItemIndex={firstItemIndex}
              itemContent={renderMessage}
              followOutput="smooth"
              startReached={loadEarlierMessages}
              increaseViewportBy={{ top: 400, bottom: 400 }}
              components={{ Header }}
            />
          )}
        </div>

        {/* 实时活动区：思考/发言/工具调用/调度占位（虚拟列表外独立渲染，不受列表滚动影响） */}
        {showLiveStrip && (
          <div
            className="shrink-0 overflow-y-auto"
            style={{ maxHeight: '30%', borderTop: '1px solid var(--border)', backgroundColor: 'var(--bg-primary)' }}
          >
            <div
              className="sticky top-0 z-10 px-4 py-1 text-[10px] flex items-center gap-1.5 shrink-0"
              style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
            >
              <Radio size={10} style={{ color: 'var(--accent)', animation: 'pd-activity 1.6s ease-in-out infinite' }} />
              实时活动
            </div>
            {streamingEl}
            {schedulingEl}
            {toolEls}
          </div>
        )}

        <div className="shrink-0" style={{ borderTop: '1px solid var(--border)' }} ref={inputBarRef}>
          {/* 工具行：会话安全模式 + 附件 + 项目根 + 发送（仿会话页工具栏） */}
          <div className="flex items-center gap-1 px-4 pt-3">
            <SecurityModeSelector value={securityMode} onChange={setSecurityMode} />
            <button
              onClick={handlePickAttachments}
              className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
              style={{
                color: attachments.length > 0 ? 'var(--accent)' : 'var(--text-secondary)',
                backgroundColor: attachments.length > 0 ? 'var(--border)' : 'transparent',
              }}
              title="添加附件（图片/文件，支持拖拽/粘贴，最多 8 个）"
            >
              <Paperclip size={11} />
              附件
            </button>
            <button
              onClick={handlePickRoomDir}
              className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0 max-w-[180px]"
              style={{ color: currentRoom?.outputDir ? 'var(--text-primary)' : 'var(--text-tertiary)', backgroundColor: 'transparent' }}
              title={currentRoom?.outputDir
                ? `项目/工作空间根：${currentRoom.outputDir}\n点击可切换（后续消息生效）`
                : '未设置项目目录（运行时默认 <工作目录>/outputs/ 下），点击选择'}
            >
              <Folder size={11} style={{ flexShrink: 0 }} />
              <span className="truncate">{currentRoom?.outputDir ? currentRoom.outputDir.split(/[\\/]/).pop() || currentRoom.outputDir : '默认项目'}</span>
            </button>
            <div className="flex-1" />
            <button
              onClick={handleSend}
              disabled={!currentRoomId || (!input.trim() && attachments.length === 0)}
              className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors disabled:opacity-30"
              style={{
                backgroundColor: (input.trim() || attachments.length > 0) && currentRoomId ? 'var(--accent)' : 'var(--bg-tertiary)',
                color: (input.trim() || attachments.length > 0) && currentRoomId ? '#fff' : 'var(--text-secondary)',
              }}
              title="发送"
            >
              <Send size={11} />
              发送
            </button>
          </div>

          {/* 工具栏底部分隔线（仿会话页） */}
          <div className="px-4 py-1">
            <div style={{ borderBottom: '1px dashed var(--border)' }} />
          </div>

          {/* 输入区（仿会话页：左右边距 px-4，顶部 pt-1 底部 pb-3） */}
          <div className="flex items-end px-4 pt-1 pb-3">
            <div className="flex-1 relative">
              {mentionOpen && mentionCandidates.length > 0 && (
                <div
                  className="absolute bottom-full left-0 right-0 mb-1 p-1 rounded-lg shadow-lg z-20 max-h-48 overflow-y-auto"
                  style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                >
                  {mentionCandidates.map((p) => (
                    <button
                      key={p.id}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => handleMentionSelect(p)}
                      className="w-full text-left px-2 py-1.5 rounded text-xs flex items-center gap-2"
                      style={{ color: 'var(--text-primary)' }}
                      onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.backgroundColor = 'var(--bg-tertiary)'; }}
                      onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.backgroundColor = 'transparent'; }}
                    >
                      <span className="truncate">{p.displayName}</span>
                      <span className="text-[10px] ml-auto shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                        {p.participantType === 'cli' ? 'CLI' : 'API'}
                      </span>
                    </button>
                  ))}
                </div>
              )}

              <textarea
                ref={inputAreaRef}
                rows={1}
                value={input}
                onChange={handleInputChange}
                onPaste={handlePaste}
                placeholder={currentRoomId ? '输入发言，启动或继续讨论…（Shift+Enter 换行）' : '请先选择或创建一个房间'}
                onInput={(e) => {
                  const el = e.currentTarget;
                  el.style.height = 'auto';
                  el.style.height = Math.min(el.scrollHeight, 200) + 'px';
                }}
                className="w-full px-3 py-2 text-xs outline-none resize-none focus:outline-none focus-visible:outline-none"
                style={{
                  backgroundColor: 'transparent',
                  color: 'var(--text-primary)',
                  border: 'none',
                  boxShadow: 'none',
                  outline: 'none',
                  minHeight: '36px',
                  maxHeight: '200px',
                  overflowY: 'auto',
                  lineHeight: '16px',
                }}
                onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSend(); } }}
              />

              {/* 附件预览：最后一行（仿会话页，位于输入框下方） */}
              {(attachments.length > 0 || dragActive) && (
                <div
                  className="flex flex-wrap gap-2 mt-2 p-2 rounded-lg transition-colors"
                  style={{
                    backgroundColor: dragActive ? 'var(--accent)0d' : 'transparent',
                    border: `1px dashed ${dragActive ? 'var(--accent)' : 'var(--border)'}`,
                  }}
                >
                  {attachments.map((att, idx) => (
                    <div
                      key={`${idx}-${att.path}`}
                      className="relative rounded-lg overflow-hidden shrink-0 flex items-center justify-center"
                      style={{
                        width: att.kind === 'image' ? 64 : 120,
                        height: att.kind === 'image' ? 64 : 40,
                        border: '1px solid var(--border)',
                        backgroundColor: 'var(--bg-tertiary)',
                      }}
                    >
                      {att.kind === 'image' ? (
                        <img src={convertFileSrc(att.path)} alt={att.name} className="w-full h-full object-cover" />
                      ) : (
                        <div className="flex items-center gap-1 px-2 max-w-full">
                          <FileText size={12} style={{ color: 'var(--text-secondary)', flexShrink: 0 }} />
                          <span className="text-[11px] truncate" style={{ color: 'var(--text-primary)' }}>{att.name}</span>
                        </div>
                      )}
                      <button
                        onClick={() => removeAttachment(idx)}
                        className="absolute top-0.5 right-0.5 w-4 h-4 rounded-full flex items-center justify-center"
                        style={{ backgroundColor: 'rgba(0,0,0,0.6)', color: '#fff' }}
                        title="移除附件"
                      >
                        <X size={10} />
                      </button>
                    </div>
                  ))}
                  {dragActive && draggingFiles.map((f, i) => (
                    <div
                      key={`drag-${i}-${f.name}`}
                      className="relative rounded-lg overflow-hidden shrink-0 flex items-center justify-center opacity-70"
                      style={{
                        width: f.kind === 'image' ? 64 : 120,
                        height: f.kind === 'image' ? 64 : 40,
                        border: '1px dashed var(--accent)',
                        backgroundColor: 'var(--bg-tertiary)',
                      }}
                    >
                      {f.kind === 'image' ? (
                        <ImagePlus size={16} style={{ color: 'var(--accent)' }} />
                      ) : (
                        <div className="flex items-center gap-1 px-2 max-w-full">
                          <FileText size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />
                          <span className="text-[11px] truncate" style={{ color: 'var(--text-secondary)' }}>{f.name}</span>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>

          {/* 用量行：紧贴输入区底部（仿会话页 SessionUsageBar） */}
          <RoomUsageBar roomId={currentRoomId} />
        </div>
      </main>

      {/* ── 右：讨论（参与者 + 立场 + 任务） / 文件历史（tab 切换）；显隐由外层折叠按钮控制 ── */}
      {rightPanelOpen && (
        <aside
          className="w-[240px] shrink-0 flex flex-col overflow-hidden"
          style={{ borderLeft: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
        >
        {/* Tab 切换栏：讨论 / 文件历史（h-10 与消息窗口顶部栏、左栏列表头部等高） */}
        <div
          className="flex h-10 shrink-0"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
        >
          <button
            onClick={() => setRightTab('overview')}
            className="px-3 flex items-center justify-center gap-1 text-[10px] font-medium transition-colors whitespace-nowrap"
            style={{
              color: rightTab === 'overview' ? 'var(--accent)' : 'var(--text-secondary)',
              borderBottom: rightTab === 'overview' ? '2px solid var(--accent)' : '2px solid transparent',
            }}
            title="参与者 / 产物目录 / 任务"
          >
            <Users size={11} />
            讨论
          </button>
          <button
            onClick={() => setRightTab('stances')}
            className="px-3 flex items-center justify-center gap-1 text-[10px] font-medium transition-colors whitespace-nowrap"
            style={{
              color: rightTab === 'stances' ? 'var(--accent)' : 'var(--text-secondary)',
              borderBottom: rightTab === 'stances' ? '2px solid var(--accent)' : '2px solid transparent',
            }}
            title="参与者立场快照（从讨论页独立，节省空间）"
          >
            <Scale size={11} />
            立场
            {stances.length > 0 && (
              <span
                className="px-1 rounded-full text-[9px] leading-[14px]"
                style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
              >
                {stances.length}
              </span>
            )}
          </button>
          <button
            onClick={() => setRightTab('files')}
            className="px-3 flex items-center justify-center gap-1 text-[10px] font-medium transition-colors whitespace-nowrap"
            style={{
              color: rightTab === 'files' ? 'var(--accent)' : 'var(--text-secondary)',
              borderBottom: rightTab === 'files' ? '2px solid var(--accent)' : '2px solid transparent',
            }}
            title="当前房间文件修改历史"
          >
            <History size={11} />
            文件历史
            {!historyLoading && fileHistoryEnabled && roomHistory.length > 0 && (
              <span
                className="px-1 rounded-full text-[9px] leading-[14px]"
                style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
              >
                {roomHistory.length}
              </span>
            )}
          </button>
        </div>

        {rightTab === 'stances' ? (
          <>
            {/* ── 立场快照 tab：独立展示参与者立场（自讨论页移出，节省 overview 空间） ── */}
            <div className="px-3 py-2 text-[10px] font-medium flex items-center gap-1 shrink-0" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
              <Scale size={10} style={{ color: 'var(--accent)' }} />
              立场快照
              {stances.length > 0 && (
                <span className="ml-auto flex items-center gap-1.5" style={{ color: 'var(--text-tertiary)' }}>
                  <span>支持 <b style={{ color: 'var(--status-success)' }}>{stanceStats.agree}</b></span>
                  <span>反对 <b style={{ color: 'var(--status-danger)' }}>{stanceStats.disagree}</b></span>
                  <span>中立 <b style={{ color: 'var(--text-tertiary)' }}>{stanceStats.neutral}</b></span>
                </span>
              )}
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto p-2 space-y-1.5" style={{ backgroundColor: 'var(--bg-primary)' }}>
              {stances.length === 0 && (
                <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  暂无立场
                </div>
              )}
              {stances.map((s, idx) => {
                const p = participantById.get(s.participantId);
                const color = colorOf(p, idx);
                const agree = s.attitude === 'agree';
                const disagree = s.attitude === 'disagree';
                const icon = agree ? <CheckCircle size={11} style={{ color: 'var(--status-success)' }} />
                  : disagree ? <XCircle size={11} style={{ color: 'var(--status-danger)' }} />
                    : <AlertCircle size={11} style={{ color: 'var(--text-tertiary)' }} />;
                const expanded = expandedStances.has(s.participantId);
                return (
                  <div
                    key={s.participantId}
                    className="px-2 py-1.5 rounded-lg cursor-pointer transition-colors"
                    style={{ backgroundColor: 'var(--bg-tertiary)' }}
                    onClick={() => toggleStance(s.participantId)}
                    title={expanded ? '点击折叠' : '点击展开详细内容'}
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="shrink-0">{icon}</span>
                      <div className="text-[11px] truncate" style={{ color }}>{p?.displayName ?? s.participantId}</div>
                      <ChevronDown
                        size={10}
                        className="ml-auto shrink-0 transition-transform"
                        style={{ color: 'var(--text-tertiary)', transform: expanded ? 'rotate(180deg)' : 'none' }}
                      />
                    </div>
                    <div className={`mt-0.5 text-[10px] leading-snug break-words ${expanded ? '' : 'truncate'}`} style={{ color: 'var(--text-primary)' }}>{renderMentions(s.stance, mentionMap)}</div>
                  </div>
                );
              })}
            </div>
          </>
        ) : rightTab === 'overview' ? (
          <>
            <div className="px-3 py-2 text-[10px] font-medium flex items-center justify-between shrink-0" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
              <span className="flex items-center gap-1"><Users size={10} style={{ color: 'var(--accent)' }} />参与者 ({participants.length})</span>
              {currentRoomId && (
                <button
                  className="pd-btn pd-btn-sm"
                  title="添加参与者"
                  style={{ color: 'var(--text-secondary)' }}
                  onClick={() => setShowAdd(true)}
                >
                  <Plus size={11} />
                </button>
              )}
            </div>
            <div
              className="p-2 grid grid-cols-3 gap-1.5 shrink-0 overflow-y-auto"
              style={{ maxHeight: '190px', backgroundColor: 'var(--bg-primary)', borderBottom: '1px solid var(--border)' }}
            >
              {participants.length === 0 && (
                <div className="col-span-3 px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  暂无参与者
                </div>
              )}
              {participants.map((p, idx) => {
                const color = colorOf(p, idx);
                const removable = p.participantType === 'api' || p.participantType === 'cli';
                // 状态守卫：房间非运行（已暂停/结束/中止）时即使 currentSpeaker/执行中任务未及时清空，也不点亮呼吸圆点。
                // 主持人无 floor：其"发言/调度"用 directorBusy（调度空窗/自身推理/被指定发言）判定。
                const isSpeaking = currentRoom?.status === 'running' &&
                  (p.participantType === 'director' ? directorBusy : (p.id === currentSpeaker || executingIds.has(p.id)));
                return (
                  <div
                    key={p.id}
                    className="group relative flex flex-col items-center gap-0.5 px-1 py-1.5 rounded-lg"
                    style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                  >
                    <div className="relative">
                      <ParticipantAvatar icon={participantIcon(p.participantType)} color={color} small />
                      {isSpeaking && (
                        <span
                          className="absolute -top-0.5 -right-0.5 w-1.5 h-1.5 rounded-full"
                          style={{ backgroundColor: 'var(--accent)', animation: 'pd-breathe 1.2s ease-in-out infinite' }}
                        />
                      )}
                    </div>
                    <div className="w-full text-center text-[10px] leading-tight truncate" style={{ color: 'var(--text-primary)' }} title={p.displayName}>{p.displayName}</div>
                    <div
                      className="w-full text-center text-[9px] leading-tight truncate"
                      style={{ color: p.participantType === 'director' ? 'var(--accent)' : 'var(--text-tertiary)' }}
                      title={p.participantType === 'director' ? '主席' : (p.systemRole || p.participantType)}
                    >
                      {p.participantType === 'director' ? '主席' : (p.systemRole || p.participantType)}
                    </div>
                    {removable && (
                      <button
                        className="absolute top-0.5 right-0.5 p-0.5 rounded opacity-0 group-hover:opacity-100"
                        style={{ color: 'var(--status-danger)' }}
                        title="移除参与者"
                        onClick={() => handleRemoveParticipant(p.id)}
                      >
                        <XCircle size={11} />
                      </button>
                    )}
                  </div>
                );
              })}
            </div>

            <div className="px-3 py-2 text-[10px] font-medium flex items-center gap-1 shrink-0" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
              <Folder size={10} style={{ color: 'var(--accent)' }} />
              产物目录
            </div>
            {/* 内容区背景与「立场快照」内容区统一（bg-primary），标题行与立场快照标题行一致（bg-secondary） */}
            <div className="px-3 py-2 flex flex-col gap-1.5 shrink-0" style={{ backgroundColor: 'var(--bg-primary)', borderBottom: '1px solid var(--border)' }}>
              <div className="flex items-center gap-1">
                <input
                  type="text"
                  value={outputDirDraft}
                  onChange={(e) => setOutputDirDraft(e.target.value)}
                  placeholder="产物统一落盘目录（绝对路径）"
                  className="flex-1 min-w-0 rounded-md px-2 py-1 text-[10px] outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)', color: 'var(--text-primary)' }}
                />
                <button
                  type="button"
                  className="px-2 py-1 rounded-md text-[10px] shrink-0"
                  style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
                  onClick={async () => {
                    const sel = await openDialog({ directory: true, multiple: false, title: '选择产物目录' });
                    if (typeof sel === 'string') setOutputDirDraft(sel);
                  }}
                >
                  选择
                </button>
                <button
                  className="px-2 py-1 rounded-md text-[10px] shrink-0"
                  style={{ color: 'var(--accent)', border: '1px solid var(--accent)', backgroundColor: 'var(--accent-light)' }}
                  onClick={async () => {
                    const v = outputDirDraft.trim();
                    if (currentRoomId && v !== (currentRoom?.outputDir ?? '')) {
                      await setRoomOutputDir(v);
                    }
                  }}
                >
                  保存
                </button>
              </div>
              <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                当前：{currentRoom?.outputDir || '（目标理解阶段由主持人命名于 <工作目录>/outputs/ 下）'}；所有参与者产物统一写入该目录。
              </span>
            </div>

            <div className="px-3 py-2 text-[10px] font-medium flex items-center gap-1 shrink-0" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
              <ListTodo size={10} style={{ color: 'var(--accent)' }} />
              子任务
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto p-2 space-y-1.5" style={{ backgroundColor: 'var(--bg-primary)' }}>
              {tasks.length === 0 && (
                <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  暂无任务
                </div>
              )}
              {tasks.map((t) => {
                const expanded = expandedTasks.has(t.id);
                return (
                <div
                  key={t.id}
                  className="px-2 py-1.5 rounded-lg text-[10px] cursor-pointer transition-colors"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
                  onClick={() => toggleTask(t.id)}
                  title={expanded ? '点击折叠' : '点击展开任务详情'}
                >
                  <div className="flex items-start gap-1.5">
                    <span className="font-medium shrink-0" style={{ color: 'var(--text-secondary)' }}>T{t.taskNo}</span>
                    <span className={`flex-1 ${expanded ? 'break-words' : 'truncate'}`}>{renderMentions(t.description, mentionMap)}</span>
                    <ChevronDown
                      size={10}
                      className="mt-0.5 shrink-0 transition-transform"
                      style={{ color: 'var(--text-tertiary)', transform: expanded ? 'rotate(180deg)' : 'none' }}
                    />
                  </div>
                  <div className="flex items-center justify-between mt-1">
                    <span className="truncate" style={{ color: 'var(--text-tertiary)' }}>
                      {t.assignee ? displayNameOf(t.assignee) : '未指派'}
                    </span>
                    <span
                      className="ml-1 px-1 rounded shrink-0"
                      style={{
                        backgroundColor:
                          t.status === 'success' ? 'var(--status-success-bg)'
                          : t.status === 'failed' ? 'var(--status-danger-bg)'
                          : t.status === 'skipped' ? 'var(--status-warning-bg)'
                          : t.status === 'running' ? 'var(--accent-light)'
                          : 'var(--bg-tertiary)',
                        color:
                          t.status === 'success' ? 'var(--status-success)'
                          : t.status === 'failed' ? 'var(--status-danger)'
                          : t.status === 'skipped' ? 'var(--status-warning)'
                          : t.status === 'running' ? 'var(--accent)'
                          : 'var(--text-tertiary)',
                      }}
                    >
                      {TASK_STATUS_LABEL[t.status] ?? t.status}
                    </span>
                  </div>
                </div>
                );
              })}
            </div>
          </>
        ) : (
          <>
            {/* ── 文件历史 tab：当前房间 write_file/edit_file 快照，可撤销，占满整栏 ── */}
            {/* 记录开关：持久化 tool_overrides.groupchat 并热切换运行期标记（当前房间立即生效） */}
            <div className="px-2 py-2 shrink-0 flex items-center justify-between gap-2" style={{ backgroundColor: 'var(--bg-primary)', borderBottom: '1px solid var(--border)' }}>
              <div className="min-w-0">
                <div className="text-[10px] font-medium flex items-center gap-1" style={{ color: 'var(--text-primary)' }}>
                  <History size={10} style={{ color: 'var(--accent)' }} />
                  文件历史记录
                </div>
                <div className="text-[9px] truncate" style={{ color: 'var(--text-tertiary)' }}>
                  {fileHistoryEnabled ? '已开启，write_file/edit_file 修改自动记录' : '已关闭，不再记录文件修改'}
                </div>
              </div>
              <button
                role="switch"
                aria-checked={fileHistoryEnabled}
                onClick={() => {
                  const next = !fileHistoryEnabled;
                  // 乐观更新，失败回滚
                  setFileHistoryEnabled(next);
                  invoke('groupchat_set_file_history', { enabled: next })
                    .catch(() => {
                      setFileHistoryEnabled(!next);
                    });
                }}
                className="shrink-0 relative rounded-full transition-colors"
                style={{
                  width: 30,
                  height: 16,
                  backgroundColor: fileHistoryEnabled ? 'var(--accent)' : 'var(--border-strong, var(--border))',
                }}
                title={fileHistoryEnabled ? '点击关闭文件历史记录' : '点击开启文件历史记录'}
              >
                <span
                  className="absolute top-[2px] rounded-full transition-all"
                  style={{
                    width: 12,
                    height: 12,
                    left: fileHistoryEnabled ? 16 : 2,
                    backgroundColor: '#fff',
                    boxShadow: '0 1px 2px rgba(0,0,0,0.2)',
                  }}
                />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-2 space-y-1.5" style={{ backgroundColor: 'var(--bg-primary)' }}>
              {historyLoading ? (
                <div className="flex items-center justify-center py-3">
                  <Loader2 size={14} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
                </div>
              ) : !fileHistoryEnabled ? (
                <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  文件历史记录已关闭，打开开关后可继续记录。
                </div>
              ) : roomHistory.length === 0 ? (
                <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  暂无文件修改记录
                </div>
              ) : (
                roomHistory.map((e) => (
                  <div
                    key={e.id}
                    className="px-2 py-1.5 rounded-lg cursor-pointer transition-opacity hover:opacity-75"
                    style={{ backgroundColor: 'var(--bg-tertiary)' }}
                    onClick={() => invoke('open_path', { path: e.filePath }).catch(() => {})}
                    title={`${e.filePath}（点击打开文件）`}
                  >
                    <div className="flex items-center gap-1">
                      <FileText size={10} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
                      <span className="text-[10px] truncate font-mono flex-1" style={{ color: 'var(--text-primary)' }} title={e.filePath}>
                        {e.filePath}
                      </span>
                    </div>
                    <div className="flex items-center justify-between mt-0.5">
                      <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                        {formatTime(e.createdAt)}
                      </span>
                      <button
                        onClick={(ev) => { ev.stopPropagation(); undoRoomHistory(e.id); }}
                        disabled={historyUndoingId === e.id}
                        className="flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[9px] transition-colors disabled:opacity-40"
                        style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--accent)', border: '1px solid var(--border)' }}
                        title="撤销到修改前版本"
                      >
                        {historyUndoingId === e.id ? <Loader2 size={9} className="animate-spin" /> : <Undo2 size={9} />}
                        撤销
                      </button>
                    </div>
                  </div>
                ))
              )}
            </div>
            </>
          )}
        </aside>
      )}

      {showCreate && <CreateRoomModal onClose={() => setShowCreate(false)} />}

      {showAdd && <AddParticipantModal onClose={() => setShowAdd(false)} />}

      {error && (
        <div
          className="fixed bottom-16 left-1/2 -translate-x-1/2 z-50 px-4 py-2 rounded-lg text-xs shadow-lg"
          style={{ color: '#fff', backgroundColor: 'var(--status-danger)' }}
        >
          {error}
        </div>
      )}

      {exportJson !== null && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ backgroundColor: 'rgba(0,0,0,0.4)' }}
          onClick={() => setExportJson(null)}
        >
          <div
            className="w-[600px] max-h-[80vh] flex flex-col rounded-xl p-4 gap-3"
            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>工作流定义草稿</span>
              <button className="pd-btn pd-btn-sm" style={{ color: 'var(--text-secondary)' }} onClick={() => setExportJson(null)}>
                <XCircle size={14} />
              </button>
            </div>
            <pre
              className="flex-1 overflow-auto rounded-lg p-3 text-[11px] leading-relaxed"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            >
              {exportJson}
            </pre>
            <div className="flex justify-end gap-2">
              <button
                className="pd-btn pd-btn-sm"
                style={{ color: 'var(--text-secondary)' }}
                onClick={handleSaveExport}
              >
                <Save size={11} />
                保存
              </button>
              <button
                className="pd-btn pd-btn-sm pd-btn-primary"
                onClick={() => navigator.clipboard?.writeText(exportJson)}
              >
                <Copy size={11} />
                复制
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default GroupChatPage;
