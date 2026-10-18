import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Library,
  Search,
  Loader2,
  FileText,
  X,
  Paperclip,
  Trash2,
  Save,
  AlertTriangle,
  ArrowUpRight,
  ChevronRight,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { TitleBar, StatusBar } from '../components/layout';
import { TagEditor } from '../components/knowledge/TagEditor';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';
import { useEnclaveTaskStore } from '../stores/enclaveTaskStore';
import { useAccountStore } from '../stores/accountStore';
import type { KbEntryView } from '../types/enclave';

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 列表里用相对时间（与个人知识库一致），详情里用绝对时间。 */
function formatRelative(unixSec: number): string {
  if (!unixSec) return '-';
  const diff = Math.floor(Date.now() / 1000) - unixSec;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} 天前`;
  return formatTime(unixSec);
}

function formatSize(bytes: number): string {
  if (!bytes) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function splitTags(raw: string): string[] {
  return raw
    .split(/[,，]/)
    .map((t) => t.trim().replace(/^#/, '').trim())
    .filter(Boolean);
}

function joinTags(tags: string[]): string {
  return tags.join(',');
}

const ORIGIN_LABEL: Record<string, string> = {
  task_output: '任务产出',
  manual: '手工添加',
};

/**
 * 飞地团队知识库 —— 独立路由 `/enclave/kb`。
 *
 * 数据来自主机 `enclave.db`：任务汇报「确认」后自动沉淀为条目；
 * 检索走 FTS5 trigram（≥3 字），短词退化为子串匹配。
 *
 * 展示方式与个人知识库一致：左侧选库，右侧**列表 + 展开行**（展开行即详情），
 * 不再单独占一栏详情面板；owner / admin 在展开行内直接改标题 / 正文 / 标签。
 */
export function EnclaveKbPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const members = useEnclaveOrgStore((s) => s.detail?.members ?? []);

  const kbs = useEnclaveKbStore((s) => s.kbs);
  const entries = useEnclaveKbStore((s) => s.entries);
  const activeEntry = useEnclaveKbStore((s) => s.activeEntry);
  const query = useEnclaveKbStore((s) => s.query);
  const loading = useEnclaveKbStore((s) => s.loading);
  const error = useEnclaveKbStore((s) => s.error);

  const [keyword, setKeyword] = useState('');
  // 展开行的草稿：正文 / 标签 / 标题一起保存（与个人知识库一致）
  const [draftTitle, setDraftTitle] = useState('');
  const [draftContent, setDraftContent] = useState('');
  const [draftTags, setDraftTags] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  // 登录账号变化 → 先校验新身份，再加载组织与知识库
  const accountKey = useAccountStore((s) => s.account?.email ?? '');

  const myRole = orgs.find((o) => o.orgId === activeOrgId)?.myRole ?? '';
  const canEdit = myRole === 'owner' || myRole === 'admin';
  /** 单一默认库：直接取列表里唯一的一项 */
  const kb = kbs[0] ?? null;

  const memberName = (raw: unknown): string => {
    const id = typeof raw === 'string' ? raw : '';
    if (!id) return '';
    const m = members.find((x) => x.userId === id);
    return m?.displayName || m?.email || `${id.slice(0, 8)}…`;
  };

  const metaStr = (entry: KbEntryView | null, key: string): string => {
    const v = entry?.metadata?.[key];
    return typeof v === 'string' ? v : '';
  };
  const metaNum = (entry: KbEntryView | null, key: string): number => {
    const v = entry?.metadata?.[key];
    return typeof v === 'number' ? v : 0;
  };

  /** 展开 / 收起：同一个条目再点一次即收起。 */
  const toggleEntry = (entry: KbEntryView) => {
    const cur = useEnclaveKbStore.getState().activeEntry;
    useEnclaveKbStore.getState().openEntry(cur?.memoryId === entry.memoryId ? null : entry);
  };

  useEffect(() => {
    setDraftTitle(activeEntry?.title ?? '');
    setDraftContent(activeEntry?.content ?? '');
    setDraftTags(splitTags(activeEntry?.tags ?? ''));
  }, [activeEntry]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const st = useEnclaveOrgStore.getState();
      if (!(await st.ensureIdentity()) || cancelled) return;
      await st.loadOrgs();
      if (cancelled) return;
      const cur = useEnclaveOrgStore.getState();
      if (cur.orgs.length === 0 && !cur.lastRefresh) await cur.refresh();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountKey]);

  useEffect(() => {
    if (activeOrgId) void useEnclaveKbStore.getState().loadKbs(activeOrgId);
  }, [activeOrgId]);

  const handleModeChange = useCallback(
    (mode: ViewMode) => {
      setMode(mode);
      navigate('/');
    },
    [setMode, navigate],
  );

  const runSearch = async () => {
    if (!activeOrgId) return;
    await useEnclaveKbStore.getState().search(activeOrgId, keyword);
  };

  const openFile = async (fileId: string) => {
    if (!activeOrgId) return;
    try {
      const path = await invoke<string>('enclave_file_abs_path', { orgId: activeOrgId, fileId });
      await invoke('open_path', { path });
    } catch {
      // 单个文件打不开不影响列表；错误由 store 统一提示
    }
  };

  const saveEntry = async () => {
    if (!activeOrgId || !activeEntry || saving) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    setSaving(true);
    try {
      await useEnclaveKbStore.getState().updateEntry(activeOrgId, kbId, activeEntry.memoryId, {
        title: draftTitle,
        content: draftContent,
        tags: joinTags(draftTags),
      });
    } finally {
      setSaving(false);
    }
  };

  /** 取消：丢弃未保存的草稿并收起 —— 按钮在展开区底部，不必滚回顶部 */
  const cancelEdit = () => {
    if (activeEntry) {
      setDraftTitle(activeEntry.title);
      setDraftContent(activeEntry.content);
      setDraftTags(splitTags(activeEntry.tags));
    }
    useEnclaveKbStore.getState().openEntry(null);
  };

  const removeEntry = async () => {
    if (!activeOrgId || !activeEntry) return;
    if (!window.confirm('从团队知识库移除该条目？任务中的交付物原件不会被删除。')) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    await useEnclaveKbStore.getState().removeEntry(activeOrgId, kbId, activeEntry.memoryId);
  };

  /** 跳回来源任务：先加载任务列表再选中，避免选中态被列表刷新覆盖。 */
  const jumpToTask = async () => {
    const taskId = activeEntry?.sourceRef;
    if (!activeOrgId || !taskId) return;
    await useEnclaveTaskStore.getState().loadTasks(activeOrgId);
    useEnclaveTaskStore.getState().selectTask(taskId);
    navigate('/enclave');
  };

  const dirty =
    !!activeEntry &&
    canEdit &&
    (draftTitle.trim() !== activeEntry.title ||
      draftContent !== activeEntry.content ||
      joinTags(draftTags) !== joinTags(splitTags(activeEntry.tags)));

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        enclaveOpen
        onOpenEnclave={() => navigate('/enclave')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
        onOpenMarket={() => navigate('/market')}
        onOpenInspirations={() => navigate('/inspirations')}
      />

      <div className="flex-1 min-h-0 px-2 pt-2 flex flex-col overflow-hidden">
        <div
          className="flex-1 min-h-0 rounded-lg overflow-hidden flex flex-col"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          {/* 顶部：组织 + 搜索 */}
          <div
            className="shrink-0 h-9 px-3 flex items-center gap-2"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <Library size={13} style={{ color: 'var(--accent)' }} />
            {orgs.map((o) => (
              <button
                key={o.orgId}
                onClick={() => void useEnclaveOrgStore.getState().selectOrg(o.orgId)}
                className="text-xs px-2 py-0.5 rounded"
                style={{
                  backgroundColor: o.orgId === activeOrgId ? 'var(--accent)' : 'var(--bg-tertiary)',
                  color: o.orgId === activeOrgId ? '#fff' : 'var(--text-secondary)',
                  border: '1px solid var(--border)',
                }}
              >
                {o.name || o.orgId}
              </button>
            ))}
            <div className="flex-1" />
            <div className="relative w-[280px]">
              <Search
                size={12}
                style={{ position: 'absolute', left: 8, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-tertiary)' }}
              />
              <input
                className="search-input w-full"
                style={{ paddingLeft: 26, backgroundColor: 'var(--bg-field)' }}
                placeholder="搜索标题 / 正文 / 标签 / 交付物（回车）"
                value={keyword}
                onChange={(e) => setKeyword(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') void runSearch(); }}
              />
              {keyword && (
                <button
                  onClick={() => setKeyword('')}
                  style={{ position: 'absolute', right: 6, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-tertiary)' }}
                >
                  <X size={12} />
                </button>
              )}
            </div>
            {loading && <Loader2 size={12} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
          </div>

          {/* 单一默认库：不再占一列做库列表，库信息收进内容区顶部一行 */}
          <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
            <div
              className="shrink-0 px-4 pt-2.5 pb-2"
              style={{ borderBottom: '1px solid var(--border)' }}
            >
              <div className="flex items-center gap-2 min-w-0">
                <Library size={12} style={{ color: 'var(--accent)' }} />
                <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>
                  {kb?.name ?? '团队知识库'}
                </span>
                <span
                  className="text-[10px] shrink-0 whitespace-nowrap"
                  style={{ color: 'var(--text-tertiary)' }}
                >
                  {kb?.entryCount ?? entries.length} 条知识
                  {kb && kb.lastEntryAt > 0 && ` · 最近更新 ${formatTime(kb.lastEntryAt)}`}
                </span>
                <div className="flex-1" />
                <span
                  className="text-[10px] shrink-0 whitespace-nowrap"
                  style={{ color: 'var(--text-tertiary)' }}
                >
                  任务汇报「确认完成」后自动沉淀 · 点击条目展开
                </span>
              </div>
              <div
                className="text-[11px] mt-1 truncate"
                style={{ color: 'var(--text-secondary)' }}
                title={kb?.description}
              >
                {kb?.description || '任务产出自动沉淀；团队共享'}
              </div>
            </div>

            <div className="flex-1 overflow-y-auto">
              {entries.length === 0 && (
                <div
                  className="h-full flex flex-col items-center justify-center gap-2 text-xs"
                  style={{ color: 'var(--text-tertiary)' }}
                >
                  <Search size={18} />
                  {query ? `没有匹配「${query}」的条目` : '还没有条目：任务汇报「确认完成」后会自动沉淀到这里'}
                </div>
              )}

              {entries.map((e) => {
                  const open = activeEntry?.memoryId === e.memoryId;
                  const tags = splitTags(e.tags);
                  const taskTitle = metaStr(e, 'taskTitle');
                  const confirmedAt = metaNum(e, 'confirmedAt');
                  return (
                    <div
                      key={e.memoryId}
                      className="px-4 py-2.5 transition-colors"
                      style={{
                        borderBottom: '1px solid var(--border)',
                        backgroundColor: open ? 'var(--bg-secondary)' : 'transparent',
                      }}
                    >
                      <div className="flex items-start gap-2">
                        <button
                          onClick={() => toggleEntry(e)}
                          className="shrink-0 mt-0.5"
                          title={open ? '收起' : '展开'}
                        >
                          <ChevronRight
                            size={12}
                            className="transition-transform"
                            style={{
                              color: 'var(--text-tertiary)',
                              transform: open ? 'rotate(90deg)' : 'none',
                            }}
                          />
                        </button>
                        <div className="flex-1 min-w-0">
                          <div
                            className="flex items-center gap-1.5 min-w-0"
                            style={{ cursor: open && canEdit ? 'default' : 'pointer' }}
                            onClick={() => { if (!(open && canEdit)) toggleEntry(e); }}
                          >
                            {open && canEdit ? (
                              <input
                                className="search-input flex-1"
                                style={{ backgroundColor: 'var(--bg-field)' }}
                                value={draftTitle}
                                onChange={(ev) => setDraftTitle(ev.target.value)}
                                onClick={(ev) => ev.stopPropagation()}
                                placeholder="条目标题"
                              />
                            ) : (
                              <span
                                className="text-xs font-medium truncate"
                                style={{ color: 'var(--text-primary)' }}
                                title={e.title}
                              >
                                {e.title}
                              </span>
                            )}
                            <span
                              className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                              title={`来源：${ORIGIN_LABEL[e.origin] ?? e.origin}`}
                            >
                              {ORIGIN_LABEL[e.origin] ?? e.origin}
                            </span>
                            {e.stale && (
                              <span
                                className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                                style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                                title="来源任务已恢复汇报，等待重新确认"
                              >
                                待更新
                              </span>
                            )}
                            {e.files.length > 0 && (
                              <span
                                className="shrink-0 flex items-center gap-0.5 text-[9px]"
                                style={{ color: 'var(--text-tertiary)' }}
                                title={`${e.files.length} 个交付物`}
                              >
                                <Paperclip size={9} />
                                {e.files.length}
                              </span>
                            )}
                          </div>

                          {!open && (
                            <div className="text-[11px] mt-1 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>
                              {e.content || '（无正文）'}
                            </div>
                          )}

                          {!open && (
                            <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                              {tags.map((t) => (
                                <span
                                  key={t}
                                  className="px-1.5 py-[1px] rounded text-[9px]"
                                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                                >
                                  #{t}
                                </span>
                              ))}
                              <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                                更新于 {formatRelative(e.updatedAt)}
                              </span>
                            </div>
                          )}
                        </div>
                      </div>

                      {/* 展开：溯源 + 正文 / 标签 / 交付物 */}
                      {open && (
                        <div className="mt-2 ml-5 space-y-3">
                          {e.stale && (
                            <div
                              className="text-[11px] px-2 py-1 rounded flex items-center gap-1"
                              style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                            >
                              <AlertTriangle size={11} />
                              来源任务已恢复汇报：当前内容为上次确认版本，等待重新确认后更新
                            </div>
                          )}

                          <div
                            className="text-[10px] flex items-center gap-3 flex-wrap"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            <span>任务「{taskTitle || e.sourceRef}」</span>
                            {confirmedAt > 0 && (
                              <span>
                                确认人 {memberName(metaStr(e, 'confirmedBy')) || '-'} · 确认于 {formatTime(confirmedAt)}
                              </span>
                            )}
                            <span>第 {metaNum(e, 'revision') || 1} 次确认</span>
                            <span>首次沉淀 {formatTime(e.addedAt)}</span>
                            <span>最近更新 {formatTime(e.updatedAt)}</span>
                            {metaStr(e, 'editedBy') && (
                              <span>
                                人工编辑 {memberName(metaStr(e, 'editedBy'))} · {formatTime(metaNum(e, 'editedAt'))}
                              </span>
                            )}
                          </div>

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              正文{canEdit ? '（可编辑）' : ''}
                            </div>
                            {canEdit ? (
                              <textarea
                                value={draftContent}
                                onChange={(ev) => setDraftContent(ev.target.value)}
                                rows={Math.min(18, Math.max(6, draftContent.split('\n').length))}
                                className="w-full p-3 rounded-lg text-xs leading-relaxed outline-none resize-y"
                                style={{
                                  backgroundColor: 'var(--bg-field)',
                                  color: 'var(--text-primary)',
                                  border: '1px solid var(--border)',
                                }}
                                placeholder="知识正文"
                              />
                            ) : (
                              <div
                                className="text-xs whitespace-pre-wrap leading-6"
                                style={{ color: 'var(--text-secondary)' }}
                              >
                                {e.content || '（无正文）'}
                              </div>
                            )}
                          </div>

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              标签（用于检索）
                            </div>
                            {canEdit ? (
                              <TagEditor tags={draftTags} onChange={setDraftTags} />
                            ) : (
                              <div className="flex items-center gap-1.5 flex-wrap">
                                {tags.length === 0 && (
                                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>-</span>
                                )}
                                {tags.map((t) => (
                                  <span
                                    key={t}
                                    className="px-1.5 py-[1px] rounded text-[10px]"
                                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                                  >
                                    #{t}
                                  </span>
                                ))}
                              </div>
                            )}
                          </div>

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              交付物（{e.files.length}）
                            </div>
                            <div
                              className="rounded-md p-2 flex flex-col gap-1"
                              style={{ border: '1px dashed var(--border)' }}
                            >
                              {e.files.length === 0 && (
                                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                                  该条目没有交付物
                                </span>
                              )}
                              {e.files.map((f) => (
                                <button
                                  key={f.fileId}
                                  onClick={() => void openFile(f.fileId)}
                                  className="attachment-row w-full text-left flex items-center gap-2 px-1 py-1"
                                  title="打开文件"
                                >
                                  <FileText size={11} style={{ color: 'var(--accent)' }} />
                                  <span
                                    className="text-[11px] truncate flex-1"
                                    style={{ color: 'var(--text-secondary)' }}
                                  >
                                    {f.fileName}
                                  </span>
                                  <span
                                    className="text-[10px] shrink-0"
                                    style={{ color: 'var(--text-tertiary)' }}
                                  >
                                    {formatSize(f.fileSize)}
                                  </span>
                                </button>
                              ))}
                            </div>
                          </div>

                          <div className="flex items-center gap-2 flex-wrap">
                            {canEdit && (
                              <button
                                onClick={() => void saveEntry()}
                                disabled={saving || !dirty}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  backgroundColor: 'var(--accent)',
                                  color: '#fff',
                                  border: 'none',
                                  opacity: saving || !dirty ? 0.5 : 1,
                                  cursor: saving || !dirty ? 'not-allowed' : 'pointer',
                                }}
                                title={dirty ? '保存标题 / 正文 / 标签' : '没有改动'}
                              >
                                <Save size={11} />
                                {saving ? '保存中…' : dirty ? '保存' : '已保存'}
                              </button>
                            )}
                            {canEdit && (
                              <button
                                onClick={cancelEdit}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--text-secondary)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="放弃未保存的改动并收起（不必滚回顶部）"
                              >
                                <X size={11} />
                                取消
                              </button>
                            )}
                            {e.sourceRef && (
                              <button
                                onClick={() => void jumpToTask()}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--text-secondary)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="跳转到产出该知识的任务"
                              >
                                <ArrowUpRight size={11} />
                                查看来源任务
                              </button>
                            )}
                            {canEdit && (
                              <button
                                onClick={() => void removeEntry()}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--status-danger)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="从团队知识库移除（交付物原件仍保留在任务中）"
                              >
                                <Trash2 size={11} />
                                移除
                              </button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>

          {error && (
            <div className="shrink-0 px-3 py-1 text-[11px]" style={{ color: 'var(--status-danger)', borderTop: '1px solid var(--border)' }}>
              {error}
            </div>
          )}
        </div>
      </div>

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
