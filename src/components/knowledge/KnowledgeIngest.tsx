/**
 * KnowledgeIngest — 投喂 + 待确认队列
 *
 * 两条路径，语义不同，界面上一并显示但互不混淆：
 *   1. 用户投喂（片段 / 文件 / 网址 / 云文档）：意图明确，**整理后直接入库**，结果看「知识」「文件」两个 tab；
 *   2. 待确认队列：留给**自动沉淀**出来的候选（阶段 C 的模型抽取），采纳才入库。
 *
 * 文件与网址共用同一条后端链路：原文复制进库（sha256 去重、多库共享同一份）→ 抽取正文 → 按语义块入库。
 * 云文档同样汇入这条链路，只是"取内容"来自云文档平台（第一期：语雀）的开放接口 ——
 * 凭证只存在后端，前端这里只传平台名与文档 id。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { Scissors, FileUp, Globe, Sparkles, Check, Trash2, FolderOpen, Loader2, Wand2, Cloud, Square, AlertTriangle, RefreshCw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { MetaEditor } from './KnowledgeFieldInputs';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import {
  ORIGIN_LABEL,
  cloudSourceLabel,
  type CloudAccount,
  type CloudDoc,
  type CloudRepo,
  type IngestQueueItem,
  type KnowledgeBase,
  type KnowledgeCandidate,
  type KnowledgeMeta,
  type KnowledgeOrigin,
} from '../../types/knowledge';

type Mode = 'snippet' | 'file' | 'url' | 'cloud' | 'ai';

const MODE_META: Record<Mode, { label: string; icon: typeof Scissors; origin: KnowledgeOrigin }> = {
  snippet: { label: '片段', icon: Scissors, origin: 'snippet' },
  file: { label: '文件', icon: FileUp, origin: 'file' },
  url: { label: '网址', icon: Globe, origin: 'url' },
  cloud: { label: '云文档', icon: Cloud, origin: 'cloud' },
  ai: { label: 'AI 生成', icon: Wand2, origin: 'ai' },
};

/**
 * 一次最多投喂的云文档篇数。
 *
 * 文档列表接口不带正文，勾 N 篇就是 N 次串行请求 —— 不设上限时用户一次勾上百篇，
 * 既会撞平台限流，也让"处理中"变得漫长得无法判断是否卡住。
 */
const CLOUD_MAX_DOCS = 20;

interface KnowledgeIngestProps {
  base: KnowledgeBase;
  bases: KnowledgeBase[];
  candidates: KnowledgeCandidate[];
  onIngest: (input: { key: string; value: string; baseIds: string[]; origin: 'snippet' | 'work' }) => void;
  /** 投喂本地文件：`items[].name` 是「允许重命名」下用户填的名字（空 = 交给 AI 取） */
  onIngestFiles: (
    items: { path: string; name: string }[],
    baseIds: string[],
    opts: { asMarkdown: boolean; rename: boolean },
  ) => void;
  onIngestUrl: (url: string, baseIds: string[], name?: string) => void;
  /** 投喂云文档：勾选的文档逐篇取正文入库（`accountId` 指定用哪个已配置账号） */
  onIngestCloudDocs: (
    items: { id: string; title: string }[],
    baseIds: string[],
    opts: { accountId: string; namespace: string },
  ) => void;
  /** AI 生成：按主题产出一条草稿（落「待确认」） */
  onGenerate: (topic: string) => void;
  /** 投喂进行中（片段/文件/网页/云文档/AI 生成共用）：用于显示进度并禁用提交 */
  busy: { kind: 'snippet' | 'file' | 'url' | 'cloud' | 'ai'; label: string; step: string } | null;
  onAdopt: (id: number, patch: { key: string; value: string; meta: KnowledgeMeta }) => void;
  onReject: (id: number) => void;
}

export function KnowledgeIngest({
  base,
  bases,
  candidates,
  onIngest,
  onIngestFiles,
  onIngestUrl,
  onIngestCloudDocs,
  onGenerate,
  busy,
  onAdopt,
  onReject,
}: KnowledgeIngestProps) {
  const [mode, setMode] = useState<Mode>('snippet');
  const [text, setText] = useState('');
  /** 片段标题（可留空 → 交给 AI 生成；不再用首行硬截断当标题） */
  const [title, setTitle] = useState('');
  const [url, setUrl] = useState('');
  /** 网址投喂的文件名（留空 → 用网页标题）。网页标题常是站点名或「下载」这类空话，值得让用户改 */
  const [urlName, setUrlName] = useState('');
  const [extraBaseIds, setExtraBaseIds] = useState<string[]>([]);
  const [picking, setPicking] = useState(false);
  const [topic, setTopic] = useState('');
  /**
   * 待入库文件（拖入/选择只入列，点「提交入库」才真正开始）。
   * 之前是选完就静默提交，用户没有任何机会核对或命名 —— 而"文件名是错的、内容是别处存的"
   * 恰恰是网页另存为 HTML 之后的常态。
   */
  const [files, setFiles] = useState<{ path: string; name: string }[]>([]);
  /** 存为 markdown（含内容整理）：内容会被 AI 加工，落盘成 .md */
  const [asMarkdown, setAsMarkdown] = useState(false);
  /** 允许重命名：行内填了名字就用它，留空则由 AI 取；关掉时一律保留原文件名 */
  const [rename, setRename] = useState(false);
  /** 拖拽悬停在投喂面板上（高亮 + 显示"松开即加入清单"） */
  const [dragOver, setDragOver] = useState(false);
  const [dragCount, setDragCount] = useState(0);

  /* ── 云文档模式（第一期：语雀） ──
   * **账号是复数**：第一步是"选哪个账号"，而不是假定只有一个平台 / 一个账号。
   * 列表是"拉一次就是一份快照"的读操作，不进 store：它只服务这个面板，
   * 放 store 反而会让"切库/刷新"意外清掉用户正在勾选的清单。 */
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccount[] | null>(null);
  const [cloudAccountId, setCloudAccountId] = useState('');
  const [cloudRepos, setCloudRepos] = useState<CloudRepo[]>([]);
  const [cloudNamespace, setCloudNamespace] = useState('');
  const [cloudDocs, setCloudDocs] = useState<CloudDoc[]>([]);
  const [cloudSelected, setCloudSelected] = useState<string[]>([]);
  const [cloudLoading, setCloudLoading] = useState(false);
  const [cloudLoadingDocs, setCloudLoadingDocs] = useState(false);

  const panelRef = useRef<HTMLDivElement>(null);
  /** 加工能力：none 时投喂不失败，只是进「待整理」 */
  const capability = useKnowledgeStore((s) => s.ingestCapability);
  const pending = useKnowledgeStore((s) => s.ingestQueue);
  const modelBlocked = capability !== null && capability.canProcess === 'none';
  const autoRanRef = useRef(false);

  useEffect(() => {
    const st = useKnowledgeStore.getState();
    void st.loadIngestCapability();
    void st.loadIngestQueue();
  }, [base.id]);

  // 进入页面时若已有可用的模型和积压，顺手清一轮（失败/不可用会被后端直接返回 stopped）
  useEffect(() => {
    if (autoRanRef.current) return;
    if (!capability || capability.canProcess === 'none' || capability.pending === 0) return;
    autoRanRef.current = true;
    void useKnowledgeStore.getState().runIngestQueue();
  }, [capability?.canProcess, capability?.pending]);

  // 模型一消失就把这两个"需要加工"的开关复位：否则会提交一个必然被拒的组合
  useEffect(() => {
    if (modelBlocked) {
      setAsMarkdown(false);
      setRename(false);
    }
  }, [modelBlocked]);

  const targetBaseIds = [base.id, ...extraBaseIds];

  /** 拉某个账号的知识库列表。换账号时把下游（知识库 / 文档 / 勾选）一起清掉 ——
   *  旧账号选中的知识库在另一个账号下根本不存在，留着只会变成一次必然失败的请求。 */
  const loadCloudRepos = useCallback(async (accountId: string) => {
    setCloudRepos([]);
    setCloudNamespace('');
    setCloudDocs([]);
    setCloudSelected([]);
    if (!accountId) return;
    setCloudLoading(true);
    try {
      const repos = await invoke<CloudRepo[]>('kb_cloud_list_repos', { accountId });
      setCloudRepos(repos);
      if (repos.length === 0) showToast('这个账号下没有可选的云文档知识库', 'info');
    } catch (e) {
      showToast(`拉取云文档知识库失败：${errorMessage(e)}`, 'error');
    } finally {
      setCloudLoading(false);
    }
  }, []);

  /** 读账号列表 → 选中 `keepId`（已不在列表里就退回第一个）→ 拉它的知识库 */
  const reloadCloud = useCallback(async (keepId: string) => {
    setCloudLoading(true);
    try {
      const accounts = await invoke<CloudAccount[]>('kb_cloud_accounts_list');
      setCloudAccounts(accounts);
      const next = accounts.some((a) => a.id === keepId) ? keepId : (accounts[0]?.id ?? '');
      setCloudAccountId(next);
      await loadCloudRepos(next);
    } catch (e) {
      showToast(`读取云文档账号失败：${errorMessage(e)}`, 'error');
      setCloudAccounts([]);
    } finally {
      setCloudLoading(false);
    }
  }, [loadCloudRepos]);

  // 进入「云文档」模式时读一次账号。每次进入都重读：用户可能刚在设置里加了账号。
  // 推一个微任务再调，避免在 effect 体内同步 setState。
  useEffect(() => {
    if (mode !== 'cloud') return;
    void Promise.resolve().then(() => reloadCloud(''));
  }, [mode, reloadCloud]);

  const loadCloudDocs = async (namespace: string) => {
    if (!namespace) return;
    setCloudLoadingDocs(true);
    setCloudDocs([]);
    setCloudSelected([]);
    try {
      const docs = await invoke<CloudDoc[]>('kb_cloud_list_docs', {
        accountId: cloudAccountId,
        namespace,
      });
      setCloudDocs(docs);
      if (docs.length === 0) showToast('这个知识库里没有文档', 'info');
    } catch (e) {
      showToast(`拉取文档列表失败：${errorMessage(e)}`, 'error');
    } finally {
      setCloudLoadingDocs(false);
    }
  };

  const toggleCloudDoc = (id: string) => {
    setCloudSelected((prev) => {
      if (prev.includes(id)) return prev.filter((x) => x !== id);
      if (prev.length >= CLOUD_MAX_DOCS) {
        showToast(`一次最多投喂 ${CLOUD_MAX_DOCS} 篇，请分批进行`, 'info');
        return prev;
      }
      return [...prev, id];
    });
  };

  const submitCloud = () => {
    if (cloudSelected.length === 0 || !cloudNamespace || !cloudAccountId) return;
    const items = cloudDocs
      .filter((d) => cloudSelected.includes(d.id))
      .map((d) => ({ id: d.id, title: d.title || d.id }));
    onIngestCloudDocs(items, targetBaseIds, { accountId: cloudAccountId, namespace: cloudNamespace });
    setCloudSelected([]);
  };

  /** 入列（按路径去重：同名文件可能被拖两次，重复入列只会白跑一遍） */
  const addFiles = (paths: string[]) => {
    setFiles((prev) => {
      const seen = new Set(prev.map((f) => f.path));
      const add = paths.filter((p) => !seen.has(p)).map((p) => ({ path: p, name: '' }));
      return add.length > 0 ? [...prev, ...add] : prev;
    });
  };

  /**
   * 拖入文件先**入列**（不再当场投喂）：用户可以核对清单、填名字、勾选加工方式，
   * 再由「提交入库」真正开始 —— 之前是选完/拖进去就悄悄开始，那个按钮永远是灰的，
   * 用户以为按钮坏了，其实文件已经在处理。
   */
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const isOverPanel = (x: number, y: number): boolean => {
      const el = panelRef.current;
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
          const p = event.payload;
          if (p.type === 'enter') {
            const over = isOverPanel(p.position.x, p.position.y);
            setDragOver(over);
            setDragCount(over ? p.paths.length : 0);
            // 拖着文件进来必然是"要投文件"，顺手切到文件模式，反馈才跟得上
            if (over) setMode('file');
          } else if (p.type === 'over') {
            setDragOver(isOverPanel(p.position.x, p.position.y));
          } else if (p.type === 'leave') {
            setDragOver(false);
            setDragCount(0);
          } else if (p.type === 'drop') {
            const over = isOverPanel(p.position.x, p.position.y);
            setDragOver(false);
            setDragCount(0);
            if (over && p.paths.length > 0) {
              setMode('file');
              addFiles(p.paths);
            }
          }
        });
        if (disposed) unlisten();
      } catch (e) {
        // 浏览器（无 Tauri 运行时）下必然失败，只提示不报错
        console.warn('[KnowledgeIngest] 注册拖拽监听失败:', e);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const submitSnippet = () => {
    const value = text.trim();
    if (!value) return;
    // 标题交给用户填（可留空）：留空时由 AI 整理出标题，不再用首行硬截断 —— 那样会
    // 把半句话当标题。AI 也没给出时，后端会从正文按句末标点推导一个能看的标题。
    onIngest({ key: title.trim(), value, baseIds: targetBaseIds, origin: 'snippet' });
    setText('');
    setTitle('');
  };

  const pickFiles = async () => {
    setPicking(true);
    try {
      const picked = await openDialog({ multiple: true, title: '选择要投喂到知识库的文件' });
      const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
      if (paths.length === 0) return;
      addFiles(paths);
    } catch (e) {
      showToast(`选择文件失败: ${errorMessage(e)}`, 'error');
    } finally {
      setPicking(false);
    }
  };

  const submitFiles = () => {
    if (files.length === 0) return;
    onIngestFiles(files, targetBaseIds, { asMarkdown, rename });
    setFiles([]);
  };

  const submitUrl = () => {
    const target = url.trim();
    if (!target) return;
    onIngestUrl(target, targetBaseIds, urlName.trim());
    setUrl('');
    setUrlName('');
  };

  const submitAi = () => {
    const t = topic.trim();
    if (!t) return;
    onGenerate(t);
  };

  const canSubmit =
    !busy
    && !(mode === 'ai' && modelBlocked)
    && (mode === 'snippet' ? Boolean(text.trim())
      : mode === 'url' ? Boolean(url.trim())
        : mode === 'ai' ? Boolean(topic.trim())
          : mode === 'cloud' ? cloudSelected.length > 0
            : files.length > 0);

  const submit = () => {
    if (mode === 'snippet') submitSnippet();
    else if (mode === 'url') submitUrl();
    else if (mode === 'ai') submitAi();
    else if (mode === 'cloud') submitCloud();
    else submitFiles();
  };

  return (
    <div className="flex-1 flex overflow-hidden">
      {/* 左：投喂（也是拖入文件的投放区） */}
      <div
        ref={panelRef}
        className="w-[400px] shrink-0 flex flex-col overflow-hidden p-4 gap-3"
        style={{ backgroundColor: 'var(--bg-side)' }}
      >
        <div className="flex items-center gap-1 p-0.5 rounded-lg shrink-0" style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}>
          {(Object.keys(MODE_META) as Mode[]).map((m) => {
            const M = MODE_META[m];
            const active = mode === m;
            const blocked = m === 'ai' && modelBlocked;
            return (
              <button
                key={m}
                onClick={() => setMode(m)}
                disabled={blocked}
                title={blocked ? 'AI 生成需要可用模型（本地或云端）' : undefined}
                className="flex-1 flex items-center justify-center gap-1 px-2 py-1 rounded-md text-[11px] transition-colors"
                style={{
                  backgroundColor: active ? 'var(--bg-primary)' : 'transparent',
                  color: active ? 'var(--accent)' : 'var(--text-secondary)',
                  fontWeight: active ? 600 : 500,
                  opacity: blocked ? 0.45 : 1,
                  cursor: blocked ? 'not-allowed' : 'pointer',
                }}
              >
                <M.icon size={11} />
                {M.label}
              </button>
            );
          })}
        </div>

        {/* 加工能力条：把"能不能整理"和"能不能投喂"分开讲清楚。
            投喂永远可用；没有模型时只是原文入库并排队，不会产出机械切片。 */}
        {capability && (
          <div
            className="shrink-0 flex items-start gap-2 px-2.5 py-2 rounded-lg text-[11px]"
            style={{
              backgroundColor: modelBlocked ? 'var(--status-warning-bg)' : 'var(--bg-tertiary)',
              color: modelBlocked ? 'var(--status-warning)' : 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            {modelBlocked ? (
              <AlertTriangle size={12} className="mt-0.5 shrink-0" />
            ) : (
              <Sparkles size={12} className="mt-0.5 shrink-0" />
            )}
            <div className="min-w-0 flex-1">
              {modelBlocked ? (
                <>
                  暂无可用模型：内容会先入库并进入「待整理」，不会生成机械切片；
                  配好本地模型或恢复网络后自动整理。
                </>
              ) : (
                <>
                  加工能力：
                  {capability.canProcess === 'local' ? '本地模型' : '云端模型'}
                  {capability.model ? ` · ${capability.model}` : ''}
                  {capability.providerName ? `（${capability.providerName}）` : ''}
                </>
              )}
              {modelBlocked && capability.reason && (
                <div className="mt-0.5 text-[10px] opacity-80 break-all">原因：{capability.reason}</div>
              )}
            </div>
            {capability.pending > 0 && (
              <button
                onClick={() => void useKnowledgeStore.getState().runIngestQueue()}
                disabled={modelBlocked}
                className="pd-btn shrink-0 flex items-center gap-1 px-2 py-0.5 rounded text-[10px]"
                style={{
                  backgroundColor: 'var(--bg-primary)',
                  border: '1px solid var(--border)',
                  color: modelBlocked ? 'var(--text-tertiary)' : 'var(--text-primary)',
                  opacity: modelBlocked ? 0.6 : 1,
                }}
                title={modelBlocked ? '模型可用后才会开始整理' : '立即整理「待整理」队列'}
              >
                <RefreshCw size={10} />
                立即整理 {capability.pending}
              </button>
            )}
          </div>
        )}

        {/* 投喂进行中：这是长任务（要调模型、长文档分批），必须有可见反馈 */}
        {busy && (
          <div
            className="shrink-0 flex items-start gap-2 px-2.5 py-2 rounded-lg text-[11px]"
            style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)', border: '1px solid var(--border)' }}
          >
            <Loader2 size={12} className="animate-spin shrink-0 mt-0.5" />
            <div className="min-w-0">
              <div>{busy.label}</div>
              {busy.step && (
                <div className="text-[10px] mt-0.5 truncate opacity-80" title={busy.step}>
                  {busy.step}
                </div>
              )}
            </div>
          </div>
        )}

        {mode === 'snippet' && (
          <>
            <div className="text-[10px] leading-relaxed shrink-0" style={{ color: 'var(--text-tertiary)' }}>
              粘贴一段草稿（结论 / 摘录 / 规则）。会先由 AI 整理成规范知识再进「待确认」，采纳时写入库。
            </div>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="标题（可留空，由 AI 生成）"
              className="shrink-0 w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={'把结论、摘录、规则丢进来……'}
              className="flex-1 w-full p-3 rounded-lg outline-none resize-none text-xs leading-relaxed"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
          </>
        )}

        {mode === 'file' && (
          <>
            {/* 投放区：拖入 / 点选都只**入列**，不在这里开始处理 */}
            <div
              className={`${files.length === 0 ? 'flex-1' : 'shrink-0'} flex flex-col items-center justify-center gap-1.5 py-3 rounded-lg text-xs`}
              style={{
                border: `1px ${dragOver ? 'solid' : 'dashed'} ${dragOver ? 'var(--accent)' : 'var(--border)'}`,
                backgroundColor: dragOver ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                color: dragOver ? 'var(--accent)' : 'var(--text-tertiary)',
              }}
            >
              <FileUp size={18} />
              <div className="font-medium">{dragOver ? '松开即加入清单' : '把文件拖到这里'}</div>
              <div className="text-[10px] px-6 text-center leading-relaxed">
                {dragOver && dragCount > 0
                  ? `${dragCount} 个文件 · 提交后入库到「${base.name}」${extraBaseIds.length > 0 ? ` 等 ${targetBaseIds.length} 个库` : ''}`
                  : '加入清单后可以核对、命名，再点「提交入库」才开始处理'}
              </div>
              <button
                onClick={() => void pickFiles()}
                disabled={picking || Boolean(busy)}
                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
              >
                {picking ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
                或点击选择文件
              </button>
            </div>

            {files.length > 0 && (
              <>
                <div className="shrink-0 flex items-center justify-between text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  <span>待入库 {files.length} 个</span>
                  <button className="pd-btn" onClick={() => setFiles([])}>清空</button>
                </div>
                <div className="flex-1 min-h-0 overflow-y-auto space-y-1.5">
                  {files.map((f, i) => {
                    const { stem, ext } = splitName(f.path);
                    const patch = (name: string) =>
                      setFiles((prev) => prev.map((x, j) => (i === j ? { ...x, name } : x)));
                    return (
                      <div key={f.path} className="rounded-lg px-2 py-1.5 space-y-1" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
                        <div className="flex items-center gap-1.5">
                          <span className="flex-1 min-w-0 truncate text-[11px]" style={{ color: 'var(--text-primary)' }} title={f.path}>
                            {stem}{ext}
                          </span>
                          <button
                            className="pd-btn shrink-0"
                            title="从清单移除"
                            onClick={() => setFiles((prev) => prev.filter((_, j) => j !== i))}
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            <Trash2 size={10} />
                          </button>
                        </div>
                        <div className="flex items-center gap-1">
                          <input
                            value={f.name}
                            disabled={!rename}
                            onChange={(e) => patch(e.target.value)}
                            placeholder={rename ? '留空则由 AI 取名' : stem}
                            // 这是**叶子输入框**（自带边框），不能挂 `pd-field`：
                            // 那个类是给外层容器用的，挂在 input 上会同一次聚焦里
                            // 叠加「accent 边框 + 2px 光晕 + 全局 1px 描边」，看起来是双层高亮
                            className="flex-1 min-w-0 px-1.5 py-1 rounded text-[10px] outline-none"
                            style={{
                              backgroundColor: 'var(--bg-field)',
                              color: rename ? 'var(--text-primary)' : 'var(--text-tertiary)',
                              border: '1px solid var(--border)',
                            }}
                          />
                          {ext && (
                            <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{ext}</span>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            )}

            {/* 两个加工开关：正交，2×2 都有意义；都需要模型，没有时禁用并说明 */}
            <div className="shrink-0 space-y-2 pt-1" style={{ borderTop: '1px solid var(--border)' }}>
              <Switch
                checked={asMarkdown}
                onChange={setAsMarkdown}
                disabled={modelBlocked}
                label="存为 markdown 格式"
                hint={modelBlocked
                  ? '需要模型整理，当前不可用；取消后可先按原文件入库待整理'
                  : '由 AI 整理内容并转成 .md 落盘（网页另存为的 HTML 最需要）'}
              />
              <Switch
                checked={rename}
                onChange={setRename}
                disabled={modelBlocked}
                label="允许重命名"
                hint={modelBlocked
                  ? '需要模型取名，当前不可用（可先入库，模型恢复后再整理）'
                  : '内容一字不动，仅更正文件名；行内留空则交给 AI 取名'}
              />
            </div>
          </>
        )}

        {mode === 'url' && (
          <div className="flex-1 flex flex-col gap-2">
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com/article"
              className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
            <input
              value={urlName}
              onChange={(e) => setUrlName(e.target.value)}
              placeholder="文件名（留空则由 AI 取名）"
              className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
            <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              抓到的正文落盘为 .md，再由 AI <strong>整理成规范知识</strong>：剔掉导航 / 页脚 / 广告 /
              相关阅读 / 评论区等噪声，按主题分节、补全标点（不改动任何事实）。纯 JS 渲染的站点可能抓不到内容。
              <br />
              文件名也由 AI 取（网页标题常是站点名或「下载」「首页」这类空话，不能用）；
              想自己定就填上面那一栏。
            </div>
          </div>
        )}

        {mode === 'cloud' && (
          <div className="flex-1 min-h-0 flex flex-col gap-2">
            {cloudAccounts === null ? (
              <div className="flex-1 flex flex-col items-center justify-center gap-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                <Loader2 size={16} className="animate-spin" />
                正在读取云文档账号…
              </div>
            ) : cloudAccounts.length === 0 ? (
              <div
                className="rounded-lg px-2.5 py-2 text-[10px] leading-relaxed"
                style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
              >
                还没有云文档账号：请到「设置 › 知识库 › 云文档账号」添加（如语雀，Token 勾只读权限即可），
                再回到这里拉取文档列表。
              </div>
            ) : (
              <>
                {/* 账号放在第一位：可能配了多个（个人 / 团队 / 多平台），不能替用户假定是哪一个 */}
                <div className="shrink-0 flex items-center gap-1.5">
                  <Select
                    value={cloudAccountId}
                    onChange={(v) => {
                      setCloudAccountId(v);
                      void loadCloudRepos(v);
                    }}
                    options={cloudAccounts.map((a) => ({
                      value: a.id,
                      label: `${cloudSourceLabel(a.source)} · ${a.label}`,
                    }))}
                    size="sm"
                    className="flex-1 min-w-0"
                    title="用哪个云文档账号拉取"
                  />
                  <button
                    className="pd-btn shrink-0 px-2 py-1.5 rounded-lg text-[11px]"
                    onClick={() => void reloadCloud(cloudAccountId)}
                    disabled={cloudLoading}
                    style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
                    title="重新读取账号列表并刷新知识库"
                  >
                    {cloudLoading ? <Loader2 size={11} className="animate-spin" /> : '刷新'}
                  </button>
                </div>

                {/* 选知识库即拉取文档列表：多一步"拉取"按钮只是多一次点击，没有额外信息 */}
                <Select
                  value={cloudNamespace}
                  onChange={(v) => {
                    setCloudNamespace(v);
                    void loadCloudDocs(v);
                  }}
                  options={cloudRepos.map((r) => ({
                    value: r.namespace,
                    label: r.groupName ? `${r.groupName} / ${r.name}` : r.name,
                  }))}
                  size="sm"
                  className="w-full"
                  placeholder={
                    cloudLoading ? '正在读取知识库…' : cloudRepos.length ? '选择云文档知识库' : '没有可选的知识库'
                  }
                  disabled={cloudLoading || cloudRepos.length === 0}
                  title="选择要拉取文档的知识库"
                />

                {cloudLoadingDocs ? (
                  <div className="flex-1 flex flex-col items-center justify-center gap-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    <Loader2 size={16} className="animate-spin" />
                    正在拉取文档列表…
                  </div>
                ) : cloudDocs.length > 0 ? (
                  <>
                    <div className="shrink-0 flex items-center justify-between text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      <span>已选 {cloudSelected.length} / {cloudDocs.length} 篇（上限 {CLOUD_MAX_DOCS}）</span>
                      <button
                        className="pd-btn"
                        onClick={() => {
                          const all = cloudSelected.length >= cloudDocs.length;
                          setCloudSelected(all ? [] : cloudDocs.slice(0, CLOUD_MAX_DOCS).map((d) => d.id));
                        }}
                      >
                        {cloudSelected.length >= cloudDocs.length ? '全不选' : '全选'}
                      </button>
                    </div>
                    <div className="flex-1 min-h-0 overflow-y-auto space-y-1">
                      {cloudDocs.map((d) => {
                        const on = cloudSelected.includes(d.id);
                        return (
                          <button
                            key={d.id}
                            onClick={() => toggleCloudDoc(d.id)}
                            className="w-full text-left px-2 py-1.5 rounded-lg flex items-start gap-1.5"
                            style={{
                              backgroundColor: on ? 'var(--accent-light)' : 'var(--bg-secondary)',
                              border: '1px solid var(--border)',
                            }}
                          >
                            <span className="shrink-0 mt-[1px]" style={{ color: on ? 'var(--accent)' : 'var(--text-tertiary)' }}>
                              {on ? <Check size={11} /> : <Square size={11} />}
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-[11px]" style={{ color: 'var(--text-primary)' }}>
                                {d.title || d.id}
                              </span>
                              <span className="block text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                                {d.wordCount > 0 ? `${d.wordCount} 字 · ` : ''}
                                {d.updatedAt ? d.updatedAt.slice(0, 10) : ''}
                              </span>
                            </span>
                          </button>
                        );
                      })}
                    </div>
                    <div className="shrink-0 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      投喂的是正文纯文本（图片 / 附件 / 画板取不到），且是拉取快照：
                      云端后续改动不会自动跟随，需要重新拉取。
                    </div>
                  </>
                ) : (
                  <div className="flex-1 flex items-center justify-center px-4 text-center text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {cloudRepos.length === 0 ? '这个账号下没有可选的云文档知识库' : '选择知识库后即可拉取文档列表'}
                  </div>
                )}
              </>
            )}
          </div>
        )}

        {mode === 'ai' && (
          <div className="flex-1 flex flex-col gap-2">
            <textarea
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              placeholder={'想要一条什么知识？\n例如：本项目里知识库与 KV 记忆是什么关系'}
              className="flex-1 w-full p-3 rounded-lg outline-none resize-none text-xs leading-relaxed"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
            <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              由「知识库模型」生成（设置 › 知识库可指定，未配置则用第一个可用提供商），标题 / 正文 / 标签 / 专属属性一次给出，
              草稿落「待确认」由你核对后再入库。
            </div>
          </div>
        )}

        {/* 目标库：默认当前库，可同时投喂到多个库（多对多的日常入口） */}
        <div className="shrink-0 space-y-1.5">
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>入库目标</div>
          <div className="flex items-center gap-1.5 flex-wrap">
            <span className="px-1.5 py-0.5 rounded text-[10px]" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
              {base.name}
            </span>
            {bases.filter((b) => b.id !== base.id).map((b) => {
              const on = extraBaseIds.includes(b.id);
              return (
                <button
                  key={b.id}
                  onClick={() => setExtraBaseIds((prev) => (on ? prev.filter((x) => x !== b.id) : [...prev, b.id]))}
                  className="pd-btn px-1.5 py-0.5 rounded text-[10px]"
                  style={{
                    backgroundColor: on ? 'var(--accent-light)' : 'var(--bg-field)',
                    color: on ? 'var(--accent)' : 'var(--text-secondary)',
                    border: '1px solid var(--border)',
                  }}
                  title={on ? '取消同时入库' : '同时加入该库（不复制内容，只加关联）'}
                >
                  {on ? '✓ ' : '+ '}{b.name}
                </button>
              );
            })}
          </div>
        </div>

        <button
          onClick={submit}
          disabled={!canSubmit}
          className="pd-btn shrink-0 flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg text-xs"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
            border: 'none',
            opacity: canSubmit ? 1 : 0.5,
            cursor: canSubmit ? 'pointer' : 'not-allowed',
          }}
        >
          {busy ? (
            <>
              <Loader2 size={12} className="animate-spin" />
              处理中…
            </>
          ) : mode === 'ai' ? (
            <>
              <Wand2 size={12} />
              生成草稿
            </>
          ) : (
            <>
              <Sparkles size={12} />
              {modelBlocked
                ? '提交入库（待整理）'
                : mode === 'file' || mode === 'cloud'
                  ? '提交入库'
                  : '提交整理'}
              {mode === 'file' && files.length > 0 ? `（${files.length}）` : ''}
              {mode === 'cloud' && cloudSelected.length > 0 ? `（${cloudSelected.length}）` : ''}
            </>
          )}
        </button>
      </div>

      {/* 右：待确认队列。
          「知识来源」说明已移到知识库**默认页** —— 那是新用户的第一屏，"内容能怎么进来"讲在那里
          才会被看见；放在这里只有已经点进某个库、又切到投喂 tab 的人才能看到。
          每种模式自己的说明仍留在左侧各自的面板里（那些是"这一步在做什么"，不是来源总览）。 */}
      <div className="flex-1 flex flex-col overflow-hidden">
        <div className="shrink-0 px-4 py-2.5 flex items-center gap-2" style={{ borderBottom: '1px solid var(--border)' }}>
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>待确认</span>
          <span className="px-1.5 py-0.5 rounded-full text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
            {candidates.length}
          </span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            自动沉淀的候选先落这里，采纳后才写入库；左侧投喂是明确意图，整理后直接入库
          </span>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          {/* 待整理：内容已收，但还没有模型做结构化整理。
              与下方「待确认」的区别：这里还没有知识条目，采纳与否无从谈起。 */}
          {pending.length > 0 && (
            <div
              className="rounded-lg p-3"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <div className="flex items-center gap-2 mb-2">
                <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
                  待整理
                </span>
                <span
                  className="px-1.5 py-0.5 rounded-full text-[10px]"
                  style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)' }}
                >
                  {pending.length}
                </span>
                <span className="text-[10px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                  原文已入库，模型可用后自动转成知识条目
                </span>
                <div className="flex-1" />
                <button
                  onClick={() => void useKnowledgeStore.getState().runIngestQueue()}
                  disabled={modelBlocked}
                  className="pd-btn shrink-0 flex items-center gap-1 px-2 py-0.5 rounded text-[10px]"
                  style={{
                    backgroundColor: 'var(--bg-primary)',
                    border: '1px solid var(--border)',
                    color: modelBlocked ? 'var(--text-tertiary)' : 'var(--text-primary)',
                    opacity: modelBlocked ? 0.6 : 1,
                  }}
                >
                  <RefreshCw size={9} />
                  立即整理
                </button>
              </div>
              <div className="space-y-1">
                {pending.map((item) => (
                  <div
                    key={item.id}
                    className="flex items-center gap-2 px-2 py-1 rounded text-[11px]"
                    style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
                  >
                    <span
                      className="shrink-0 px-1 rounded text-[9px]"
                      style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                    >
                      {KIND_LABEL[item.kind] ?? item.kind}
                    </span>
                    <span
                      className="flex-1 min-w-0 truncate"
                      style={{ color: 'var(--text-primary)' }}
                      title={pendingTitle(item)}
                    >
                      {pendingTitle(item)}
                    </span>
                    {item.status === 'failed' ? (
                      <span
                        className="shrink-0 text-[10px]"
                        style={{ color: 'var(--status-danger)' }}
                        title={item.lastError}
                      >
                        失败{item.attempts > 1 ? ` ×${item.attempts}` : ''}
                      </span>
                    ) : (
                      <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        {item.status === 'running' ? '整理中' : '等待模型'}
                      </span>
                    )}
                    {item.kind === 'snippet' && (
                      <button
                        className="shrink-0 text-[10px]"
                        style={{ color: 'var(--text-secondary)' }}
                        onClick={() => void useKnowledgeStore.getState().manualIngest(item.id)}
                        title="不经模型，按原文放进「待确认」"
                      >
                        手工入库
                      </button>
                    )}
                    <button
                      className="shrink-0 text-[10px]"
                      style={{ color: 'var(--accent)' }}
                      onClick={() => void useKnowledgeStore.getState().retryIngestItem(item.id)}
                      title="重试（模型可用时立即整理）"
                    >
                      重试
                    </button>
                    <button
                      className="shrink-0"
                      style={{ color: 'var(--text-tertiary)' }}
                      onClick={() => void useKnowledgeStore.getState().dropIngestItem(item.id)}
                      title="移出队列（原文仍保留在「文件」里）"
                    >
                      <Trash2 size={10} />
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}

          {candidates.length === 0 ? (
            <div className="h-full flex flex-col items-center justify-center gap-2 text-xs" style={{ color: 'var(--text-tertiary)' }}>
              <Check size={18} />
              队列已清空
            </div>
          ) : (
            candidates.map((c) => (
              <CandidateCard key={c.id} candidate={c} base={base} bases={bases} onAdopt={onAdopt} onReject={onReject} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

/* ────────────── 文件清单的两个小件 ────────────── */

/** 待整理条目的类型标签 */
const KIND_LABEL: Record<string, string> = {
  snippet: '片段',
  file: '文件',
  url: '网址',
  cloud: '云文档',
};

/** 待整理条目的展示名：从 payload 里取原始输入（不额外存冗余字段） */
function pendingTitle(item: IngestQueueItem): string {
  try {
    const p = JSON.parse(item.payload) as Record<string, unknown>;
    if (item.kind === 'file') {
      const path = String(p.path ?? '');
      return path.split(/[\\/]/).pop() || item.sourceRef;
    }
    if (item.kind === 'url') return String(p.url ?? item.sourceRef);
    if (item.kind === 'cloud') return String(p.name ?? p.docId ?? item.sourceRef);
    const text = String(p.value ?? p.key ?? '');
    return text.slice(0, 40) || '片段';
  } catch {
    return item.sourceRef;
  }
}

/**
 * 拆出文件名主体与扩展名（只按最后一段路径处理，纯展示用）。
 * 后缀本身由后端 `with_original_ext` 兜底 —— 用户写了别的后缀也以原文件格式为准。
 */
function splitName(path: string): { stem: string; ext: string } {
  const full = path.split(/[\\/]/).pop() ?? path;
  const i = full.lastIndexOf('.');
  return i > 0 ? { stem: full.slice(0, i), ext: full.slice(i) } : { stem: full, ext: '' };
}

/** 加工开关（两个，正交）：勾上去才由后端动手脚 */
function Switch({
  checked,
  onChange,
  disabled,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label: string;
  hint: string;
}) {
  return (
    <label
      className={`flex items-start gap-2 select-none ${disabled ? 'cursor-not-allowed' : 'cursor-pointer'}`}
      style={{ opacity: disabled ? 0.55 : 1 }}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-[2px] shrink-0"
        style={{ accentColor: 'var(--accent)', cursor: disabled ? 'not-allowed' : 'pointer' }}
      />
      <span className="min-w-0">
        <span className="block text-[11px]" style={{ color: 'var(--text-primary)' }}>{label}</span>
        <span className="block text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>{hint}</span>
      </span>
    </label>
  );
}

/* ────────────── 单张候选卡：可就地改标题 / 正文 / 专属属性 ────────────── */
function CandidateCard({
  candidate,
  base,
  bases,
  onAdopt,
  onReject,
}: {
  candidate: KnowledgeCandidate;
  base: KnowledgeBase;
  bases: KnowledgeBase[];
  onAdopt: (id: number, patch: { key: string; value: string; meta: KnowledgeMeta }) => void;
  onReject: (id: number) => void;
}) {
  const [key, setKey] = useState(candidate.key);
  const [value, setValue] = useState(candidate.value);
  const [meta, setMeta] = useState<KnowledgeMeta>(candidate.meta);

  const inputStyle = {
    backgroundColor: 'var(--bg-field)',
    color: 'var(--text-primary)',
    border: '1px solid var(--border)',
  } as const;

  const baseName = bases.find((b) => b.id === candidate.baseId)?.name ?? candidate.baseId;

  return (
    <div className="rounded-lg p-3 space-y-2.5" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
      <div className="flex items-center gap-2">
        <Sparkles size={12} style={{ color: 'var(--accent)', flexShrink: 0 }} />
        <input
          value={key}
          onChange={(e) => setKey(e.target.value)}
          className="flex-1 min-w-0 bg-transparent outline-none text-xs font-medium"
          style={{ color: 'var(--text-primary)' }}
        />
        <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
          {ORIGIN_LABEL[candidate.origin]}
        </span>
        <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
          → {baseName}
        </span>
      </div>

      <textarea
        value={value}
        onChange={(e) => setValue(e.target.value)}
        rows={3}
        className="w-full p-2 rounded-lg outline-none resize-none text-[11px] leading-relaxed"
        style={inputStyle}
      />

      {/* 专属属性按当前库的字段定义预填，采纳前可改 */}
      <MetaEditor fields={base.fields} value={meta} onChange={setMeta} />

      {candidate.tags.length > 0 && (
        <div className="flex items-center gap-1 flex-wrap">
          <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>标签</span>
          {candidate.tags.map((t) => (
            <span
              key={t}
              className="px-1.5 py-[1px] rounded text-[9px]"
              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
            >
              #{t}
            </span>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2">
        <span className="text-[10px] flex-1 min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }} title={`${candidate.reason}${candidate.sourceRef ? ` · ${candidate.sourceRef}` : ''}`}>
          {candidate.reason}
          {candidate.sourceRef ? ` · ${candidate.sourceRef}` : ''}
        </span>
        <button
          onClick={() => onAdopt(candidate.id, { key, value, meta })}
          disabled={!key.trim() || !value.trim()}
          className="pd-btn shrink-0 flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
            border: 'none',
            opacity: key.trim() && value.trim() ? 1 : 0.5,
          }}
        >
          <Check size={11} />
          采纳
        </button>
        <button
          onClick={() => onReject(candidate.id)}
          className="pd-btn shrink-0 flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
          style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
        >
          <Trash2 size={11} />
          丢弃
        </button>
      </div>
    </div>
  );
}
