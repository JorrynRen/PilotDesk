import { useEffect, useMemo, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { useNavigate } from 'react-router-dom';
import { Globe, Plus, Settings2 } from 'lucide-react';
import { useCustomTabsStore } from '../../stores/customTabsStore';

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
 * 自定义标签页壳：固定在 MainLayout 的 TitleBar 与 StatusBar 之间，
 * 内容区渲染当前激活标签的 iframe（网络地址或本地 HTML）。
 */
export function CustomTabHost() {
  const { tabs, activeTabId, setActiveTab } = useCustomTabsStore();
  const navigate = useNavigate();
  const active = useMemo(() => tabs.find((t) => t.id === activeTabId) || null, [tabs, activeTabId]);
  // 本地路径时区分目录/文件，决定 iframe 走 dirindex 索引页还是 asset 文件协议
  const [activeIsDir, setActiveIsDir] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (active && !/^https?:\/\//i.test(active.url)) {
      invoke<boolean>('path_is_directory', { path: active.url })
        .then((isDir) => { if (!cancelled) setActiveIsDir(isDir); })
        .catch(() => { if (!cancelled) setActiveIsDir(false); });
    } else {
      setActiveIsDir(false);
    }
    return () => { cancelled = true; };
  }, [active?.id, active?.url]);

  if (tabs.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <div className="w-12 h-12 rounded-xl flex items-center justify-center" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
        </div>
        <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>暂无自定义标签</p>
        <button
          onClick={() => navigate('/settings?tab=customtabs')}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all active:scale-[.98]"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          <Settings2 size={13} />
          前往设置添加标签
        </button>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* 标签切换行 */}
      <div
        className="flex items-center gap-1 px-3 h-9 shrink-0 overflow-x-auto"
        style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        {tabs.map((t) => {
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
        <button
          onClick={() => navigate('/settings?tab=customtabs')}
          className="flex items-center gap-1 px-2 py-1 rounded-md text-xs shrink-0 transition-colors"
          style={{ color: 'var(--text-tertiary)' }}
          title="管理自定义标签"
        >
          <Plus size={13} />
        </button>
      </div>

      {/* iframe 区：壳内容 */}
      <div className="flex-1 relative" style={{ backgroundColor: '#fff' }}>
        {active ? (
          <iframe
            key={active.id}
            src={resolveFrameSrc(active.url, activeIsDir)}
            className="absolute inset-0 w-full h-full border-0"
            title={active.label}
            allowFullScreen
          />
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2">
            <Globe size={22} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>在顶部选择或管理自定义标签</p>
          </div>
        )}
      </div>
    </div>
  );
}
