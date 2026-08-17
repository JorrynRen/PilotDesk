import { useEffect, useMemo, useRef, useCallback, useState, memo } from 'react';
import { createPortal } from 'react-dom';
import {
  Users, Plus, Bot, User, Sparkles, Send, Pause, Play, Square,
  GitBranch, Activity, CheckCircle, XCircle, AlertCircle, Download, Copy, Trash2,
  ImagePlus, FileText, X, Clock, Flag, Save,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { useGroupChatStore } from '../stores/groupChatStore';
import { useApiProviderStore } from '../stores/apiProviderStore';
import { useAgentRegistry } from '../hooks/useAgentRegistry';
import { showToast } from '../utils/toast';
import { MarkdownRenderer, collapseBlankLines, linkifyUrls } from '../components/message/MarkdownRenderer';
import { ConfirmationCard, parseConfirmation } from '../components/confirmation/ConfirmationCard';
import { ThinkingChain } from '../components/message/ThinkingChain';
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

/** 解析持久化 toolCalls JSON 为聚合展示步骤（与 MessageBubble/会话模式格式对齐）。 */
function parseToolCalls(raw: string | undefined | null): ThinkingChainStep[] {
  if (!raw) return [];
  try {
    const chain = JSON.parse(raw);
    if (!Array.isArray(chain)) return [];
    const steps: ThinkingChainStep[] = [];
    for (const s of chain) {
      if (s.type === 'reasoning') {
        steps.push({ id: `r-${s.id || steps.length}`, type: 'reasoning', content: s.content ?? '', timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'tool_start') {
        steps.push({ id: `t-${s.id || steps.length}`, type: 'tool_start', toolName: s.toolName, toolArgs: s.args, timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'tool_result') {
        steps.push({ id: `tr-${s.id || steps.length}`, type: 'tool_result', toolName: s.toolName, toolResult: s.result, toolSuccess: s.success !== false, timestamp: s.ts || Date.now() / 1000 });
      } else if (s.type === 'file_diff') {
        steps.push({ id: `f-${s.id || steps.length}`, type: 'file_diff', filePath: s.filePath, fileDiff: s.fileDiff, timestamp: s.ts || Date.now() / 1000 });
      }
    }
    return steps;
  } catch {
    return [];
  }
}

/** 实时工具调用（agent-tool-* 事件）转聚合展示步骤。 */
function realtimeToolCallsToSteps(calls: GroupChatToolCall[] | undefined): ThinkingChainStep[] {
  if (!calls || calls.length === 0) return [];
  const steps: ThinkingChainStep[] = [];
  for (const c of calls) {
    steps.push({
      id: `${c.toolId}-start`,
      type: 'tool_start',
      toolName: c.toolName,
      toolArgs: c.arguments,
      timestamp: Date.now() / 1000,
    });
    if (c.status === 'done') {
      steps.push({
        id: `${c.toolId}-result`,
        type: 'tool_result',
        toolName: c.toolName,
        toolResult: c.result,
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
    if (!topic.trim()) {
      setError('请填写讨论议题');
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
          讨论议题
          <textarea
            className="px-3 py-2 rounded-lg text-xs outline-none resize-none"
            rows={2}
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            value={topic}
            onChange={(e) => setTopic(e.target.value)}
            placeholder="本次多 Agent 群聊要解决的问题"
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

        <div className="flex justify-end gap-2 px-5 py-4 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
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

/** 按 `## 已完成/未完成/失败/最终结论` 标题切分结论正文为四部分。 */
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

function ConclusionContent({ content }: { content: string }) {
  const sections = parseConclusionSections(content);
  if (sections.length === 0) return <MarkdownRenderer content={content} />;
  return (
    <div className="flex flex-col">
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
  /** 该确认请求是否已收到用户回复（数据驱动，可跨会话恢复）。 */
  responded: boolean;
  onRespond: (requestId: string, responses: GroupChatConfirmationResponseInput[]) => Promise<void>;
  /** 点击图片放大预览（定义在 GroupChatPage，经 props 传入避免模块级作用域找不到）。 */
  onPreviewImage: (src: string) => void;
}

const GroupChatMessageItem = memo(function GroupChatMessageItem({
  message,
  participant,
  color,
  isUser,
  displayName,
  responded,
  onRespond,
  onPreviewImage,
}: GroupChatMessageItemProps) {
  const atts = parseAttachments(message.attachments);
  const confirmation = message.kind === 'confirmation_request' ? parseConfirmation(message.extra) : null;
  const isConclusion = message.kind === 'conclusion';
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
            className="block w-full rounded-xl text-xs leading-relaxed break-words overflow-hidden"
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
                      onClick={() => onPreviewImage(convertFileSrc(att.path))}
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
              <span className="whitespace-pre-wrap">{message.content}</span>
            ) : isConclusion ? (
              <ConclusionContent content={message.content} />
            ) : (
              <>
                <ThinkingChain steps={parseToolCalls(message.toolCalls)} />
                <MarkdownRenderer content={message.content} />
                {confirmation && (
                  <ConfirmationCard
                    confirmation={confirmation}
                    responded={responded}
                    onSubmit={(responses) => onRespond(confirmation.requestId, responses)}
                    submittedText="已提交，等待主持人继续…"
                    openHint="需要你的决定，请直接在下方的输入框中回复。"
                  />
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

export function GroupChatPage() {
  const {
    rooms, currentRoomId, participants, messages, stances, tasks, streaming,
    currentSpeaker, currentRound, toolCalls, error,
    messagesLoading, loadingEarlier, totalMessages, loadEarlierMessages,
    loadRooms, selectRoom, sendMessage, deleteRoom, removeParticipant,
    respondConfirmation, pause, resume, abort, exportWorkflow,
  } = useGroupChatStore();

  const { getTheme } = useAgentRegistry();
  const [input, setInput] = useState('');
  const [showCreate, setShowCreate] = useState(false);
  const [showAdd, setShowAdd] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportJson, setExportJson] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [draggingFiles, setDraggingFiles] = useState<{ name: string; kind: 'image' | 'file' }[]>([]);
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionQuery, setMentionQuery] = useState('');
  const attachmentsRef = useRef<Attachment[]>([]);
  const inputBarRef = useRef<HTMLDivElement>(null);
  const inputAreaRef = useRef<HTMLTextAreaElement>(null);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  // 图片放大预览（Portal 到 body，避免被 Virtuoso transform 影响）
  const [previewImg, setPreviewImg] = useState<string | null>(null);
  const [previewScale, setPreviewScale] = useState(1);
  const [previewOffset, setPreviewOffset] = useState({ x: 0, y: 0 });
  const previewDragRef = useRef<{ startX: number; startY: number; offsetX: number; offsetY: number; moved: boolean } | null>(null);

  useEffect(() => {
    if (!previewImg) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setPreviewImg(null); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewImg]);

  // 打开图片预览（每次打开重置缩放比例与位置）
  const openImagePreview = useCallback((src: string) => {
    setPreviewScale(1);
    setPreviewOffset({ x: 0, y: 0 });
    previewDragRef.current = null;
    setPreviewImg(src);
  }, []);

  useEffect(() => {
    loadRooms();
  }, [loadRooms]);

  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  const currentRoom = rooms.find((r) => r.id === currentRoomId) ?? null;
  const isActive = currentRoom?.status === 'running' || currentRoom?.status === 'paused';

  const participantById = useMemo(() => {
    const map = new Map<string, GroupChatParticipant>();
    for (const p of participants) map.set(p.id, p);
    return map;
  }, [participants]);

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

  // 已收到用户回复的确认请求 id（由 confirmation_response.replyTo 关联 requestId），
  // 用于跨会话恢复「已提交」态。
  const respondedRequestIds = useMemo(() => {
    const set = new Set<string>();
    for (const m of messages) {
      if (m.kind === 'confirmation_response' && m.replyTo) set.add(m.replyTo);
    }
    return set;
  }, [messages]);

  const displayNameOf = useCallback(
    (id: string) => participantById.get(id)?.displayName ?? id,
    [participantById],
  );

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
      return (
        <GroupChatMessageItem
          message={m}
          participant={p}
          color={colorOf(p, index)}
          isUser={m.sender === 'user'}
          displayName={displayNameOf(m.sender)}
          responded={responded}
          onRespond={respondConfirmation}
          onPreviewImage={openImagePreview}
        />
      );
    },
    [sortedMessages, participantById, colorOf, displayNameOf, respondedRequestIds, respondConfirmation, openImagePreview],
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

  const Footer = () => {
    const streamingEl = currentSpeaker
      ? (() => {
          const streamingText = streaming[currentSpeaker] ?? '';
          const streamingSteps = realtimeToolCallsToSteps(toolCalls[currentSpeaker]);
          if (!streamingText && streamingSteps.length === 0) return null;
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
                    className="block w-full px-3 py-2 rounded-xl text-xs leading-relaxed whitespace-pre-wrap break-words"
                    style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                  >
                    <ThinkingChain steps={streamingSteps} defaultCollapsed={false} />
                    {streamingText && (
                      <>
                        {collapseBlankLines(streamingText)}
                        <span
                          className="inline-block w-1.5 h-3 ml-0.5 align-middle"
                          style={{ backgroundColor: 'var(--accent)', animation: 'pd-pulse 1.2s infinite' }}
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
      .filter(([pid, calls]) => calls.length > 0 && pid !== currentSpeaker)
      .map(([pid, calls]) => {
        const steps = realtimeToolCallsToSteps(calls);
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
                <ThinkingChain steps={steps} defaultCollapsed={false} />
              </div>
            </div>
          </div>
        );
      });

    return (
      <>
        {streamingEl}
        {toolEls}
      </>
    );
  };

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
    const content = input.trim();
    const hasAttachments = attachments.length > 0;
    if ((!content && !hasAttachments) || !currentRoomId) return;

    // 解析 @ 提及：匹配 content 中第一个可 @ 参与者（api/cli）的 displayName。
    let mention: string | null = null;
    let recipients: string[] = [];
    for (const p of participants) {
      if (p.participantType !== 'api' && p.participantType !== 'cli') continue;
      if (content.includes(`@${p.displayName}`)) {
        mention = p.id;
        recipients = [p.id];
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
      await sendMessage(content, hasAttachments ? attachments : undefined, mention, recipients);
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
      {/* ── 左：房间列表 ── */}
      <aside
        className="w-[220px] shrink-0 flex flex-col overflow-hidden"
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
          {rooms.map((r) => (
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
                  <span className="px-1 rounded shrink-0 flex items-center gap-1" style={{ color: STATUS_COLOR[r.status] }}>
                    {r.status === 'running' && (
                      <span
                        className="inline-block w-1.5 h-1.5 rounded-full shrink-0"
                        style={{ backgroundColor: STATUS_COLOR[r.status], animation: 'pd-breathe 1.2s ease-in-out infinite' }}
                      />
                    )}
                    {STATUS_LABEL[r.status]}
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
          ))}
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
          <div className="flex-1" />
          {currentRoom && isActive && (
            <>
              <button
                onClick={() => (currentRoom.status === 'running' ? pause() : resume())}
                className="pd-btn pd-btn-sm"
                style={{ color: 'var(--text-secondary)' }}
                title={currentRoom.status === 'running' ? '暂停讨论' : '继续讨论'}
              >
                {currentRoom.status === 'running' ? <Pause size={11} /> : <Play size={11} />}
                {currentRoom.status === 'running' ? '暂停' : '继续'}
              </button>
              <button
                onClick={() => abort()}
                className="pd-btn pd-btn-sm"
                style={{ color: 'var(--status-danger)' }}
                title="结束讨论"
              >
                <Square size={11} />
                结束
              </button>
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
          {messagesLoading ? (
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
              components={{ Header, Footer }}
            />
          )}
        </div>

        <div className="shrink-0 px-4 py-3" style={{ borderTop: '1px solid var(--border)' }} ref={inputBarRef}>
          {/* 附件预览：图片缩略图 + 文件 chip */}
          {(attachments.length > 0 || dragActive) && (
            <div
              className="flex flex-wrap gap-2 mb-2 p-2 rounded-lg transition-colors"
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

          <div className="relative flex items-center gap-2">
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
            <button
              onClick={handlePickImages}
              className="pd-btn pd-btn-sm shrink-0 self-stretch"
              style={{ color: attachments.some((a) => a.kind === 'image') ? 'var(--accent)' : 'var(--text-secondary)' }}
              title="添加图片（支持拖拽/粘贴，最多 8 个）"
            >
              <ImagePlus size={12} />
              图片
            </button>
            <button
              onClick={handlePickFiles}
              className="pd-btn pd-btn-sm shrink-0 self-stretch"
              style={{ color: attachments.some((a) => a.kind === 'file') ? 'var(--accent)' : 'var(--text-secondary)' }}
              title="添加文件（支持拖拽/粘贴，最多 8 个）"
            >
              <FileText size={12} />
              文件
            </button>
            <textarea
              ref={inputAreaRef}
              rows={1}
              value={input}
              onChange={handleInputChange}
              onPaste={handlePaste}
              placeholder={currentRoomId ? '输入发言，启动或继续讨论…（Shift+Enter 换行）' : '请先选择或创建一个房间'}
              disabled={!currentRoomId}
              onInput={(e) => {
                const el = e.currentTarget;
                el.style.height = 'auto';
                el.style.height = Math.min(el.scrollHeight, 120) + 'px';
              }}
              className="flex-1 px-3 py-2 rounded-lg text-xs outline-none resize-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
                minHeight: 30,
                maxHeight: 120,
                overflowY: 'auto',
              }}
              onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSend(); } }}
            />
            <button
              onClick={handleSend}
              disabled={!currentRoomId || (!input.trim() && attachments.length === 0)}
              className="pd-btn pd-btn-sm pd-btn-primary shrink-0 self-stretch"
              title="发送"
            >
              <Send size={12} />
              发送
            </button>
          </div>
        </div>
      </main>

      {/* ── 右：参与者 + 立场 + 任务 ── */}
      <aside
        className="w-[240px] shrink-0 flex flex-col overflow-hidden"
        style={{ borderLeft: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        <div className="px-3 h-10 text-[10px] font-medium flex items-center justify-between" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
          <span>参与者 ({participants.length})</span>
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
        <div className="p-2 grid grid-cols-3 gap-1.5" style={{ backgroundColor: 'var(--bg-primary)', borderBottom: '1px solid var(--border)' }}>
          {participants.length === 0 && (
            <div className="col-span-3 px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
              暂无参与者
            </div>
          )}
          {participants.map((p, idx) => {
            const color = colorOf(p, idx);
            const removable = p.participantType === 'api' || p.participantType === 'cli';
            const isSpeaking = p.id === currentSpeaker || executingIds.has(p.id);
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

        <div className="px-3 py-2 text-[10px] font-medium flex items-center gap-1" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
          <GitBranch size={10} style={{ color: 'var(--accent)' }} />
          立场快照
        </div>
        <div className="p-2 space-y-1.5 max-h-[200px] overflow-y-auto shrink-0" style={{ backgroundColor: 'var(--bg-primary)', borderBottom: '1px solid var(--border)' }}>
          {stances.length === 0 && (
            <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
              暂无立场
            </div>
          )}
          {stances.map((s, idx) => {
            const p = participantById.get(s.participantId);
            const color = colorOf(p, idx);
            const agree = /支持|同意|赞成/.test(s.stance);
            const disagree = /反对|质疑|拒绝/.test(s.stance);
            const icon = agree ? <CheckCircle size={11} style={{ color: 'var(--status-success)' }} />
              : disagree ? <XCircle size={11} style={{ color: 'var(--status-danger)' }} />
                : <AlertCircle size={11} style={{ color: 'var(--text-tertiary)' }} />;
            return (
              <div key={s.participantId} className="px-2 py-1.5 rounded-lg" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                <div className="flex items-center gap-1.5">
                  <span className="shrink-0">{icon}</span>
                  <div className="text-[11px] truncate" style={{ color }}>{p?.displayName ?? s.participantId}</div>
                </div>
                <div className="mt-0.5 text-[10px] leading-snug break-words" style={{ color: 'var(--text-primary)' }}>{s.stance}</div>
              </div>
            );
          })}
        </div>

        <div className="px-3 py-2 text-[10px] font-medium flex items-center gap-1" style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}>
          <Activity size={10} style={{ color: 'var(--accent)' }} />
          任务
        </div>
        <div className="flex-1 min-h-[160px] overflow-y-auto p-2 space-y-1.5" style={{ backgroundColor: 'var(--bg-primary)' }}>
          {tasks.length === 0 && (
            <div className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
              暂无任务
            </div>
          )}
          {tasks.map((t) => (
            <div key={t.id} className="px-2 py-1.5 rounded-lg text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}>
              <div className="flex items-start gap-1.5">
                <span className="font-medium shrink-0" style={{ color: 'var(--text-secondary)' }}>T{t.taskNo}</span>
                <span className="flex-1 break-words">{t.description}</span>
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
          ))}
        </div>
      </aside>

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

      {/* 图片放大预览：Portal 到 body，支持缩放/拖拽/Escape 关闭 */}
      {previewImg && createPortal(
        <div
          className="fixed inset-0 z-[100] flex items-center justify-center p-6 cursor-zoom-out select-none"
          style={{ backgroundColor: 'rgba(0,0,0,0.85)' }}
          onClick={() => {
            if (previewDragRef.current?.moved) {
              previewDragRef.current.moved = false;
              return;
            }
            setPreviewImg(null);
          }}
          onWheel={(e) => {
            e.preventDefault();
            e.stopPropagation();
            const factor = Math.exp(-e.deltaY * 0.002);
            setPreviewScale((s) => Math.min(6, Math.max(0.25, s * factor)));
          }}
          onMouseMove={(e) => {
            const d = previewDragRef.current;
            if (!d) return;
            const dx = e.clientX - d.startX;
            const dy = e.clientY - d.startY;
            if (!d.moved && Math.hypot(dx, dy) > 3) d.moved = true;
            if (d.moved) setPreviewOffset({ x: d.offsetX + dx, y: d.offsetY + dy });
          }}
          onMouseUp={() => { previewDragRef.current = null; }}
        >
          <button
            onClick={() => setPreviewImg(null)}
            className="absolute top-4 right-4 w-9 h-9 rounded-full flex items-center justify-center text-white/80 hover:text-white hover:bg-white/10 transition-colors"
            aria-label="关闭预览"
          >
            <X size={20} />
          </button>
          <img
            src={previewImg}
            alt="图片预览"
            draggable={false}
            className="max-w-full max-h-full object-contain rounded-lg shadow-2xl cursor-grab"
            style={{ transform: `translate(${previewOffset.x}px, ${previewOffset.y}px) scale(${previewScale})` }}
            onMouseDown={(e) => {
              if (e.button !== 0) return;
              e.preventDefault();
              e.stopPropagation();
              previewDragRef.current = { startX: e.clientX, startY: e.clientY, offsetX: previewOffset.x, offsetY: previewOffset.y, moved: false };
            }}
            onClick={(e) => e.stopPropagation()}
          />
        </div>,
        document.body
      )}
    </div>
  );
}

export default GroupChatPage;
