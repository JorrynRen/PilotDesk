import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { Globe, Plus, X, Settings2 } from 'lucide-react';
import { useCustomTabsStore } from '../../stores/customTabsStore';
import { useTerminal } from '../../TerminalManager';
import { CustomTabsSettings } from '../settings/CustomTabsSettings';

/** 目录索引协议地址：?path=<encodeURIComponent(绝对路径)>，由后端动态生成文件列表页 */
const DIRINDEX_BASE = 'http://dirindex.localhost/';

/** 把标签 url 解析为 iframe src：
 *  网络地址(http/https)直接使用；本地目录走 dirindex 协议生成索引页；本地文件走 asset 协议 */
function resolveFrameSrc(url: string, isDir: boolean): string {
  if (/^https?:\/\//i.test(url)) return url;
  if (isDir) return `${DIRINDEX_BASE}?path=${encodeURIComponent(url)}`;
  return convertFileSrc(url);
}

/**
 * 自定义标签页壳：固定在 MainLayout 的 TitleBar 与 StatusBar 之间。
 *
 * 生命周期模型：
 * - CustomTabHost 常挂载（App 以 CSS 显隐），主视图切换不会卸载 iframe；
 * - 每个标签首次被激活时才创建 iframe，此后切换仅 display 显隐，网页状态保留；
 * - 顶栏「＋」打开「标签管理」覆盖层：管理页以 absolute 覆盖叠加在主视图之上，
 *   主树（含全部 iframe）保持挂载，关闭覆盖层不会导致内容重新加载；
 * - 「＋」右侧关闭按钮：关闭当前激活标签 = 卸载其 iframe 并从顶栏移除（配置保留）；
 *   若关闭后没有任何标签页实例，自动回退到会话路由。
 * 「已关闭」为运行时状态，离开自定义模式即复位——下次进入恢复全部标签为打开。
 */
export function CustomTabHost() {
  const { tabs, activeTabId, setActiveTab } = useCustomTabsStore();
  const { viewMode, setMode } = useTerminal();

  // 标签管理覆盖层（覆盖叠加在主视图上，不替换/卸载主树）
  const [showManage, setShowManage] = useState(false);

  // 运行时「已关闭」标签集合（不影响持久化配置；离开自定义模式时复位）
  const [closedIds, setClosedIds] = useState<string[]>([]);
  // 已创建过 iframe 的标签（keep-alive：首次激活时挂载一次，之后常驻）
  const [activatedIds, setActivatedIds] = useState<string[]>([]);
  // 本地路径目录判定结果缓存：url -> isDir（http 无需判定）
  const [dirByUrl, setDirByUrl] = useState<Record<string, boolean>>({});

  const closedSet = useMemo(() => new Set(closedIds), [closedIds]);
  // 顶栏仅展示未关闭标签
  const openTabs = useMemo(() => tabs.filter((t) => !closedSet.has(t.id)), [tabs, closedSet]);
  const active = useMemo(() => tabs.find((t) => t.id === activeTabId) || null, [tabs, activeTabId]);

  // 离开自定义模式：复位运行时关闭状态，下次进入全部标签恢复为打开
  const prevViewRef = useRef(viewMode);
  useEffect(() => {
    const prev = prevViewRef.current;
    prevViewRef.current = viewMode;
    if (prev === 'custom' && viewMode !== 'custom') {
      setClosedIds([]);
    }
  }, [viewMode]);

  // 同步设置中的删除：被移除配置的标签清出运行时状态，避免幽灵 chip / iframe
  useEffect(() => {
    const valid = new Set(tabs.map((t) => t.id));
    setClosedIds((prev) => prev.filter((id) => valid.has(id)));
    setActivatedIds((prev) => prev.filter((id) => valid.has(id)));
  }, [tabs]);

  // 当前激活标签（未关闭）首次出现时挂载其 iframe
  useEffect(() => {
    if (!active || closedSet.has(active.id)) return;
    setActivatedIds((prev) => (prev.includes(active.id) ? prev : [...prev, active.id]));
    // 依赖 active.url：修改地址后保证 iframe 以新地址重建（src 变化本身会触发加载）
  }, [active?.id, active?.url, closedSet]);

  // 本地路径判定：首次需要时异步检测目录/文件，结果按 url 缓存
  useEffect(() => {
    if (!active || /^https?:\/\//i.test(active.url)) return;
    if (dirByUrl[active.url] !== undefined) return;
    let cancelled = false;
    invoke<boolean>('path_is_directory', { path: active.url })
      .then((isDir) => { if (!cancelled) setDirByUrl((m) => ({ ...m, [active.url]: isDir })); })
      .catch(() => { if (!cancelled) setDirByUrl((m) => ({ ...m, [active.url]: false })); });
    return () => { cancelled = true; };
  }, [active?.id, active?.url, dirByUrl]);

  const closeActive = useCallback(() => {
    if (!active) return;
    const closingId = active.id;
    // 关闭后剩余打开标签数：当前激活必在 openTabs 内
    const remaining = openTabs.length - 1;
    setClosedIds((prev) => (prev.includes(closingId) ? prev : [...prev, closingId]));
    setActivatedIds((prev) => prev.filter((id) => id !== closingId));
    if (remaining > 0) {
      const nextOpen = tabs.filter((t) => t.id !== closingId && !closedSet.has(t.id));
      setActiveTab(nextOpen[0]?.id ?? null);
    } else {
      // 没有任何标签页实例：直接回退会话路由（关闭状态随后由离开 effect 复位）
      setActiveTab(null);
      setMode('session');
    }
  }, [active, tabs, openTabs, closedSet, setActiveTab, setMode]);

  // 已激活且未关闭、src 就绪的标签 → 渲染为常驻 iframe。
  // 网络地址无需目录判定；本地路径等 dirByUrl 结果返回后再挂载，避免 src 切换触发二次加载。
  const frames = tabs.filter(
    (t) =>
      activatedIds.includes(t.id) &&
      !closedSet.has(t.id) &&
      (/^https?:\/\//i.test(t.url) || dirByUrl[t.url] !== undefined)
  );

  return (
    <div className="flex-1 flex flex-col overflow-hidden relative">
      {/* 标签切换行 */}
      <div
        className="flex items-center gap-1 px-3 h-9 shrink-0 overflow-x-auto"
        style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        {openTabs.map((t) => {
          const isActive = t.id === activeTabId;
          return (
            <button
              key={t.id}
              onClick={() => setActiveTab(t.id)}
              className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs shrink-0 transition-colors"
              style={{
                backgroundColor: isActive ? 'var(--accent)' : 'transparent',
                color: isActive ? '#fff' : 'var(--text-secondary)',
              }}
              title={t.url}
            >
              <Globe size={11} />
              {t.label}
            </button>
          );
        })}
        {openTabs.length === 0 && (
          <span className="text-[11px] px-1" style={{ color: 'var(--text-tertiary)' }}>没有打开的标签</span>
        )}

        <div className="flex-1" />

        {/* 「＋」：打开标签管理覆盖层（主视图保持挂载，关闭后内容不重载） */}
        <button
          onClick={() => setShowManage(true)}
          className="flex items-center px-1.5 py-1 rounded-md text-xs shrink-0 transition-colors"
          style={{ color: 'var(--text-secondary)' }}
          title="管理自定义标签"
        >
          <Plus size={14} />
        </button>

        {/* 关闭按钮（「＋」右侧）：关闭当前激活标签，仅卸载 iframe，配置保留 */}
        <button
          onClick={closeActive}
          disabled={!active}
          className="flex items-center px-1.5 py-1 rounded-md text-xs shrink-0 transition-colors disabled:opacity-30"
          style={{ color: 'var(--text-secondary)' }}
          title={active ? `关闭「${active.label}」（仅卸载，可重新打开）` : '没有可关闭的标签'}
        >
          <X size={13} />
        </button>
      </div>

      {/* iframe 区：keep-alive 常驻层，切换仅显隐 */}
      <div className="flex-1 relative" style={{ backgroundColor: '#fff' }}>
        {frames.map((t) => (
          <iframe
            key={t.id}
            src={resolveFrameSrc(t.url, !!dirByUrl[t.url])}
            className="absolute inset-0 w-full h-full border-0"
            style={{ display: t.id === activeTabId ? 'block' : 'none' }}
            title={t.label}
            allowFullScreen
          />
        ))}
        {tabs.length === 0 && !active && !showManage && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3">
            <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>暂无自定义标签</p>
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
        {openTabs.length > 0 && !active && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2">
            <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>在顶部选择标签页</p>
          </div>
        )}
      </div>

      {/* 标签管理覆盖层：叠加在主视图之上，主树（iframe）保持挂载 */}
      {showManage && (
        <div className="absolute inset-0 z-30 flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
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
            <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>自定义标签管理</span>
          </div>
          <div className="flex-1 overflow-y-auto px-5 py-4">
            <CustomTabsSettings />
          </div>
        </div>
      )}
    </div>
  );
}
