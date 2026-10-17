import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Library, Search, Loader2, FileText, X } from 'lucide-react';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
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

  useEffect(() => {
    void useEnclaveOrgStore.getState().loadOrgs();
  }, []);

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
                  <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>{e.title}</span>
                  <span className="text-[10px] line-clamp-2" style={{ color: 'var(--text-tertiary)' }}>
                    {e.content || '（无正文）'}
                  </span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{formatTime(e.addedAt)}</span>
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
                <div className="flex flex-col gap-2">
                  <div className="flex items-center gap-2">
                    <FileText size={13} style={{ color: 'var(--accent)' }} />
                    <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>{activeEntry.title}</span>
                  </div>
                  <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {activeEntry.tags && <span>标签：{activeEntry.tags} · </span>}
                    来源：{activeEntry.origin === 'task_output' ? '任务产出' : activeEntry.origin} · {formatTime(activeEntry.addedAt)}
                  </div>
                  <div className="text-xs whitespace-pre-wrap leading-6" style={{ color: 'var(--text-secondary)' }}>
                    {activeEntry.content || '（无正文）'}
                  </div>
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
