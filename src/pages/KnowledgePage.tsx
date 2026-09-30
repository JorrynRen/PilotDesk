/**
 * KnowledgePage — 知识库（独立路由 /knowledge）
 *
 * 数据来自后端 kb_* 命令（MEMORY.db）。页面结构：左列表 + 右「知识 / 文件 / 投喂 / 图谱」。
 *   左：知识库列表（用户只在这里维护库的「定义」）
 *   右：选中库的三个视图 —— 知识（检索）/ 文件 / 投喂 / 图谱
 *
 * 与记忆的关系：知识库是 KV 记忆的命名子集（多对多关联），通用属性直接用记忆表自带的
 * 分类 / 标签 / 热度 / 重要，专属属性按「库 × 条目」各存一份。
 */

import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  Library, Plus, Pencil, Trash2, Layers, Sparkles, Network, Search, Loader2, FolderOpen, X,
} from 'lucide-react';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { makeBaseId, useKnowledgeStore, type KnowledgeTab } from '../stores/knowledgeStore';
import { KnowledgeEntryList } from '../components/knowledge/KnowledgeEntryList';
import { KnowledgeFileList } from '../components/knowledge/KnowledgeFileList';
import { KnowledgeIngest } from '../components/knowledge/KnowledgeIngest';
import { KnowledgeGraph } from '../components/knowledge/KnowledgeGraph';
import { KnowledgeBaseDialog } from '../components/knowledge/KnowledgeBaseDialog';
import { confirmDialog } from '../stores/confirmStore';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import {
  ORIGIN_COLOR,
  ORIGIN_DESC,
  ORIGIN_LABEL,
  type KnowledgeOrigin,
} from '../types/knowledge';

const TAB_META: Record<KnowledgeTab, { label: string; icon: typeof Search }> = {
  entries: { label: '知识', icon: Search },
  files: { label: '文件', icon: Layers },
  ingest: { label: '投喂', icon: Sparkles },
  graph: { label: '图谱', icon: Network },
};

export function KnowledgePage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const bases = useKnowledgeStore((s) => s.bases);
  const entries = useKnowledgeStore((s) => s.entries);
  const files = useKnowledgeStore((s) => s.files);
  const fileAttrs = useKnowledgeStore((s) => s.fileAttrs);
  const candidates = useKnowledgeStore((s) => s.candidates);
  const activeBaseId = useKnowledgeStore((s) => s.activeBaseId);
  const activeTab = useKnowledgeStore((s) => s.activeTab);
  const loading = useKnowledgeStore((s) => s.loading);
  const ingestBusy = useKnowledgeStore((s) => s.ingestBusy);

  const [dialog, setDialog] = useState<{ open: boolean; baseId: string | null }>({ open: false, baseId: null });

  // 首次进入拉库列表（store 会连带加载首个库的条目/文件/候选）
  useEffect(() => {
    void useKnowledgeStore.getState().loadBases();
  }, []);

  // 组合开关点击：切模式并回到主布局（与设置页同一套处理）
  const handleModeChange = useCallback((mode: ViewMode) => {
    setMode(mode);
    navigate('/');
  }, [setMode, navigate]);

  // entries / files / candidates 已经是「当前库」的内容（过滤在后端做，切库即重拉）
  //
  // **刻意不兜底到 `bases[0]`**：知识库默认是"没打开"的，要用户自己在左列表里点。
  // 兜底到第一个会让人以为"默认就是开着的"，也永远看不到那个带涉密提示的默认页。
  const base = bases.find((b) => b.id === activeBaseId) ?? null;

  const handleDeleteBase = async (id: string, name: string) => {
    // 影响面由后端算：只有「不再被任何库关联」的条目会被一并删除
    const orphanCount = entries.filter((e) => e.baseIds.length === 1 && e.baseIds[0] === id).length;
    const ok = await confirmDialog({
      title: `删除知识库「${name}」`,
      message: `库定义与全部关联会被删除；其中 ${orphanCount} 条知识不再被其它任何库关联，将被一并删除（仍被其它库引用的条目与文件会保留）。\n`
        + '只想删掉库里的文件？请到「文件」页选中后用「从库中移除」，那里不会动库定义。',
      confirmText: '删除知识库',
    });
    if (!ok) return;
    const removed = await useKnowledgeStore.getState().deleteBase(id);
    if (!removed) return;
    showToast(`已删除「${name}」：连带删除 ${removed.entries} 条知识、${removed.files} 个文件登记`, 'success');
  };

  /** 在文件管理器里打开知识库根目录：翻原文、拷走整份资料是最直接的"取出"方式，
   *  比在应用里一层层点开快得多（根目录可能已被用户自定义，所以现问一次） */
  const openKnowledgeRoot = async () => {
    try {
      const info = await invoke<{ root: string }>('kb_root_info');
      await invoke('open_path', { path: info.root });
    } catch (e) {
      showToast(`打开目录失败：${errorMessage(e)}`, 'error');
    }
  };

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        knowledgeOpen
        onOpenKnowledge={() => { /* 已在知识库页，无需动作 */ }}
        onOpenSettings={() => navigate('/settings')}
      />

      <div className="flex flex-1 overflow-hidden">
        {/* 左：知识库列表 */}
        <aside className="w-[240px] shrink-0 flex flex-col overflow-hidden" style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
          <div className="h-10 shrink-0 px-3 flex items-center gap-2" style={{ borderBottom: '1px solid var(--border)' }}>
            <Library size={13} style={{ color: 'var(--accent)' }} />
            <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>知识库</span>
            <span className="px-1.5 py-0.5 rounded-full text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
              {bases.length}
            </span>
            <div className="flex-1" />
            <button
              onClick={() => setDialog({ open: true, baseId: null })}
              className="pd-btn p-1 rounded hover:opacity-80"
              style={{ color: 'var(--accent)' }}
              title="新建知识库（只需填名称、描述、专属属性字段）"
            >
              <Plus size={13} />
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-2 space-y-1">
            {bases.map((b) => {
              const active = base?.id === b.id;
              return (
                <button
                  key={b.id}
                  onClick={() => void useKnowledgeStore.getState().selectBase(b.id)}
                  className="w-full text-left px-2.5 py-2 rounded-lg transition-colors"
                  style={{
                    // 卡片要「标题 / 描述 / 统计」纵向三行，但全局 button 重置把 <button> 定成了
                    // inline-flex + row + gap 4（未分层，优先级高于 Tailwind 的 flex-col 工具类），
                    // 唯一能覆盖它的是行内样式 —— 否则三行会被排成三列（标题被压成 0 宽）
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'stretch',
                    gap: 0,
                    // 卡片态：未选中也要有边框，否则一堆卡片贴在侧栏底色上，边界全糊在一起。
                    // 选中态**统一按会话列表（`SessionListItem`）的口径**：选中 = `var(--border)` 底、
                    // 未选中 = 透明（卡片描边另给）。主题色（`--accent-light`）只留给批量多选，
                    // 不用它表达"我在哪儿" —— 一屏卡片里只有一个"当前"，铺主题色太吵。
                    backgroundColor: active ? 'var(--border)' : 'transparent',
                    border: '1px solid var(--border)',
                  }}
                >
                  <div className="flex items-center gap-1.5 min-w-0">
                    <Library size={10} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
                    <span className="text-xs truncate" style={{ color: 'var(--text-primary)', fontWeight: active ? 600 : 500 }}>{b.name}</span>
                    {b.pendingCount > 0 && (
                      <span className="ml-auto shrink-0 px-1.5 py-[1px] rounded-full text-[9px]" style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>
                        {b.pendingCount}
                      </span>
                    )}
                  </div>
                  <div className="text-[10px] mt-1 line-clamp-2" style={{ color: 'var(--text-tertiary)' }}>{b.description || '（无描述）'}</div>
                  <div className="flex items-center gap-2 mt-1.5 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                    <span>{b.entryCount} 条知识</span>
                    <span>{b.fileCount} 个文件</span>
                    <span>{b.fields.length} 个专属字段</span>
                  </div>
                </button>
              );
            })}
          </div>

          <div className="shrink-0 px-3 py-2 text-[9px] leading-relaxed" style={{ borderTop: '1px solid var(--border)', color: 'var(--text-tertiary)' }}>
            知识条目与 KV 记忆同库存储：不占 600 条会话记忆配额，也不会被自动清理。
          </div>
        </aside>

        {/* 右：选中库的视图 */}
        {base ? (
          <main className="flex-1 flex flex-col overflow-hidden">
            <div className="shrink-0 px-4 pt-3" style={{ borderBottom: '1px solid var(--border)' }}>
              <div className="flex flex-col">
                {/* 标题行与按钮同一行：简介因此能占满整宽 —— 放在左侧那一列里时，
                    按钮一多就把简介挤窄、`line-clamp-2` 一到就截断，看着像"简介丢了后半句" */}
                <div className="flex items-center gap-2 min-w-0">
                  <h2 className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>{base.name}</h2>
                  {loading && (
                    <span className="shrink-0 flex items-center gap-1 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                      <Loader2 size={10} className="animate-spin" />
                      加载中
                    </span>
                  )}
                  <div className="flex-1" />
                  <div className="flex items-center gap-2 shrink-0">
                    <button
                      onClick={() => useKnowledgeStore.getState().closeBase()}
                      className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                      style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                      title="关闭这个知识库（回到默认页）。库、条目、文件都不会动。"
                    >
                      <X size={11} />
                      关闭知识库
                    </button>
                    <button
                      onClick={() => void openKnowledgeRoot()}
                      className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                      style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                      title="在文件管理器里打开知识库目录（原文与库结构都在这里）"
                    >
                      <FolderOpen size={11} />
                      打开库根目录
                    </button>
                    <button
                      onClick={() => setDialog({ open: true, baseId: base.id })}
                      className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                      style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                    >
                      <Pencil size={11} />
                      编辑定义
                    </button>
                    <button
                      onClick={() => void handleDeleteBase(base.id, base.name)}
                      className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                      style={{ color: 'var(--status-danger, #EF4444)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                    >
                      <Trash2 size={11} />
                      删除知识库
                    </button>
                  </div>
                </div>
                <div className="text-[11px] mt-1 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>{base.description}</div>
                <div className="flex items-center gap-3 mt-1.5 text-[10px] flex-wrap" style={{ color: 'var(--text-tertiary)' }}>
                  <span>{base.entryCount} 条知识</span>
                  <span>{base.fileCount} 个文件</span>
                  <span>{base.pinnedCount} 条重要</span>
                  <span>{base.fields.length} 个专属字段</span>
                </div>
              </div>

              {/* tab：知识 / 文件 / 投喂 / 图谱 */}
              <div className="flex items-center gap-1 mt-2">
                {(Object.keys(TAB_META) as KnowledgeTab[]).map((t) => {
                  const M = TAB_META[t];
                  const active = activeTab === t;
                  const badge = t === 'ingest' ? candidates.length : 0;
                  return (
                    <button
                      key={t}
                      onClick={() => useKnowledgeStore.getState().setTab(t)}
                      className="flex items-center gap-1 px-2.5 py-1.5 text-[11px] transition-colors"
                      style={{
                        color: active ? 'var(--accent)' : 'var(--text-secondary)',
                        borderBottom: active ? '2px solid var(--accent)' : '2px solid transparent',
                        fontWeight: active ? 600 : 500,
                      }}
                    >
                      <M.icon size={11} />
                      {M.label}
                      {badge > 0 && (
                        <span className="px-1 rounded-full text-[9px]" style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>{badge}</span>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>

            {activeTab === 'entries' && (
              <KnowledgeEntryList
                base={base}
                entries={entries}
                onSave={(key, patch) => useKnowledgeStore.getState().updateEntry(key, patch)}
                onRemove={(key) => void useKnowledgeStore.getState().removeFromBase(key)}
                onRemoveMany={(keys) => void useKnowledgeStore.getState().removeEntries(keys)}
              />
            )}
            {activeTab === 'files' && <KnowledgeFileList base={base} files={files} fileAttrs={fileAttrs} />}
            {activeTab === 'ingest' && (
              <KnowledgeIngest
                base={base}
                bases={bases}
                candidates={candidates}
                onIngest={(input) => void useKnowledgeStore.getState().addCandidates(input)}
                onIngestFiles={(items, baseIds, opts) =>
                  void useKnowledgeStore.getState().ingestFiles(items, baseIds, opts)}
                onIngestUrl={(url, baseIds, name) => void useKnowledgeStore.getState().ingestUrl(url, baseIds, name)}
                onIngestCloudDocs={(items, baseIds, opts) =>
                  void useKnowledgeStore.getState().ingestCloudDocs(items, baseIds, opts)}
                onGenerate={(topic) => void useKnowledgeStore.getState().generate(topic)}
                busy={ingestBusy}
                onAdopt={(id, patch) => void useKnowledgeStore.getState().adoptCandidate(id, patch)}
                onReject={(id) => void useKnowledgeStore.getState().rejectCandidate(id)}
              />
            )}
            {activeTab === 'graph' && <KnowledgeGraph base={base} entries={entries} />}
          </main>
        ) : (
          /**
           * 默认页：知识库**默认是关着的**（不自动打开任何一个库），这里就是打开前的那一屏。
           *
           * 两件事必须在这一屏讲：① 涉密提示 —— 知识库的 AI 整理与生成会把内容提交给 LLM（联网），
           * 这是用户在按下第一个按钮**之前**就该知道的；② 知识来源 —— "内容能怎么进来"决定了
           * 他要不要试一把，讲在这里才有人看到（放在投喂 tab 里，只有已经打开库又切过去的人才看得见）。
           *
           * 内容高于一屏时靠外层滚动：`justify-center` 在滚动容器里会把顶部截掉，
           * 所以用 `min-h-full` 包一层 —— 内容不超过一屏时居中，超过了就从顶部正常排下来。
           */
          <main className="flex-1 overflow-y-auto px-8 py-6">
            <div className="min-h-full flex flex-col items-center justify-center gap-4">
              <Library size={22} style={{ color: 'var(--text-tertiary)' }} />
              {bases.length === 0 ? (
                <>
                  <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>还没有知识库</div>
                  <button
                    onClick={() => setDialog({ open: true, baseId: null })}
                    className="pd-btn flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
                  >
                    <Plus size={12} />
                    新建知识库
                  </button>
                </>
              ) : (
                <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                  从左侧选择一个知识库打开（共 {bases.length} 个）
                </div>
              )}

              <KnowledgeOrigins />

              <div
                className="w-full max-w-[560px] px-3.5 py-2.5 rounded-lg text-[11px] leading-relaxed text-center"
                style={{
                  color: 'var(--text-secondary)',
                  backgroundColor: 'rgba(245, 158, 11, 0.10)',
                  border: '1px solid rgba(245, 158, 11, 0.35)',
                }}
              >
                <span className="font-medium" style={{ color: '#F59E0B' }}>涉密提示：</span>
                知识库 AI 整理需联网提交 LLM，请勿提交涉密文件、内容。
              </div>
            </div>
          </main>
        )}
      </div>

      {/* 全局状态栏：独立路由也要保留（与会话/工作流等模式一致），否则底部少一条、页面像被截断 */}
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />

      {dialog.open && (
        <KnowledgeBaseDialog
          base={dialog.baseId ? bases.find((b) => b.id === dialog.baseId) ?? null : null}
          onClose={() => setDialog({ open: false, baseId: null })}
          onSubmit={(input) => {
            const store = useKnowledgeStore.getState();
            setDialog({ open: false, baseId: null });
            if (dialog.baseId) {
              void store.updateBase(dialog.baseId, input);
            } else {
              // id 由前端按名称生成（后端只校验唯一性），撞了就加序号
              const taken = new Set(bases.map((b) => b.id));
              let id = makeBaseId(input.name);
              let n = 2;
              while (taken.has(id)) id = `${makeBaseId(input.name)}-${n++}`;
              void store.createBase(input, id);
            }
          }}
        />
      )}
    </div>
  );
}

/**
 * 知识来源说明（默认页用）。
 *
 * 为什么在默认页：这里是新用户打开知识库时看到的第一屏，"内容能怎么进来"是决定他要不要
 * 试一把的信息；放在投喂 tab 里只有「已经打开某个库 + 又切到投喂」的人才看得到。
 *
 * 六条两列排：单列会拖成很长一竖，而这一屏还要放涉密提示。
 */
function KnowledgeOrigins() {
  return (
    <div
      className="w-full max-w-[560px] px-4 py-3 rounded-lg"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-center gap-1.5 mb-2">
        <Sparkles size={11} style={{ color: 'var(--accent)' }} />
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>知识来源（6 种）</span>
      </div>
      <div className="grid grid-cols-2 gap-x-4 gap-y-2">
        {(Object.keys(ORIGIN_DESC) as KnowledgeOrigin[]).map((o) => {
          // 「工作中沉淀」是自动的，其余都要用户发起 —— 这个区别决定了用户需不需要动手
          const auto = ORIGIN_DESC[o].trigger === '自动';
          return (
            <div key={o} className="flex items-start gap-1.5 min-w-0">
              <span className="w-2 h-2 rounded-full shrink-0 mt-[5px]" style={{ backgroundColor: ORIGIN_COLOR[o] }} />
              <div className="min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-[11px]" style={{ color: 'var(--text-primary)' }}>{ORIGIN_LABEL[o]}</span>
                  <span
                    className="shrink-0 px-1 py-[1px] rounded text-[9px]"
                    style={{
                      backgroundColor: auto ? 'var(--bg-tertiary)' : 'var(--accent-light)',
                      color: auto ? 'var(--text-tertiary)' : 'var(--accent)',
                    }}
                  >
                    {ORIGIN_DESC[o].trigger}
                  </span>
                </div>
                <div className="mt-0.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  {ORIGIN_DESC[o].note}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
