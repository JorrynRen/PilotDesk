import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Library,
  Search,
  Loader2,
  FileText,
  X,
  Paperclip,
  Pencil,
  Trash2,
  Check,
  AlertTriangle,
  ArrowUpRight,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';
import { useEnclaveTaskStore } from '../stores/enclaveTaskStore';
import { useAccountStore } from '../stores/accountStore';

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function formatSize(bytes: number): string {
  if (!bytes) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * 飞地团队知识库 —— 独立路由 `/enclave/kb`。
 *
 * 数据来自主机 `enclave.db`：任务汇报「确认」后自动沉淀为条目；
 * 检索走 FTS5 trigram（≥3 字），短词退化为子串匹配。
 */
export function EnclaveKbPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);

  const kbs = useEnclaveKbStore((s) => s.kbs);
  const activeKbId = useEnclaveKbStore((s) => s.activeKbId);
  const entries = useEnclaveKbStore((s) => s.entries);
  const activeEntry = useEnclaveKbStore((s) => s.activeEntry);
  const query = useEnclaveKbStore((s) => s.query);
  const loading = useEnclaveKbStore((s) => s.loading);
  const error = useEnclaveKbStore((s) => s.error);
  const [keyword, setKeyword] = useState('');
  // 条目人工维护（owner / admin）
  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState('');
  const [draftContent, setDraftContent] = useState('');
  const [draftTags, setDraftTags] = useState('');
  // 登录账号变化 → 先校验新身份，再加载组织与知识库
  const accountKey = useAccountStore((s) => s.account?.email ?? '');

  const members = useEnclaveOrgStore((s) => s.detail?.members ?? []);
  const myRole =
    orgs.find((o) => o.orgId === activeOrgId)?.myRole ?? '';
  const canEdit = myRole === 'owner' || myRole === 'admin';

  const memberName = (raw: unknown): string => {
    const id = typeof raw === 'string' ? raw : '';
    if (!id) return '';
    const m = members.find((x) => x.userId === id);
    return m?.displayName || m?.email || `${id.slice(0, 8)}…`;
  };

  const metaStr = (entry: typeof activeEntry, key: string): string => {
    const v = entry?.metadata?.[key];
    return typeof v === 'string' ? v : '';
  };
  const metaNum = (entry: typeof activeEntry, key: string): number => {
    const v = entry?.metadata?.[key];
    return typeof v === 'number' ? v : 0;
  };

  useEffect(() => {
    setEditing(false);
    setDraftTitle(activeEntry?.title ?? '');
    setDraftContent(activeEntry?.content ?? '');
    setDraftTags(activeEntry?.tags ?? '');
  }, [activeEntry]);

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
    if (!activeOrgId || !activeEntry) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    const ok = await useEnclaveKbStore.getState().updateEntry(activeOrgId, kbId, activeEntry.memoryId, {
      title: draftTitle,
      content: draftContent,
      tags: draftTags,
    });
    if (ok) setEditing(false);
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
            <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>团队知识库</span>
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
                placeholder="搜索标题 / 内容 / 标签（回车）"
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

          <div className="flex-1 min-h-0 flex overflow-hidden">
            {/* 左：知识库列表 */}
            <div className="w-[240px] shrink-0 overflow-auto py-1" style={{ borderRight: '1px solid var(--border)' }}>
              {kbs.length === 0 && (
                <div className="px-3 py-4 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                  暂无知识库
                </div>
              )}
              {kbs.map((kb) => (
                <button
                  key={kb.id}
                  onClick={() => activeOrgId && void useEnclaveKbStore.getState().selectKb(activeOrgId, kb.id)}
                  className="w-full text-left px-3 py-2 flex flex-col gap-0.5"
                  style={{ backgroundColor: kb.id === activeKbId ? 'var(--bg-tertiary)' : 'transparent' }}
                >
                  <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>{kb.name}</span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {kb.entryCount} 条 · {kb.isImplicit ? '自动归档' : '自定义'}
                  </span>
                </button>
              ))}
            </div>

            {/* 中：条目列表 */}
            <div className="w-[320px] shrink-0 overflow-auto" style={{ borderRight: '1px solid var(--border)' }}>
              {entries.length === 0 && (
                <div className="px-3 py-6 text-[11px] leading-5" style={{ color: 'var(--text-tertiary)' }}>
                  {query ? `没有匹配「${query}」的条目` : '还没有条目：任务汇报「确认完成」后会自动沉淀到这里'}
                </div>
              )}
              {entries.map((e) => (
                <button
                  key={e.memoryId}
                  onClick={() => useEnclaveKbStore.getState().openEntry(e)}
                  className="w-full text-left px-3 py-2 flex flex-col gap-1"
                  style={{
                    borderBottom: '1px solid var(--border)',
                    backgroundColor: activeEntry?.memoryId === e.memoryId ? 'var(--bg-tertiary)' : 'transparent',
                  }}
                >
                  <span className="flex items-center gap-1.5 min-w-0">
                    <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>{e.title}</span>
                    {e.stale && (
                      <span
                        className="text-[10px] px-1 rounded shrink-0"
                        style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                      >
                        待更新
                      </span>
                    )}
                  </span>
                  <span className="text-[10px] line-clamp-2" style={{ color: 'var(--text-tertiary)' }}>
                    {e.content || '（无正文）'}
                  </span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {formatTime(e.updatedAt)}
                    {e.files.length > 0 && ` · ${e.files.length} 个交付物`}
                  </span>
                </button>
              ))}
            </div>

            {/* 右：条目详情 */}
            <div className="flex-1 min-w-0 overflow-auto p-4">
              {!activeEntry && (
                <div className="h-full flex items-center justify-center text-xs" style={{ color: 'var(--text-tertiary)' }}>
                  选择左侧条目查看内容
                </div>
              )}
              {activeEntry && (
                <div className="flex flex-col gap-3">
                  {/* 标题 + 维护操作 */}
                  <div className="flex items-start gap-2">
                    <FileText size={13} style={{ color: 'var(--accent)', marginTop: 2 }} />
                    {editing ? (
                      <input
                        className="search-input flex-1"
                        style={{ backgroundColor: 'var(--bg-field)' }}
                        value={draftTitle}
                        onChange={(e) => setDraftTitle(e.target.value)}
                        placeholder="条目标题"
                        autoFocus
                      />
                    ) : (
                      <span className="text-sm font-medium flex-1" style={{ color: 'var(--text-primary)' }}>
                        {activeEntry.title}
                      </span>
                    )}
                    {canEdit && !editing && (
                      <div className="flex items-center gap-1 shrink-0">
                        <button
                          onClick={() => setEditing(true)}
                          className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
                          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                          title="编辑条目（owner / admin）"
                        >
                          <Pencil size={11} /> 编辑
                        </button>
                        <button
                          onClick={() => void removeEntry()}
                          className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
                          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--status-danger)', border: '1px solid var(--border)' }}
                          title="从团队知识库移除（交付物原件仍保留在任务中）"
                        >
                          <Trash2 size={11} /> 移除
                        </button>
                      </div>
                    )}
                    {editing && (
                      <div className="flex items-center gap-1 shrink-0">
                        <button
                          onClick={() => void saveEntry()}
                          className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
                          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                        >
                          <Check size={11} /> 保存
                        </button>
                        <button
                          onClick={() => {
                            setEditing(false);
                            setDraftTitle(activeEntry.title);
                            setDraftContent(activeEntry.content);
                            setDraftTags(activeEntry.tags);
                          }}
                          className="text-[11px] px-1.5 py-0.5 rounded"
                          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                        >
                          取消
                        </button>
                      </div>
                    )}
                  </div>

                  {/* 待更新提示：来源任务已恢复汇报 */}
                  {activeEntry.stale && (
                    <div
                      className="text-[11px] px-2 py-1 rounded flex items-center gap-1"
                      style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                    >
                      <AlertTriangle size={11} />
                      来源任务已恢复汇报：当前内容为上次确认版本，等待重新确认后更新
                    </div>
                  )}

                  {/* 溯源信息 */}
                  <div className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
                    <div>
                      来源：{activeEntry.origin === 'task_output' ? '任务产出' : activeEntry.origin}
                      {metaStr(activeEntry, 'taskTitle') && (
                        <> · 任务「{metaStr(activeEntry, 'taskTitle')}」</>
                      )}
                    </div>
                    <div>
                      首次沉淀：{formatTime(activeEntry.addedAt)} · 最近更新：{formatTime(activeEntry.updatedAt)}
                    </div>
                    {activeEntry.origin === 'task_output' && (
                      <div>
                        {metaNum(activeEntry, 'confirmedAt') > 0 && (
                          <>
                            确认人：{memberName(metaStr(activeEntry, 'confirmedBy')) || '-'} · 确认时间：
                            {formatTime(metaNum(activeEntry, 'confirmedAt'))} ·{' '}
                          </>
                        )}
                        第 {metaNum(activeEntry, 'revision') || 1} 次确认
                      </div>
                    )}
                    {metaStr(activeEntry, 'editedBy') && (
                      <div>
                        人工编辑：{memberName(metaStr(activeEntry, 'editedBy'))} ·{' '}
                        {formatTime(metaNum(activeEntry, 'editedAt'))}
                      </div>
                    )}
                  </div>

                  {/* 标签 */}
                  <div className="text-[11px] flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
                    <span>标签：</span>
                    {editing ? (
                      <input
                        className="search-input flex-1"
                        style={{ backgroundColor: 'var(--bg-field)' }}
                        value={draftTags}
                        onChange={(e) => setDraftTags(e.target.value)}
                        placeholder="逗号分隔"
                      />
                    ) : (
                      <span>{activeEntry.tags || '-'}</span>
                    )}
                  </div>

                  {/* 正文 */}
                  {editing ? (
                    <textarea
                      className="search-input w-full resize-y"
                      style={{ backgroundColor: 'var(--bg-field)', minHeight: 120, lineHeight: 1.8 }}
                      value={draftContent}
                      onChange={(e) => setDraftContent(e.target.value)}
                      placeholder="知识正文"
                    />
                  ) : (
                    <div className="text-xs whitespace-pre-wrap leading-6" style={{ color: 'var(--text-secondary)' }}>
                      {activeEntry.content || '（无正文）'}
                    </div>
                  )}

                  {/* 交付物（条目级附件） */}
                  <div
                    className="rounded-md p-2 flex flex-col gap-1"
                    style={{ border: '1px dashed var(--border)' }}
                  >
                    <div className="text-[11px] flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
                      <Paperclip size={11} /> 交付物（{activeEntry.files.length}）
                    </div>
                    {activeEntry.files.length === 0 && (
                      <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                        该条目没有交付物
                      </span>
                    )}
                    {activeEntry.files.map((f) => (
                      <button
                        key={f.fileId}
                        onClick={() => void openFile(f.fileId)}
                        className="attachment-row w-full text-left flex items-center gap-2 px-1 py-1"
                        title="打开文件"
                      >
                        <FileText size={11} style={{ color: 'var(--accent)' }} />
                        <span className="text-[11px] truncate flex-1" style={{ color: 'var(--text-secondary)' }}>
                          {f.fileName}
                        </span>
                        <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                          {formatSize(f.fileSize)}
                        </span>
                      </button>
                    ))}
                  </div>

                  {activeEntry.sourceRef && (
                    <div>
                      <button
                        onClick={() => void jumpToTask()}
                        className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                        title="跳转到产出该知识的任务"
                      >
                        <ArrowUpRight size={11} /> 查看来源任务
                      </button>
                    </div>
                  )}
                </div>
              )}
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
