/**
 * 飞地一对一传讯浮窗。
 *
 * App 级常驻，仅在飞地路由可见；切换飞地子路由不会卸载。传讯以覆盖层浮在
 * 当前页面上，不参与工作区布局，也不改变窗口尺寸或触发响应式收敛。
 */

import { useEffect, useMemo, useRef, useState, type ChangeEvent } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { listen } from '@tauri-apps/api/event';
import {
  ChevronRight,
  Download,
  FileText,
  Files,
  KeyRound,
  Link2,
  ListTree,
  Loader2,
  MessageSquareText,
  Paperclip,
  Search,
  Send,
  Server,
  ShieldAlert,
  Trash2,
  Undo2,
  RotateCcw,
  X,
} from 'lucide-react';
import { confirmDialog } from '../../stores/confirmStore';
import { useEnclaveApiShareStore } from '../../stores/enclaveApiShareStore';
import { useEnclaveKbStore } from '../../stores/enclaveKbStore';
import { useEnclaveMessengerStore } from '../../stores/enclaveMessengerStore';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
import { showToast } from '../../utils/toast';
import type { ApiShareDistributionView, MessengerMessageView } from '../../types/enclave';
import { ApiShareConfigImportDialog } from './ApiShareConfigImportDialog';

type ReferenceKind = 'task' | 'kb-entry' | 'kb-file' | 'api-share';

const REFERENCE_TABS: { kind: ReferenceKind; label: string; searchHint: string }[] = [
  { kind: 'task', label: '任务', searchHint: '搜索任务标题' },
  { kind: 'kb-entry', label: '知识条目', searchHint: '搜索知识条目标题' },
  { kind: 'kb-file', label: '知识文件', searchHint: '搜索知识文件名' },
  { kind: 'api-share', label: 'API 共享', searchHint: '搜索 API 共享名称' },
];

interface ReferenceChoice {
  kind: ReferenceKind;
  id: string;
  title: string;
  kbId?: string;
}

function fuzzyMatch(value: string, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return true;
  const text = value.toLocaleLowerCase();
  if (text.includes(needle)) return true;
  let matched = 0;
  for (const char of text) {
    if (char === needle[matched]) matched += 1;
    if (matched === needle.length) return true;
  }
  return false;
}

function fmtClock(unixSec: number): string {
  const date = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function parseMeta(raw: string): Record<string, unknown> {
  try {
    const value = JSON.parse(raw);
    return value && typeof value === 'object' ? value : {};
  } catch {
    return {};
  }
}

function base64ToBytes(raw: string): Uint8Array<ArrayBuffer> {
  const binary = atob(raw);
  const out = new Uint8Array(new ArrayBuffer(binary.length));
  for (let index = 0; index < binary.length; index += 1) out[index] = binary.charCodeAt(index);
  return out;
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.includes(',') ? result.slice(result.indexOf(',') + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'));
    reader.readAsDataURL(file);
  });
}

function Avatar({ name, online }: { name: string; online: boolean }) {
  return (
    <span className="enclave-messenger-avatar" aria-hidden="true">
      {name.slice(0, 1).toUpperCase()}
      <i data-online={online} />
    </span>
  );
}

function ReferenceIcon({ kind }: { kind: ReferenceKind }) {
  if (kind === 'task') return <ListTree size={11} />;
  if (kind === 'api-share') return <KeyRound size={11} />;
  if (kind === 'kb-file') return <Files size={11} />;
  return <FileText size={11} />;
}

export function EnclaveMessenger() {
  const location = useLocation();
  const navigate = useNavigate();
  const isEnclaveRoute = location.pathname === '/enclave' || location.pathname.startsWith('/enclave/');

  const {
    open,
    orgId,
    peerId,
    members,
    messages,
    settings,
    loadingMembers,
    loadingMessages,
    sending,
    error,
    setOpen,
    selectPeer,
    loadMembers,
    loadThread,
    setRetention,
    send,
    sendFile,
    getFile,
    recall,
    clearThread,
  } = useEnclaveMessengerStore();

  const orgs = useEnclaveOrgStore((state) => state.orgs);
  const detail = useEnclaveOrgStore((state) => state.detail);
  const caps = useEnclaveOrgStore((state) => state.caps);
  const myRole = orgs.find((item) => item.orgId === orgId)?.myRole ?? detail?.org?.myRole ?? caps?.role ?? '';
  const canSetRetention = myRole === 'owner' || myRole === 'admin';
  const myId = caps?.identity?.userId ?? '';

  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState('');
  const [sensitive, setSensitive] = useState(false);
  const [refOpen, setRefOpen] = useState(false);
  const [refKind, setRefKind] = useState<ReferenceKind>('task');
  const [refQuery, setRefQuery] = useState('');
  const [pendingReference, setPendingReference] = useState<ReferenceChoice | null>(null);
  const [loadingReferences, setLoadingReferences] = useState(false);
  const [messageBusyId, setMessageBusyId] = useState('');
  const [importShare, setImportShare] = useState<Pick<
    ApiShareDistributionView,
    'id' | 'kind' | 'sourceName' | 'models' | 'expiresAt'
  > | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const threadEndRef = useRef<HTMLDivElement | null>(null);

  const visible = isEnclaveRoute && open;

  const tasks = useEnclaveTaskStore((state) => state.tasks);
  const kbEntries = useEnclaveKbStore((state) => state.entries);
  const kbFiles = useEnclaveKbStore((state) => state.files);
  const apiSources = useEnclaveApiShareStore((state) => state.sources);

  const referenceItems = useMemo<ReferenceChoice[]>(() => {
    let items: ReferenceChoice[];
    if (refKind === 'task') {
      items = tasks.map((task) => ({ kind: 'task', id: task.id, title: task.title }));
    } else if (refKind === 'kb-entry') {
      items = kbEntries.map((entry) => ({
        kind: 'kb-entry',
        id: entry.memoryId,
        title: entry.title,
        kbId: entry.kbId,
      }));
    } else if (refKind === 'kb-file') {
      items = kbFiles.map((file) => ({
        kind: 'kb-file',
        id: file.fileId,
        title: file.fileName,
        kbId: file.kbId,
      }));
    } else {
      items = apiSources
        .filter((source) => source.enabled)
        .map((source) => ({
          kind: 'api-share',
          id: source.id,
          title: source.name,
        }));
    }
    return items.filter((item) => fuzzyMatch(item.title, refQuery)).slice(0, 80);
  }, [apiSources, kbEntries, kbFiles, refKind, refQuery, tasks]);

  const peer = members.find((item) => item.userId === peerId) ?? null;
  const filteredMembers = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return members;
    return members.filter((item) => (
      item.displayName.toLowerCase().includes(needle)
      || item.email.toLowerCase().includes(needle)
    ));
  }, [members, query]);

  useEffect(() => {
    if (!visible) return;
    void loadMembers(orgId);
    if (peerId) void loadThread(orgId, peerId);
  }, [loadMembers, loadThread, orgId, peerId, visible]);

  useEffect(() => {
    if (!visible || !orgId) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<{ orgId?: string; topic?: string }>('enclave:changed', (event) => {
      if (event.payload?.orgId !== orgId || event.payload?.topic !== 'im') return;
      void loadMembers(orgId);
      if (peerId) void loadThread(orgId, peerId);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadMembers, loadThread, orgId, peerId, visible]);

  useEffect(() => {
    if (!refOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setRefOpen(false);
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [refOpen]);

  useEffect(() => {
    if (!visible) return;
    window.requestAnimationFrame(() => {
      threadEndRef.current?.scrollIntoView({ block: 'end' });
    });
  }, [messages, peerId, visible]);

  const loadReferences = async (kind: ReferenceKind) => {
    if (!orgId) return;
    setRefKind(kind);
    setRefOpen(true);
    setLoadingReferences(true);
    try {
      if (kind === 'task') {
        await useEnclaveTaskStore.getState().loadTasks(orgId);
      } else if (kind === 'api-share') {
        await useEnclaveApiShareStore.getState().load(orgId, false);
      } else {
        const kb = useEnclaveKbStore.getState();
        if (!kb.activeKbId) await kb.loadKbs(orgId);
        const activeKbId = useEnclaveKbStore.getState().activeKbId;
        if (kind === 'kb-file' && activeKbId) {
          await useEnclaveKbStore.getState().loadFiles(orgId, activeKbId);
        } else {
          await useEnclaveKbStore.getState().search(orgId, useEnclaveKbStore.getState().query);
        }
      }
    } finally {
      setLoadingReferences(false);
    }
  };

  const chooseReference = (choice: ReferenceChoice) => {
    setPendingReference(choice);
    setRefOpen(false);
  };

  const submit = async () => {
    if (!orgId || !peerId) return;
    if (!draft.trim() && !pendingReference) return;
    const metadata = pendingReference
      ? JSON.stringify({
          refType: pendingReference.kind,
          refId: pendingReference.id,
          ...(pendingReference.kbId ? { kbId: pendingReference.kbId } : {}),
        })
      : undefined;
    const result = await send(orgId, {
      peerId,
      kind: pendingReference ? 'reference' : 'text',
      content: draft.trim(),
      metadata,
      sensitive,
    });
    if (result) {
      setDraft('');
      setPendingReference(null);
      setSensitive(false);
    } else {
      showToast(useEnclaveMessengerStore.getState().error ?? '发送失败', 'error');
    }
  };

  const handleFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file || !orgId || !peerId) return;
    if (file.size > (settings?.maxFileBytes ?? 20 * 1024 * 1024)) {
      showToast('在线直传文件不能超过 20 MB', 'error');
      return;
    }
    try {
      const data = await fileToBase64(file);
      const result = await sendFile(orgId, peerId, file.name, file.type || 'application/octet-stream', data);
      if (!result) showToast(useEnclaveMessengerStore.getState().error ?? '发送文件失败', 'error');
    } catch {
      showToast('读取文件失败', 'error');
    }
  };

  const downloadFile = async (fileId: string, fallbackName: string) => {
    const file = await getFile(orgId, fileId);
    if (!file) {
      showToast(useEnclaveMessengerStore.getState().error ?? '文件已过期或下线', 'error');
      return;
    }
    const bytes = base64ToBytes(file.dataBase64);
    const url = URL.createObjectURL(new Blob([bytes], { type: file.mime || 'application/octet-stream' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = file.fileName || fallbackName;
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
  };

  const openReference = (message: MessengerMessageView) => {
    const meta = parseMeta(message.metadata);
    const kind = String(meta.refType ?? '');
    if (kind === 'task') {
      useEnclaveTaskStore.getState().selectTask(String(meta.refId ?? ''));
      navigate('/enclave');
    } else if (kind === 'kb-entry' || kind === 'kb-file') {
      navigate('/enclave/kb');
    } else if (kind === 'api-share') {
      navigate('/enclave/api-share');
    }
  };

  const importSharedConnection = (message: MessengerMessageView) => {
    const meta = parseMeta(message.metadata);
    const distributionId = String(meta.distributionId ?? '');
    const kind = String(meta.kind ?? '');
    if (
      !distributionId
      || (kind !== 'route_token' && kind !== 'provider_config')
      || !message.referenceAvailable
    ) {
      showToast('该共享连接已撤回、过期或来源已停用', 'warning');
      return;
    }
    setImportShare({
      id: distributionId,
      kind,
      sourceName: String(meta.sourceName ?? message.referenceTitle ?? '共享连接'),
      models: Array.isArray(meta.models)
        ? meta.models.filter((model): model is string => typeof model === 'string')
        : [],
      expiresAt: meta.expiresAt === null ? null : Number(meta.expiresAt ?? 0) || null,
    });
  };

  const handleRecall = async (message: MessengerMessageView) => {
    if (!peerId || !myId) return;
    const warning = message.kind === 'file'
      ? '消息会显示为“已撤回”，原在线文件立即失效。对方已下载的副本不会被收回。'
      : message.kind === 'api-share-distribution'
        ? '消息会显示为“已撤回”；已领取的凭据不会失效，请同时在分发记录中撤销。'
        : '消息会显示为“已撤回”。对方已经看到的副本不会被收回。';
    const ok = await confirmDialog({
      title: '撤回消息',
      message: warning,
      confirmText: '撤回',
      danger: true,
    });
    if (!ok) return;

    setMessageBusyId(message.id);
    try {
      const recalled = await recall(orgId, peerId, message.id);
      if (recalled) showToast('消息已撤回，可重新编辑后发送', 'success');
    } finally {
      setMessageBusyId('');
    }
  };

  const reuseRecalled = async (message: MessengerMessageView) => {
    if (message.kind === 'file') {
      fileInputRef.current?.click();
      showToast('请重新选择要发送的文件', 'info');
      return;
    }
    if (message.kind === 'api-share-distribution') {
      setMessageBusyId(message.id);
      try {
        const sent = await send(orgId, {
          peerId,
          kind: message.kind,
          content: message.content,
          metadata: message.metadata,
          sensitive: message.sensitive,
        });
        if (sent) {
          showToast('共享连接卡片已重新发送', 'success');
        }
      } finally {
        setMessageBusyId('');
      }
      return;
    }

    setDraft(message.content);
    setSensitive(message.sensitive);
    setRefOpen(false);
    if (message.kind === 'reference') {
      const meta = parseMeta(message.metadata);
      const kind = String(meta.refType ?? '') as ReferenceKind;
      if (REFERENCE_TABS.some((tab) => tab.kind === kind) && meta.refId) {
        setPendingReference({
          kind,
          id: String(meta.refId),
          title: message.referenceTitle || '原引用',
          kbId: meta.kbId ? String(meta.kbId) : undefined,
        });
      }
    } else {
      setPendingReference(null);
    }
    window.requestAnimationFrame(() => textareaRef.current?.focus());
    showToast('已恢复到输入框，可修改后重新发送', 'success');
  };

  const handleClear = async () => {
    if (!peerId) return;
    const ok = await confirmDialog({
      title: '清空传讯记录',
      message: `将清空你与「${peer?.displayName || peerId}」在保留期内的全部传讯。此操作不可撤销。`,
      confirmText: '清空',
      danger: true,
    });
    if (ok) {
      const success = await clearThread(orgId, peerId);
      if (success) showToast('传讯记录已清空', 'success');
    }
  };

  if (!isEnclaveRoute) return null;

  return (
    <>
      <aside
      className="enclave-messenger-drawer"
      data-open={visible}
      aria-hidden={!visible}
      aria-label="团队传讯"
    >
      <header className="enclave-messenger-head">
        <span className="enclave-messenger-head-icon"><MessageSquareText size={15} /></span>
        <div className="min-w-0 flex-1">
          <strong>传讯</strong>
          <span>发送消息、文件和工作引用</span>
        </div>
        {canSetRetention && settings && (
          <select
            className="enclave-messenger-retention"
            value={settings.retentionDays}
            title="消息保留期"
            aria-label="消息保留期"
            onChange={(event) => void setRetention(orgId, Number(event.target.value) as 7 | 30 | 90)}
          >
            <option value={7}>保留 7 天</option>
            <option value={30}>保留 30 天</option>
            <option value={90}>保留 90 天</option>
          </select>
        )}
        <button
          type="button"
          className="enclave-messenger-icon"
          title="关闭传讯"
          aria-label="关闭传讯"
          onClick={() => setOpen(false)}
        >
          <X size={14} />
        </button>
      </header>

      <div className="enclave-messenger-body">
        <section className="enclave-messenger-members">
          <label className="enclave-messenger-search">
            <Search size={12} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="查找成员"
            />
          </label>
          <div className="enclave-messenger-member-list">
            {loadingMembers && members.length === 0 && (
              <div className="enclave-messenger-loading"><Loader2 size={13} className="animate-spin" />读取成员</div>
            )}
            {!loadingMembers && filteredMembers.length === 0 && (
              <div className="enclave-messenger-list-empty">没有可传讯的成员</div>
            )}
            {filteredMembers.map((member) => (
              <button
                key={member.userId}
                type="button"
                className="enclave-messenger-member"
                data-active={member.userId === peerId}
                onClick={() => void selectPeer(member.userId)}
                title={`${member.displayName}${member.email ? ` · ${member.email}` : ''}`}
              >
                <Avatar name={member.displayName || member.email || member.userId} online={member.online} />
                <span className="enclave-messenger-member-copy">
                  <strong>{member.displayName || member.email || member.userId}</strong>
                  <small>{member.lastMessagePreview || (member.online ? '在线' : '离线')}</small>
                </span>
                {member.lastMessageAt > 0 && <ChevronRight size={11} />}
              </button>
            ))}
          </div>
        </section>

        <section className="enclave-messenger-thread">
          {!peerId ? (
            <div className="enclave-messenger-welcome">
              <span><MessageSquareText size={22} /></span>
              <strong>选择一位成员开始传讯</strong>
              <p>可发送文本、任务引用、知识库引用和在线小文件。消息按组织保留期自动过期。</p>
            </div>
          ) : (
            <>
              <header className="enclave-messenger-thread-head">
                <Avatar name={peer?.displayName || peerId} online={!!peer?.online} />
                <div className="min-w-0 flex-1">
                  <strong className="truncate">{peer?.displayName || peerId}</strong>
                  <span>{peer?.online ? '在线' : '离线'} · {peer?.role || '成员'}</span>
                </div>
                <button type="button" className="enclave-messenger-icon" title="清空记录" onClick={() => void handleClear()}>
                  <Trash2 size={13} />
                </button>
              </header>

              <div className="enclave-messenger-message-list pd-scroll-stable">
                {loadingMessages && messages.length === 0 && (
                  <div className="enclave-messenger-loading"><Loader2 size={13} className="animate-spin" />读取消息</div>
                )}
                {!loadingMessages && messages.length === 0 && (
                  <div className="enclave-messenger-list-empty">还没有消息，从下方开始。</div>
                )}
                {messages.map((message) => {
                  const mine = message.fromUserId === myId;
                  const meta = parseMeta(message.metadata);
                  const isFile = message.kind === 'file';
                  const isApiShareDistribution = message.kind === 'api-share-distribution';
                  const fileId = String(meta.fileId ?? '');
                  const fileName = String(meta.fileName ?? '在线文件');
                  const fileSize = Number(meta.size ?? 0);
                  const distributionKind = meta.kind === 'route_token' || meta.kind === 'provider_config'
                    ? meta.kind
                    : null;
                  const distributionSourceName = String(meta.sourceName ?? message.referenceTitle ?? '共享连接');
                  const distributionExpiresAt = Number(meta.expiresAt ?? 0);
                  const distributionModels = Array.isArray(meta.models)
                    ? meta.models.filter((model): model is string => typeof model === 'string')
                    : [];
                  const distributionAvailable = !!distributionKind && message.referenceAvailable;
                  const routeDistribution = distributionKind === 'route_token';
                  return (
                    <div
                      key={message.id}
                      className="enclave-messenger-message"
                      data-mine={mine}
                      data-recalled={!!message.recalledAt}
                    >
                      <div className="enclave-messenger-bubble">
                        {message.recalledAt ? (
                          <div className="enclave-messenger-recalled">
                            <span><Undo2 size={12} />消息已撤回</span>
                            {mine && (
                              <button
                                type="button"
                                disabled={messageBusyId === message.id}
                                onClick={() => void reuseRecalled(message)}
                              >
                                {messageBusyId === message.id
                                  ? <Loader2 size={10} className="animate-spin" />
                                  : <RotateCcw size={10} />}
                                {message.kind === 'file'
                                  ? '重新选择'
                                  : message.kind === 'api-share-distribution'
                                    ? '重新发送'
                                    : '再次编辑'}
                              </button>
                            )}
                          </div>
                        ) : (
                          <>
                        {message.kind === 'reference' ? (
                          <button
                            type="button"
                            className="enclave-messenger-reference"
                            onClick={() => openReference(message)}
                          >
                            <span><ReferenceIcon kind={String(meta.refType ?? '') as ReferenceKind} /></span>
                            <div className="min-w-0 flex-1">
                              <small>{String(meta.refType ?? '引用')}</small>
                              <strong className="truncate">
                                {message.referenceAvailable ? message.referenceTitle : '引用已删除或无权访问'}
                              </strong>
                            </div>
                            <ChevronRight size={12} />
                          </button>
                        ) : isFile ? (
                          <button
                            type="button"
                            className="enclave-messenger-file"
                            onClick={() => void downloadFile(fileId, fileName)}
                          >
                            <span><Download size={14} /></span>
                            <div className="min-w-0 flex-1">
                              <strong className="truncate">{fileName}</strong>
                              <small>
                                {fileSize > 0 ? `${(fileSize / 1024 / 1024).toFixed(2)} MB · ` : ''}
                                在线文件，10 分钟后失效
                              </small>
                            </div>
                          </button>
                        ) : isApiShareDistribution ? (
                          <div className="enclave-messenger-share-card">
                            <div className="enclave-messenger-share-head">
                              <span data-kind={distributionKind ?? 'provider_config'}>
                                {routeDistribution ? <KeyRound size={14} /> : <Server size={14} />}
                              </span>
                              <div className="min-w-0 flex-1">
                                <small>{routeDistribution ? '路由令牌' : 'Provider 配置'}</small>
                                <strong className="truncate" title={distributionSourceName || message.content}>
                                  {distributionSourceName || message.content}
                                </strong>
                              </div>
                            </div>
                            <div className="enclave-messenger-share-models">
                              {distributionModels.length > 0
                                ? distributionModels.map((model) => (
                                    <code key={model} title={model}>{model}</code>
                                  ))
                                : <span>模型信息未提供</span>}
                            </div>
                            <div className="enclave-messenger-share-foot">
                              <span>
                                {!distributionAvailable
                                  ? '连接已撤回、过期或来源已停用'
                                  : distributionExpiresAt > 0
                                    ? `${mine ? '已发送 · ' : ''}有效至 ${fmtClock(distributionExpiresAt)}`
                                    : mine ? '已发送' : '长期有效'}
                              </span>
                              {!mine && (
                                <button
                                  type="button"
                                  disabled={!distributionAvailable}
                                  onClick={() => importSharedConnection(message)}
                                >
                                  {distributionAvailable ? '领取连接' : '不可领取'}
                                  <ChevronRight size={11} />
                                </button>
                              )}
                            </div>
                          </div>
                        ) : (
                          <p>{message.content}</p>
                        )}
                        {message.kind === 'reference' && message.content && (
                          <p className="enclave-messenger-reference-copy">{message.content}</p>
                        )}
                        {message.sensitive && (
                          <span className="enclave-messenger-sensitive"><ShieldAlert size={10} />含明文敏感信息</span>
                        )}
                          </>
                        )}
                      </div>
                      <div className="enclave-messenger-message-meta">
                        <time>{fmtClock(message.createdAt)}</time>
                        {mine && !message.recalledAt && (
                          <button
                            type="button"
                            disabled={messageBusyId === message.id}
                            title="撤回消息"
                            onClick={() => void handleRecall(message)}
                          >
                            {messageBusyId === message.id
                              ? <Loader2 size={9} className="animate-spin" />
                              : <Undo2 size={9} />}
                            撤回
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
                <div ref={threadEndRef} />
              </div>

              <div className="enclave-messenger-compose">
                {pendingReference && (
                  <div className="enclave-messenger-pending">
                    <ReferenceIcon kind={pendingReference.kind} />
                    <span className="truncate">{pendingReference.title}</span>
                    <button type="button" onClick={() => setPendingReference(null)} title="移除引用">
                      <X size={11} />
                    </button>
                  </div>
                )}
                {refOpen && (
                  <div className="enclave-messenger-reference-panel">
                    <div className="enclave-messenger-reference-head">
                      <div className="enclave-messenger-reference-tabs" role="tablist" aria-label="引用类型">
                        {REFERENCE_TABS.map((tab) => (
                          <button
                            key={tab.kind}
                            type="button"
                            role="tab"
                            aria-selected={refKind === tab.kind}
                            data-active={refKind === tab.kind}
                            onClick={() => void loadReferences(tab.kind)}
                          >
                            {tab.label}
                          </button>
                        ))}
                      </div>
                      <button
                        type="button"
                        className="enclave-messenger-reference-close"
                        title="关闭引用"
                        aria-label="关闭引用"
                        onClick={() => setRefOpen(false)}
                      >
                        <X size={12} />
                      </button>
                    </div>
                    <label className="enclave-messenger-reference-search">
                      <Search size={11} />
                      <input
                        value={refQuery}
                        onChange={(event) => setRefQuery(event.target.value)}
                        placeholder={REFERENCE_TABS.find((tab) => tab.kind === refKind)?.searchHint}
                        aria-label="搜索引用"
                        autoFocus
                      />
                    </label>
                    <div className="enclave-messenger-reference-list">
                      {loadingReferences ? (
                        <div className="enclave-messenger-loading"><Loader2 size={12} className="animate-spin" />读取引用</div>
                      ) : referenceItems.length === 0 ? (
                        <div className="enclave-messenger-reference-empty">
                          {refQuery.trim() ? '没有匹配结果' : '暂无可引用内容'}
                        </div>
                      ) : (
                        referenceItems.map((item) => (
                          <button
                            key={`${item.kind}:${item.kbId ?? ''}:${item.id}`}
                            type="button"
                            onClick={() => chooseReference(item)}
                          >
                            <ReferenceIcon kind={item.kind} />
                            <span className="truncate" title={item.title}>{item.title}</span>
                          </button>
                        ))
                      )}
                    </div>
                  </div>
                )}
                <textarea
                  ref={textareaRef}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  placeholder={pendingReference ? '补充说明（可选）' : '输入消息，Enter 发送，Shift + Enter 换行'}
                  rows={2}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' && !event.shiftKey) {
                      event.preventDefault();
                      void submit();
                    }
                  }}
                />
                <div className="enclave-messenger-compose-actions">
                  <button type="button" title="添加引用" onClick={() => void loadReferences(refKind)}>
                    <Link2 size={13} />
                  </button>
                  <button type="button" title="发送在线文件" onClick={() => fileInputRef.current?.click()}>
                    <Paperclip size={13} />
                  </button>
                  <button
                    type="button"
                    className="enclave-messenger-sensitive-toggle"
                    data-active={sensitive}
                    title="标记消息含明文敏感信息"
                    onClick={() => setSensitive((value) => !value)}
                  >
                    <ShieldAlert size={12} />
                    敏感
                  </button>
                  <input ref={fileInputRef} type="file" hidden onChange={(event) => void handleFile(event)} />
                  <span className="flex-1" />
                  <button
                    type="button"
                    className="enclave-messenger-send"
                    disabled={sending || (!draft.trim() && !pendingReference)}
                    title="发送"
                    onClick={() => void submit()}
                  >
                    {sending ? <Loader2 size={13} className="animate-spin" /> : <Send size={13} />}
                  </button>
                </div>
              </div>
            </>
          )}
        </section>
      </div>

      {error && <div className="enclave-messenger-error">{error}</div>}
      </aside>
      {visible && orgId && importShare && (
        <ApiShareConfigImportDialog
          orgId={orgId}
          distribution={importShare}
          onClose={() => setImportShare(null)}
          onImported={() => setImportShare(null)}
        />
      )}
    </>
  );
}
