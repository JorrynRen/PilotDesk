import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { Globe, Plus, X, Settings2, ExternalLink, AlertTriangle, Search, LayoutGrid } from 'lucide-react';
import { useCustomTabsStore, groupCustomTabs, PORTAL_TAB_ID, type CustomTab } from '../../stores/customTabsStore';
import { useTerminal } from '../../TerminalManager';
import { CustomTabsSettings } from '../settings/CustomTabsSettings';
import { TabIcon } from '../common/TabIcon';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/** 目录索引协议地址：?path=<encodeURIComponent(绝对路径)>，由后端动态生成文件列表页 */
const DIRINDEX_BASE = 'http://dirindex.localhost/';

/**
 * iframe 常驻上限（LRU）。
 * 标签按钮全部保留，但只让**最近使用**的最多 8 个标签挂载 iframe；超出即卸载最久未用者
 * （其 iframe 从 DOM 移除，网页状态释放），再次点开时重新加载 —— 用内存换标签数不设上限。
 */
const FRAME_LRU_MAX = 8;

/**
 * iframe 加载超时阈值（毫秒）。
 * 无法可靠探测 X-Frame-Options / CSP 拒绝（多数浏览器下被拒的 iframe 仍会触发 onload），
 * 故采用「超时提示 + 手动出口」：超过此时长仍未 onload，则该标签内容区顶部给出提示条。
 */
const LOAD_TIMEOUT_MS = 8000;

/** 把标签 url 解析为 iframe src：
 *  网络地址(http/https)直接使用；本地目录走 dirindex 协议生成索引页；本地文件走 asset 协议 */
function resolveFrameSrc(url: string, isDir: boolean): string {
  if (/^https?:\/\//i.test(url)) return url;
  if (isDir) return `${DIRINDEX_BASE}?path=${encodeURIComponent(url)}`;
  return convertFileSrc(url);
}

/** 门户卡片副行文案：网络地址显示主机名，本地路径直接显示原路径（过长由 CSS 截断） */
function subtitleOf(url: string): string {
  if (/^https?:\/\//i.test(url)) {
    try { return new URL(url).host; } catch { return url; }
  }
  return url;
}

/**
 * 门户标签页壳：固定在 MainLayout 的 TitleBar 与 StatusBar 之间。
 *
 * 生命周期模型：
 * - CustomTabHost 常挂载（App 以 CSS 显隐），主视图切换不会卸载 iframe；
 * - 标签被激活时才创建 iframe，此后切换仅 display 显隐、网页状态保留；iframe 常驻数量上限 8（LRU），
 *   超出即卸载最久未用者（标签按钮仍全留），再次点开时重新加载；
 * - 顶栏标签有三态：激活（当前查看，主色高亮）/ 保活中（iframe 仍挂载但未查看，淡底 + 主色小圆点）/
 *   未加载（从未打开或已关闭，透明底）—— 用样式把"保活"与"当前查看"分开；
 * - 「门户」是 custom 模式下的首页（非真实标签，用哨兵 id 表示）：按分组渲染全部标签卡片网格，
 *   卡片点击即打开该标签（顶栏切到该标签、加载其 iframe），另提供搜索 / 管理与「在浏览器打开」；
 *   进入 custom 模式时**默认落在此页**（无有效激活标签即回落门户），不预加载任何标签的 iframe；
 * - 顶栏「＋」打开「标签管理」覆盖层（**门户态隐藏** —— 门户内容区已有「管理」按钮，
 *   两处并存只是同一功能的两个入口）：管理页以 absolute 覆盖叠加在主视图之上，
 *   主树（含全部 iframe）保持挂载，关闭覆盖层不会导致内容重新加载；
 * - 每个标签右上角的关闭角标（悬停 / 聚焦该标签时显现，触屏常显）：**关闭 = 卸载其 iframe**
 *   并把激活态回落门户页，但标签按钮**保留在顶栏** —— 关闭不是"让标签消失"，只是取消它的
 *   激活 / 加载状态，再次点击即重新加载；配置始终保留（删除配置在「标签管理」里做）。
 * - 离开 custom 模式会清空 LRU 窗口（卸载全部 iframe）：下次进门户是干净的默认页，不预加载任何标签。
 */
export function CustomTabHost() {
  const { tabs, activeTabId, setActiveTab } = useCustomTabsStore();
  const { viewMode } = useTerminal();

  // 标签管理覆盖层（覆盖叠加在主视图上，不替换/卸载主树）
  const [showManage, setShowManage] = useState(false);

  // 已挂载 iframe 的标签，**按最近使用排序（队首最新）**，并裁到 FRAME_LRU_MAX（LRU keep-alive）
  const [activatedIds, setActivatedIds] = useState<string[]>([]);
  // 门户页搜索关键词（按名称 / 地址过滤）
  const [query, setQuery] = useState('');
  // 本地路径目录判定结果缓存：url -> isDir（http 无需判定）
  const [dirByUrl, setDirByUrl] = useState<Record<string, boolean>>({});
  // iframe 加载状态：key = `${id}|${url}`；无记录=加载中，'loaded'=已 onload，'timeout'=超时未加载
  const [frameLoadState, setFrameLoadState] = useState<Record<string, 'loaded' | 'timeout'>>({});
  // 用户手动关闭「可能无法内嵌」提示的标签（key）
  const [dismissedTip, setDismissedTip] = useState<Record<string, boolean>>({});

  const active = useMemo(() => tabs.find((t) => t.id === activeTabId) || null, [tabs, activeTabId]);

  // 门户页激活态：custom 模式下用哨兵 id 表示「当前激活的是门户页」而非某个标签
  const isPortal = activeTabId === PORTAL_TAB_ID;
  // 门户页过滤结果（按名称 / 地址，忽略大小写）
  const portalFiltered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return tabs;
    return tabs.filter((t) => t.label.toLowerCase().includes(q) || t.url.toLowerCase().includes(q));
  }, [tabs, query]);
  // 门户页分组（组顺序 = 组内最小 order；未分组恒排最后）
  const portalGroups = useMemo(() => groupCustomTabs(portalFiltered), [portalFiltered]);

  // 离开自定义模式：清空 LRU 窗口（卸载全部标签 iframe）。
  // 目的是让「下次打开门户」回到干净的默认页 —— 不预加载任何标签，只有用户点选后才加载。
  const prevViewRef = useRef(viewMode);
  useEffect(() => {
    const prev = prevViewRef.current;
    prevViewRef.current = viewMode;
    if (prev === 'custom' && viewMode !== 'custom') {
      setActivatedIds([]);
    }
  }, [viewMode]);

  /**
   * custom 模式的**默认落点是门户页**：当前没有「有效」激活标签时（null，或指向已被删除的残留 id）回落到门户。
   *
   * 为什么需要：开门户时若残留着上次的激活标签，就会顺带把那个标签的 iframe 拉起来 ——
   * 标签应当只在用户显式点选（顶栏段 / 门户卡片）后才加载。
   * 注意：已挂载的 iframe 属 keep-alive（LRU），这里**不做卸载**。
   */
  useEffect(() => {
    if (viewMode !== 'custom') return;
    if (activeTabId === PORTAL_TAB_ID) return;
    if (activeTabId && tabs.some((t) => t.id === activeTabId)) return;
    setActiveTab(PORTAL_TAB_ID);
  }, [viewMode, activeTabId, tabs, setActiveTab]);

  // 同步设置中的删除：被移除配置的标签清出运行时状态，避免幽灵 chip / iframe。
  // 用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮级联渲染
  // （`react-hooks/set-state-in-effect`），而"外部值变了就修剪派生状态"正是 React 推荐的
  // adjust-during-render 场景，两者行为一致。
  const [prevTabs, setPrevTabs] = useState(tabs);
  if (prevTabs !== tabs) {
    setPrevTabs(tabs);
    const valid = new Set(tabs.map((t) => t.id));
    setActivatedIds((prev) => prev.filter((id) => valid.has(id)));
  }

  // 当前激活标签首次出现（或被重新点开、地址变更）时挂载其 iframe。同样用「渲染期修正」代替 effect。
  // key 含 active.url：修改地址后也触发一次（幂等），iframe 以新地址重建（src 变化本身会触发加载）。
  const activeKey = active ? `${active.id}|${active.url}` : null;
  const [prevActiveKey, setPrevActiveKey] = useState<string | null>(null);
  if (activeKey !== prevActiveKey) {
    setPrevActiveKey(activeKey);
    if (active) {
      // LRU：移到队首（最近使用）并裁到上限；被裁掉的标签卸载 iframe，下次点开时重新加载
      setActivatedIds((prev) => [active.id, ...prev.filter((id) => id !== active.id)].slice(0, FRAME_LRU_MAX));
    }
  }

  // 本地路径判定：首次需要时异步检测目录/文件，结果按 url 缓存
  const activeUrl = active?.url;
  useEffect(() => {
    if (!activeUrl || /^https?:\/\//i.test(activeUrl)) return;
    if (dirByUrl[activeUrl] !== undefined) return;
    let cancelled = false;
    invoke<boolean>('path_is_directory', { path: activeUrl })
      .then((isDir) => { if (!cancelled) setDirByUrl((m) => ({ ...m, [activeUrl]: isDir })); })
      .catch(() => { if (!cancelled) setDirByUrl((m) => ({ ...m, [activeUrl]: false })); });
    return () => { cancelled = true; };
  }, [activeUrl, dirByUrl]);

  /**
   * 关闭指定标签（顶栏标签行的关闭角标共用此逻辑）：**卸载其 iframe**（移出 LRU 窗口）
   * 并把激活态回落门户页；标签按钮**保留在顶栏**（关闭不是"让标签消失"，只是取消激活 / 加载），
   * 再次点击即重新加载。配置始终保留 —— 删除配置在「标签管理」里做。
   */
  const closeTab = useCallback((closingId: string) => {
    setActivatedIds((prev) => prev.filter((id) => id !== closingId));
    if (activeTabId === closingId) setActiveTab(PORTAL_TAB_ID);
  }, [activeTabId, setActiveTab]);

  /** 打开标签（顶栏段 / 门户卡片点击共用）：激活即加载 —— iframe 由 activeKey 的修正逻辑挂载 */
  const openTab = useCallback((id: string) => {
    setActiveTab(id);
  }, [setActiveTab]);

  // 在 LRU 窗口内、src 就绪的标签 → 渲染为常驻 iframe（顺序即最近使用顺序）。
  // 网络地址无需目录判定；本地路径等 dirByUrl 结果返回后再挂载，避免 src 切换触发二次加载。
  const frames = useMemo(
    () =>
      activatedIds
        .map((id) => tabs.find((t) => t.id === id))
        .filter(
          (t): t is CustomTab =>
            !!t && (/^https?:\/\//i.test(t.url) || dirByUrl[t.url] !== undefined)
        ),
    [tabs, activatedIds, dirByUrl]
  );

  // 加载超时：对每个已挂载 iframe 起 8 秒计时；到点仍未 onload 则标记超时（已 loaded 的跳过）。
  // 帧集合变化（新增/关闭帧、地址变更）会重建计时器 —— 即"重新加载时重置计时"。
  useEffect(() => {
    const timers = frames.map((t) => {
      const key = `${t.id}|${t.url}`;
      return setTimeout(() => {
        setFrameLoadState((m) => (m[key] === 'loaded' ? m : { ...m, [key]: 'timeout' }));
      }, LOAD_TIMEOUT_MS);
    });
    return () => { timers.forEach(clearTimeout); };
  }, [frames]);

  // 切换标签时"重置提示"：清掉目标标签的提示关闭标记，使其若仍无法内嵌能重新给出提示。
  // 用「渲染期修正」而非 effect，与上方其它派生状态一致（避免 set-state-in-effect 级联渲染）。
  const [prevActiveKeyForTip, setPrevActiveKeyForTip] = useState<string | null>(null);
  if (activeKey !== prevActiveKeyForTip) {
    setPrevActiveKeyForTip(activeKey);
    if (activeKey && dismissedTip[activeKey]) {
      setDismissedTip((m) => {
        const next = { ...m };
        delete next[activeKey];
        return next;
      });
    }
  }

  /** iframe onload：清除该标签的超时提示（正常内嵌） */
  const handleFrameLoad = useCallback((key: string) => {
    setFrameLoadState((m) => (m[key] === 'loaded' ? m : { ...m, [key]: 'loaded' }));
  }, []);

  /** 关闭「可能无法内嵌」提示条（仅本标签，本次会话） */
  const dismissEmbedTip = useCallback((key: string) => {
    setDismissedTip((m) => ({ ...m, [key]: true }));
  }, []);

  // 在浏览器打开：复用 open_path（http(s) 交系统默认浏览器；本地路径交系统默认程序）
  const openInBrowser = useCallback(async (targetUrl: string) => {
    try {
      await invoke('open_path', { path: targetUrl });
    } catch (e) {
      showToast(`打开失败：${errorMessage(e)}`, 'error');
    }
  }, []);

  // 提示条只在「当前激活标签超时未加载且用户未关闭提示」时显示
  const showEmbedTip = !!activeKey && frameLoadState[activeKey] === 'timeout' && !dismissedTip[activeKey];

  return (
    <div className="flex-1 flex flex-col overflow-hidden relative">
      {/* 标签切换行 */}
      <div
        className="flex items-center gap-1 px-3 h-9 shrink-0 overflow-x-auto"
        style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        {/* 门户：custom 模式下的首页（特殊页，非真实标签，故无关闭角标） */}
        <div className="pd-tab-chip relative shrink-0">
          <button
            onClick={() => setActiveTab(PORTAL_TAB_ID)}
            className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs transition-colors"
            style={{
              backgroundColor: isPortal ? 'var(--accent)' : 'transparent',
              color: isPortal ? '#fff' : 'var(--text-secondary)',
            }}
            title="门户：按分组查看全部标签"
          >
            <LayoutGrid size={11} />
            门户
          </button>
        </div>
        {/* 全部门户标签常驻此行（不做"关闭即消失"），标签有三种状态：
            - 激活态：当前正在查看 → 主色高亮；
            - 保活中：iframe 仍挂载（LRU 窗口内）但不是当前查看的 → 未激活态 + 主色小圆点，
              让"已加载、切换即时显示"与"未加载、点开才加载"能一眼区分；
            - 未加载：从未打开 / 已关闭 → 透明底普通样式。
            关闭只卸载 iframe 并回落门户页，标签按钮仍在此处，点一下即重新加载。 */}
        {tabs.map((t) => {
          const isActive = t.id === activeTabId;
          const keepAlive = !isActive && activatedIds.includes(t.id);
          return (
            <div key={t.id} className="pd-tab-chip relative shrink-0">
              <button
                onClick={() => setActiveTab(t.id)}
                className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs transition-colors"
                style={{
                  backgroundColor: isActive ? 'var(--accent)' : keepAlive ? 'var(--bg-tertiary)' : 'transparent',
                  color: isActive ? '#fff' : keepAlive ? 'var(--text-primary)' : 'var(--text-secondary)',
                }}
                title={keepAlive ? `${t.url}（保活中：页面已加载，切换即时显示）` : t.url}
              >
                <TabIcon icon={t.icon} size={11} />
                {t.label}
                {keepAlive && (
                  <span
                    aria-hidden
                    className="shrink-0 rounded-full"
                    style={{ width: 4, height: 4, backgroundColor: 'var(--accent)' }}
                  />
                )}
              </button>
              {/* 关闭角标：默认隐藏，悬停 / 聚焦该标签时显现（触屏常显）。
                  仅关闭本标签（stopPropagation 阻止冒泡到标签的切换点击）。 */}
              <button
                type="button"
                aria-label={`关闭标签 ${t.label}`}
                title="关闭标签"
                onClick={(e) => { e.stopPropagation(); closeTab(t.id); }}
                className="pd-tab-chip-x"
              >
                <X size={9} />
              </button>
            </div>
          );
        })}

        <div className="flex-1" />

        {/* 「＋」：打开标签管理覆盖层（主视图保持挂载，关闭后内容不重载）。
            门户态隐藏 —— 门户内容区已有「管理」按钮，两者并存只是同一功能的两个入口。 */}
        {!isPortal && (
          <button
            onClick={() => setShowManage(true)}
            className="flex items-center px-1.5 py-1 rounded-md text-xs shrink-0 transition-colors"
            style={{ color: 'var(--text-secondary)' }}
            title="管理门户标签"
          >
            <Plus size={14} />
          </button>
        )}
      </div>

      {/* 内容区：门户页 / iframe keep-alive 常驻层（切换仅显隐）。
          内嵌页面是**另一个文档**：它的原生滚动条颜色取决于那个文档的 color-scheme。
          深色族下由 CSS 给 iframe 透传 color-scheme（见 globals.css），
          让「自己没声明过颜色方案」的网页不再出现白滚动条；
          而**自己声明了 light / 自带浅色 UI** 的网页我们无法干预 —— 那是对方的设计。 */}
      <div className="flex-1 relative" style={{ backgroundColor: isPortal ? 'var(--bg-content)' : '#fff' }}>
        {frames.map((t) => (
          <iframe
            key={t.id}
            src={resolveFrameSrc(t.url, !!dirByUrl[t.url])}
            className="absolute inset-0 w-full h-full border-0"
            style={{ display: t.id === activeTabId ? 'block' : 'none' }}
            title={t.label}
            allowFullScreen
            onLoad={() => handleFrameLoad(`${t.id}|${t.url}`)}
          />
        ))}

        {/* 门户页：按分组渲染全部门户标签卡片网格（搜索 / 管理 / 空态 / 已打开标记 / 在浏览器打开） */}
        {isPortal && (
          <div className="absolute inset-0 overflow-y-auto" style={{ backgroundColor: 'var(--bg-content)' }}>
            <div className="max-w-[1080px] mx-auto px-6 py-5">
              {/* 标题行 */}
              <div className="flex items-center gap-2 mb-4">
                <LayoutGrid size={16} style={{ color: 'var(--accent)' }} />
                <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>门户</h2>
                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>共 {tabs.length} 个门户标签</span>
                <div className="flex-1" />
                <button
                  onClick={() => setShowManage(true)}
                  className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-xs transition-all active:scale-[.98]"
                  style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
                  title="管理门户标签"
                >
                  <Settings2 size={12} />
                  管理
                </button>
              </div>

              {/* 搜索框：按名称 / 地址过滤 */}
              <div className="relative mb-4 max-w-[320px]">
                <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none" style={{ color: 'var(--text-tertiary)' }} />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="按名称或地址搜索"
                  className="w-full pl-8 pr-3 py-2 rounded-lg text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
                />
              </div>

              {tabs.length === 0 ? (
                // 空态：一个标签都没有
                <div className="flex flex-col items-center justify-center gap-3 py-16">
                  <Globe size={24} style={{ color: 'var(--text-tertiary)' }} />
                  <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>还没有门户标签，去添加</p>
                  <button
                    onClick={() => setShowManage(true)}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all active:scale-[.98]"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                  >
                    <Plus size={13} />
                    添加门户标签
                  </button>
                </div>
              ) : portalFiltered.length === 0 ? (
                // 有标签但无匹配结果
                <div className="flex flex-col items-center justify-center gap-2 py-16">
                  <Search size={20} style={{ color: 'var(--text-tertiary)' }} />
                  <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>没有匹配的标签</p>
                </div>
              ) : (
                portalGroups.map((g) => (
                  <section key={g.group ?? '__ungrouped__'} className="mb-5">
                    <h3 className="text-[11px] font-medium mb-2" style={{ color: 'var(--text-tertiary)' }}>
                      {g.group ?? '未分组'}
                    </h3>
                    <div className="grid gap-2.5" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(210px, 1fr))' }}>
                      {g.items.map((t) => {
                        // 「已打开」= iframe 当前真的挂载着（LRU 窗口内）；关闭后会摘掉这个标记
                        const opened = activatedIds.includes(t.id);
                        return (
                          <div
                            key={t.id}
                            className="group flex items-center gap-2.5 p-3 rounded-xl transition-colors"
                            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                          >
                            {/* 卡片主体：点击即在顶栏切到该标签并加载其 iframe */}
                            <button
                              onClick={() => openTab(t.id)}
                              className="flex-1 min-w-0 flex items-center gap-2.5 text-left"
                              title={t.url}
                            >
                              <span
                                className="shrink-0 w-8 h-8 rounded-lg flex items-center justify-center"
                                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)' }}
                              >
                                <TabIcon icon={t.icon} size={15} />
                              </span>
                              <span className="min-w-0 flex-1">
                                <span className="block text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>{t.label}</span>
                                <span className="flex items-center gap-1.5 mt-0.5 min-w-0">
                                  <span className="text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }}>{subtitleOf(t.url)}</span>
                                  {opened && (
                                    <span
                                      className="shrink-0 text-[9px] px-1 py-px rounded-full"
                                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)' }}
                                    >
                                      已打开
                                    </span>
                                  )}
                                </span>
                              </span>
                            </button>
                            {/* 在浏览器打开：悬停卡片时才显现 */}
                            <button
                              onClick={() => void openInBrowser(t.url)}
                              className="shrink-0 p-1.5 rounded-md opacity-0 group-hover:opacity-100 transition-opacity"
                              style={{ color: 'var(--text-secondary)' }}
                              title="在浏览器中打开"
                            >
                              <ExternalLink size={13} />
                            </button>
                          </div>
                        );
                      })}
                    </div>
                  </section>
                ))
              )}
            </div>
          </div>
        )}

        {/* 白屏降级提示条：8 秒内未 onload（可能被 X-Frame-Options / CSP 拒绝内嵌）时就地给出出口。
            无法可靠探测拒绝（被拒 iframe 多数仍触发 onload），故用超时近似 + 手动"在浏览器打开"。 */}
        {showEmbedTip && active && (
          <div
            className="absolute top-0 left-0 right-0 z-10 flex items-center gap-2 px-3 py-2 text-[11px]"
            style={{
              backgroundColor: 'var(--status-warning-bg, #FEF3C7)',
              borderBottom: '1px solid var(--border)',
            }}
          >
            <AlertTriangle size={13} className="shrink-0" style={{ color: 'var(--status-warning, #F59E0B)' }} />
            <span className="flex-1 leading-relaxed" style={{ color: 'var(--text-primary)' }}>
              页面可能不允许被内嵌（X-Frame-Options / CSP）。你可以点这里在浏览器中打开。
            </span>
            <button
              onClick={() => void openInBrowser(active.url)}
              className="pd-btn shrink-0 flex items-center gap-1 px-2 py-1 rounded text-[11px] transition-all active:scale-[.98]"
              style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            >
              <ExternalLink size={12} />
              在浏览器中打开
            </button>
            <button
              onClick={() => activeKey && dismissEmbedTip(activeKey)}
              className="pd-btn shrink-0 p-1 rounded transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="关闭提示"
            >
              <X size={13} />
            </button>
          </div>
        )}
        {tabs.length === 0 && !active && !isPortal && !showManage && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3">
            <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>暂无门户标签</p>
            <button
              onClick={() => setShowManage(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all active:scale-[.98]"
              style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            >
              <Settings2 size={13} />
              打开标签管理
            </button>
          </div>
        )}
        {tabs.length > 0 && !active && !isPortal && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2">
            <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>在顶部选择标签页</p>
          </div>
        )}
      </div>

      {/* 标签管理覆盖层：叠加在主视图之上，主树（iframe）保持挂载 */}
      {showManage && (
        <div className="absolute inset-0 z-30 flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-content)' }}>
          <div className="flex items-center gap-2 px-3 h-9 shrink-0" style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
            <button
              onClick={() => setShowManage(false)}
              className="flex items-center gap-1 px-2 py-1 rounded-md text-xs shrink-0 transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="返回标签页"
            >
              <X size={13} />
              返回
            </button>
            <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>门户标签管理</span>
          </div>
          <div className="flex-1 overflow-y-auto px-5 py-4">
            <CustomTabsSettings />
          </div>
        </div>
      )}
    </div>
  );
}
