import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Search, RefreshCw, X, Download, Check, Lightbulb, Sparkles, Loader2 } from 'lucide-react';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { TagChips } from '../inspiration/TagChips';

/**
 * 灵感市场（「资源市集 › 灵感」tab）。
 *
 * 与另外三个 tab 的关系：插件/模板/Agent 是"装到本地某个体系里"，灵感是"抄进自己的灵感库" ——
 * 所以这里唯一的动作就是「导入到我的灵感库」，导入后在「灵感库」（/inspirations）里正常编辑。
 *
 * 数据分两步取（与插件/模板市场同构）：
 *   1. 进 tab 拉 `inspiration_market_index` —— 只有标题/图标/标签/摘要，体积恒定小；
 *   2. 点开详情才按需拉 `inspiration_market_fetch` —— 正文只有一份，在源文件里。
 * 代价是搜索覆盖到摘要为止（摘要 200 字，基本涵盖提示词的场景说明部分）。
 *
 * 标签与本地「灵感库」对齐：卡片/详情都展示，导入时一并写进本地，因此
 * 「从市场抄来的」和「自己建的」在库里长得一样，都能按标签筛。
 */

/** 市场索引里的一条灵感（只有元信息，没有正文） */
interface MarketInspiration {
  id: string;
  title: string;
  icon: string;
  /**
   * 标签（可选）。
   * 生成脚本一定会写这个字段，但 CDN 上可能还留着旧版索引 —— 所以按可选处理，
   * 取用处统一 `?? []`，别让一条老数据把整个列表打崩。
   */
  tags?: string[];
  /** 正文摘要（生成脚本截取，200 字封顶） */
  excerpt: string;
  /** 相对市场根的路径，如 inspirations/<id>/<id>.json */
  path: string;
}

interface MarketIndex {
  schemaVersion: string;
  updatedAt: string;
  inspirations: MarketInspiration[];
}

/** 单条详情正文的加载状态（按 id 存放，避免"打开 A 又打开 B"时状态串台） */
interface ContentState {
  loading: boolean;
  error: string | null;
  content: string;
}

export function InspirationMarketTab() {
  const [items, setItems] = useState<MarketInspiration[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [detail, setDetail] = useState<MarketInspiration | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  /** marketId → 本地灵感 id：判断"是否已导入"，覆盖更新时定位到本地那条 */
  const [imported, setImported] = useState<Map<string, string>>(new Map());
  /** 已拉取过的正文，按 id 缓存（重开详情不再请求） */
  const [contents, setContents] = useState<Map<string, ContentState>>(new Map());

  /**
   * 已导入清单必须**不受灵感库页筛选状态影响**，所以直接查全量，
   * 不复用 inspirationStore（那个列表会被灵感库的标签/收藏筛选过）。
   */
  const loadImported = useCallback(async () => {
    try {
      const list = await invoke<{ id: string; marketId?: string | null }[]>('list_inspirations', {
        tag: null,
        favoriteOnly: false,
      });
      setImported(new Map(
        list.filter((i) => !!i.marketId).map((i) => [i.marketId as string, i.id]),
      ));
    } catch {
      /* 拿不到就当作"都没导入过"，不打断浏览 */
    }
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<MarketIndex>('inspiration_market_index');
      setItems(Array.isArray(data?.inspirations) ? data.inspirations : []);
    } catch (err) {
      setError(errorMessage(err));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    // 不在 effect 体内同步 setState（load 开头就 setLoading(true)，会被
    // `react-hooks/set-state-in-effect` 判为级联渲染）：推到微任务，同一个任务、早于绘制。
    void Promise.resolve().then(() => Promise.all([load(), loadImported()]));
  }, [load, loadImported]);

  /** 按需拉正文（只认 id，路径由后端从索引里取，前端不碰路径） */
  const loadContent = useCallback(async (id: string) => {
    setContents((prev) => new Map(prev).set(id, { loading: true, error: null, content: '' }));
    try {
      const data = await invoke<{ content?: string }>('inspiration_market_fetch', { id });
      setContents((prev) => new Map(prev).set(id, { loading: false, error: null, content: data?.content ?? '' }));
    } catch (err) {
      setContents((prev) => new Map(prev).set(id, { loading: false, error: errorMessage(err), content: '' }));
    }
  }, []);

  const openDetail = (item: MarketInspiration) => {
    setDetail(item);
    if (!contents.has(item.id)) void loadContent(item.id);
  };

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((it) =>
      it.title.toLowerCase().includes(q) || it.excerpt.toLowerCase().includes(q));
  }, [items, query]);

  /**
   * 导入 / 覆盖更新。已导入过则覆盖本地那条，避免同一灵感在库里堆成好几份。
   *
   * 覆盖时**不动标签**：那条的标签是用户自己的组织层（首次导入时市场标签已经写进去了，
   * 之后用户可能又加了自己的）。"用市场内容覆盖"覆盖的是内容（图标/标题/正文），
   * 顺手抹掉用户手工加的标签属于误伤。
   */
  const handleImport = async (item: MarketInspiration, content: string) => {
    setBusyId(item.id);
    try {
      const localId = imported.get(item.id);
      if (localId) {
        await invoke('update_inspiration', {
          payload: { id: localId, icon: item.icon, title: item.title, content },
        });
        showToast('已用市场内容覆盖更新（本地标签保留）', 'success');
      } else {
        await invoke('create_inspiration', {
          payload: {
            icon: item.icon,
            title: item.title,
            content,
            // 市场带的标签一并落地，导入后就能在灵感库/侧栏按标签筛
            tags: item.tags ?? [],
            // 标记来源：灵感库里能一眼看出这条是抄来的，用户自建的是 manual
            sourceAgent: 'market',
            marketId: item.id,
          },
        });
        showToast('已导入到我的灵感库', 'success');
      }
      await loadImported();
    } catch (err) {
      showToast(`导入失败：${errorMessage(err)}`, 'error');
    }
    setBusyId(null);
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* 工具行：搜索 + 计数 + 刷新 */}
      <div
        className="shrink-0 px-4 py-2 flex items-center gap-2"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        <div className="relative flex-1 min-w-0 max-w-md">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2"
            style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索灵感标题或摘要..."
            className="block w-full pl-8 pr-3 py-1.5 rounded-md text-[11px] outline-none"
            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
          />
        </div>

        {!loading && !error && (
          <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            共 {filtered.length !== items.length ? `${filtered.length}/${items.length}` : items.length} 条
          </span>
        )}

        <div className="flex-1" />

        <button
          onClick={() => { void load(); void loadImported(); }}
          disabled={loading}
          className="pd-btn text-[10px] px-2 py-1 rounded flex items-center gap-1 shrink-0"
          style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)' }}
          title="重新获取市场索引"
        >
          <RefreshCw size={11} className={loading ? 'pd-animate-spin' : ''} />
          刷新
        </button>
      </div>

      {/* 列表 */}
      <div className="flex-1 overflow-y-auto p-4 pd-scroll-stable">
        {error && (
          <div className="flex items-center gap-2 px-3 py-2 rounded-lg mb-3"
            style={{ backgroundColor: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.2)' }}>
            <span className="text-[11px] flex-1" style={{ color: '#EF4444' }}>
              读取灵感市场失败：{error}
            </span>
            <button
              onClick={() => void load()}
              className="pd-btn text-[10px] px-2 py-0.5 rounded"
              style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}
            >
              重试
            </button>
          </div>
        )}

        {loading && !error && (
          <div className="flex items-center justify-center py-12 text-xs" style={{ color: 'var(--text-tertiary)' }}>
            正在加载灵感市场...
          </div>
        )}

        {!loading && !error && filtered.length === 0 && (
          <div className="flex flex-col items-center justify-center py-12" style={{ color: 'var(--text-tertiary)' }}>
            <Lightbulb size={30} style={{ opacity: 0.3, marginBottom: 10 }} />
            <p className="text-[13px] mb-1">{query ? '没有匹配的灵感' : '灵感市场暂时还没有内容'}</p>
            <p className="text-[11px]">{query ? '换个关键词试试' : '欢迎把你的提示词分享上来'}</p>
          </div>
        )}

        {!loading && !error && filtered.length > 0 && (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {filtered.map((item) => {
              const isImported = imported.has(item.id);
              return (
                <button
                  key={item.id}
                  onClick={() => openDetail(item)}
                  /* pd-card-btn：覆盖全局 button 重置（它把按钮定成 inline-flex + row，
                     卡片需要纵向堆叠 —— 这个必须用未分层的类，Tailwind 的 flex-col 盖不住） */
                  className="pd-card-btn relative overflow-hidden rounded-lg p-3 transition-colors"
                  style={{
                    backgroundColor: 'var(--bg-secondary)',
                    border: `1px solid ${isImported ? 'var(--accent)' : 'var(--border)'}`,
                  }}
                  onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--accent)'; }}
                  onMouseLeave={(e) => { e.currentTarget.style.borderColor = isImported ? 'var(--accent)' : 'var(--border)'; }}
                >
                  {/* 「已导入」做成右上角角标：绝对定位、不进文字流，因此不会再与标题抢宽度或压到内容上。
                      卡片 overflow-hidden + 圆角会把角标的右上角裁成与卡片一致的弧度。 */}
                  {isImported && (
                    <span
                      className="absolute top-0 right-0 text-[9px] font-medium px-1.5 py-[3px]"
                      style={{
                        backgroundColor: 'var(--accent)',
                        color: '#fff',
                        borderBottomLeftRadius: 6,
                      }}
                    >
                      已导入
                    </span>
                  )}
                  {/* 标题行在有角标时右侧留出空位，避免长标题钻到角标底下 */}
                  <div className="flex items-center gap-2 min-w-0" style={isImported ? { paddingRight: 46 } : undefined}>
                    <span style={{ fontSize: 18, lineHeight: 1 }}>{item.icon}</span>
                    <span className="text-xs font-medium truncate flex-1" style={{ color: 'var(--text-primary)' }}>
                      {item.title}
                    </span>
                  </div>
                  <p className="text-[10px] mt-2 leading-relaxed line-clamp-3" style={{ color: 'var(--text-secondary)' }}>
                    {item.excerpt}
                  </p>
                  {/* 标签：与灵感库卡片、会话侧栏共用 TagChips，最多 4 个（超出折叠 +N） */}
                  <TagChips tags={item.tags ?? []} max={4} className="mt-2" />
                </button>
              );
            })}
          </div>
        )}
      </div>

      {detail && (
        <InspirationDetailDialog
          item={detail}
          state={contents.get(detail.id)}
          isImported={imported.has(detail.id)}
          busy={busyId === detail.id}
          onRetry={() => void loadContent(detail.id)}
          onImport={(content) => handleImport(detail, content)}
          onClose={() => setDetail(null)}
        />
      )}
    </div>
  );
}

/**
 * 灵感详情弹窗 —— 与「插件 README」同一套观感（居中模态 + 遮罩点击关闭）。
 *
 * 正文按**纯文本**渲染（white-space: pre-wrap），不走 Markdown：
 * 提示词里的 `---`、`|` 之类符号在 Markdown 下会被解释成分割线/表格，把原文改样。
 */
function InspirationDetailDialog({
  item, state, isImported, busy, onRetry, onImport, onClose,
}: {
  item: MarketInspiration;
  /** undefined = 还没开始拉（刚挂载那一帧），按加载中渲染 */
  state: ContentState | undefined;
  isImported: boolean;
  busy: boolean;
  onRetry: () => void;
  onImport: (content: string) => void;
  onClose: () => void;
}) {
  const loading = !state || state.loading;
  const content = state?.content ?? '';
  const canImport = !loading && content.length > 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[640px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)', overflow: 'hidden' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="shrink-0 px-5 py-3 flex items-center justify-between gap-3"
          style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="flex items-center gap-2 min-w-0">
            <span style={{ fontSize: 20, lineHeight: 1 }}>{item.icon}</span>
            <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>
              {item.title}
            </span>
            {isImported && (
              <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded-full flex items-center gap-0.5"
                style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
                <Check size={9} /> 已导入
              </span>
            )}
          </div>
          <button onClick={onClose} className="pd-btn px-1.5 py-1 rounded text-[10px] shrink-0"
            style={{ color: 'var(--text-tertiary)' }}>
            <X size={14} />
          </button>
        </div>

        {/* Body：正文原样展示，保留换行 */}
        <div className="flex-1 overflow-y-auto pd-scroll-stable" style={{ padding: '16px 20px' }}>
          {loading && (
            <div className="flex items-center justify-center gap-2 py-10 text-xs" style={{ color: 'var(--text-tertiary)' }}>
              <Loader2 size={14} className="pd-animate-spin" />
              正在获取正文...
            </div>
          )}

          {!loading && state?.error && (
            <div className="flex flex-col items-center gap-3 py-10">
              <span className="text-xs" style={{ color: '#EF4444' }}>获取正文失败：{state.error}</span>
              <button onClick={onRetry} className="pd-btn px-3 py-1 rounded text-[11px]"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                重试
              </button>
            </div>
          )}

          {!loading && !state?.error && (
            <>
              {/* 详情里展示全部标签（卡片上为了排版折叠了） */}
              <TagChips tags={item.tags ?? []} className="mb-3" />
              <div className="text-xs leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
                {content}
              </div>
            </>
          )}
        </div>

        {/* Footer */}
        <div className="shrink-0 px-5 py-3 flex items-center justify-between gap-2"
          style={{ borderTop: '1px solid var(--border)' }}>
          <span className="text-[10px] flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <Sparkles size={10} />
            {isImported ? '再次导入会用市场内容覆盖你本地那条' : '导入后可在「灵感库」里编辑'}
          </span>
          <button
            onClick={() => onImport(content)}
            disabled={busy || !canImport}
            className="pd-btn px-4 py-1.5 rounded text-xs flex items-center gap-1.5 shrink-0"
            style={{
              backgroundColor: isImported ? 'var(--bg-tertiary)' : 'var(--accent)',
              color: isImported ? 'var(--text-secondary)' : '#fff',
              border: isImported ? '1px solid var(--border)' : 'none',
              opacity: canImport ? 1 : 0.45,
              cursor: canImport ? 'pointer' : 'default',
            }}
          >
            <Download size={12} />
            {busy ? '处理中...' : isImported ? '用市场内容覆盖更新' : '导入到我的灵感库'}
          </button>
        </div>
      </div>
    </div>
  );
}
