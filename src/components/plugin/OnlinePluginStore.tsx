/**
 * OnlinePluginStore — 在线插件商店面板
 *
 * 浏览、安装、更新在线插件。
 * 与本地已安装插件对比版本，标记状态。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Store, Download, RefreshCw, Search, Package, Star, User, HardDrive, BookOpen, X, AlertCircle, CheckCircle, Loader2 } from 'lucide-react';

// ── 友好错误提示 ──

/** 将后端返回的内部错误信息转换为用户友好的提示文字 */
function formatUserFriendlyError(raw: string): { title: string; detail: string } {
  if (raw.includes('连接失败') || raw.includes('连接超时') || raw.includes('请求超时')) {
    return { title: '网络连接失败', detail: '无法连接到插件商店服务器，请检查网络连接后重试。' };
  }
  if (raw.includes('HTTP 5') || raw.includes('HTTP 502') || raw.includes('HTTP 503')) {
    return { title: '商店服务器繁忙', detail: '插件商店暂时无法响应，请稍后重试。' };
  }
  if (raw.includes('HTTP 4') || raw.includes('HTTP 404')) {
    return { title: '插件数据异常', detail: '未找到对应的插件文件，请刷新后重试。' };
  }
  if (raw.includes('清单验证失败')) {
    return { title: '插件验证未通过', detail: '该插件不符合安全规范，已拒绝安装。' };
  }
  if (raw.includes('解析索引失败') || raw.includes('解析 manifest')) {
    return { title: '插件数据异常', detail: '插件数据格式错误，请刷新后重试。' };
  }
  if (raw.includes('未找到插件')) {
    return { title: '插件不存在', detail: '该插件在商店中已不存在，请刷新列表。' };
  }
  if (raw.includes('写入') || raw.includes('创建目录') || raw.includes('清理')) {
    return { title: '安装失败', detail: '无法写入插件文件，请检查磁盘空间和权限。' };
  }
  if (raw.includes('重试') && raw.includes('失败')) {
    return { title: '商店服务器繁忙', detail: '多次尝试后仍无法连接，请稍后重试。' };
  }
  if (raw.includes('锁定失败')) {
    return { title: '系统繁忙', detail: '插件系统暂时无法响应，请稍后重试。' };
  }
  const cleaned = raw.replace(/https?:\/\/[^\s]+/g, '').replace(/\s+/g, ' ').trim();
  if (cleaned.length > 0 && cleaned !== raw) {
    return { title: '安装失败', detail: cleaned };
  }
  return { title: '安装失败', detail: '发生未知错误，请稍后重试。' };
}

// ── 类型定义 ──

interface OnlinePluginInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAppVersion: string;
  baseUrl: string;
  icon?: string;
  size?: string;
  readme?: string;
}

interface LocalPluginVersion {
  id: string;
  version: string;
}

type PluginStatus = 'installed' | 'update-available' | 'not-installed' | 'installing';

// ── 子组件：插件卡片 ──

function PluginCard({
  plugin,
  status,
  installing,
  onInstall,
}: {
  plugin: OnlinePluginInfo;
  status: PluginStatus;
  installing: boolean;
  onInstall: (id: string) => void;
}) {
  const [iconError, setIconError] = useState(false);

  const isInstalled = status === 'installed';
  const isInstalling = status === 'installing';

  return (
    <div
      className="plugin-card"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
        borderRadius: 'var(--radius-lg)',
        padding: '16px',
        transition: 'box-shadow 0.15s ease, transform 0.15s ease, border-color 0.15s ease',
        cursor: isInstalled ? 'default' : 'pointer',
        position: 'relative',
        overflow: 'hidden',
      }}
      onClick={() => !isInstalled && !isInstalling && onInstall(plugin.id)}
      onMouseEnter={(e) => {
        if (!isInstalled) {
          e.currentTarget.style.boxShadow = 'var(--shadow-md)';
          e.currentTarget.style.borderColor = 'var(--accent)';
          e.currentTarget.style.transform = 'translateY(-1px)';
        }
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.boxShadow = 'none';
        e.currentTarget.style.borderColor = 'var(--border)';
        e.currentTarget.style.transform = 'translateY(0)';
      }}
    >
      {/* 顶部：图标 + 名称 + 版本 + 操作按钮 */}
      <div className="flex items-start gap-3 mb-3">
        {/* 图标 */}
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: 'var(--radius-md)',
            backgroundColor: 'var(--bg-tertiary)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
            overflow: 'hidden',
            border: '1px solid var(--border)',
          }}
        >
          {plugin.icon && !iconError ? (
            <img
              src={plugin.icon}
              alt={plugin.name}
              style={{ width: '100%', height: '100%', objectFit: 'cover' }}
              onError={() => setIconError(true)}
            />
          ) : (
            <Package size={18} style={{ color: 'var(--text-tertiary)' }} />
          )}
        </div>

        {/* 名称和版本 */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <h4
              style={{
                fontSize: 'var(--fs-13)',
                fontWeight: 600,
                color: 'var(--text-primary)',
                margin: 0,
                whiteSpace: 'nowrap',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
              }}
            >
              {plugin.name}
            </h4>
            <span
              style={{
                fontSize: 'var(--fs-10)',
                color: 'var(--text-tertiary)',
                whiteSpace: 'nowrap',
                fontFamily: 'var(--font-mono)',
              }}
            >
              v{plugin.version}
            </span>
          </div>
        </div>

        {/* 操作按钮（图标旁边） */}
        <div className="shrink-0">
          {isInstalled ? (
            <span
              style={{
                fontSize: 'var(--fs-10)',
                color: '#10B981',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 3,
                padding: '3px 10px',
                borderRadius: 'var(--radius-full)',
                backgroundColor: 'rgba(16,185,129,0.1)',
                fontWeight: 500,
              }}
            >
              <CheckCircle size={10} />
              已安装
            </span>
          ) : (
            <button
              onClick={(e) => { e.stopPropagation(); onInstall(plugin.id); }}
              disabled={isInstalling}
              className="pd-btn-primary"
              style={{
                fontSize: 'var(--fs-11)',
                padding: '5px 14px',
                borderRadius: 'var(--radius-md)',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 5,
                fontWeight: 500,
              }}
            >
              {isInstalling ? (
                <><Loader2 size={11} className="pd-animate-spin" /></>
              ) : status === 'update-available' ? (
                <><Download size={11} /> 更新</>
              ) : (
                <><Download size={11} /> 安装</>
              )}
            </button>
          )}
        </div>
      </div>

      {/* 描述 */}
      <p
        style={{
          fontSize: 'var(--fs-11)',
          color: 'var(--text-secondary)',
          lineHeight: 1.5,
          margin: '0 0 12px 0',
          display: '-webkit-box',
          WebkitLineClamp: 2,
          WebkitBoxOrient: 'vertical',
          overflow: 'hidden',
        }}
      >
        {plugin.description}
      </p>

      {/* 元信息 */}
      <div className="flex items-center gap-3" style={{ flexWrap: 'wrap' }}>
        <div className="flex items-center gap-1">
          <User size={10} style={{ color: 'var(--text-tertiary)' }} />
          <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>{plugin.author}</span>
        </div>
        {plugin.size && (
          <div className="flex items-center gap-1">
            <HardDrive size={10} style={{ color: 'var(--text-tertiary)' }} />
            <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>{plugin.size}</span>
          </div>
        )}
        <div className="flex items-center gap-1">
          <Star size={10} style={{ color: 'var(--text-tertiary)' }} />
          <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>v{plugin.minAppVersion}+</span>
        </div>
        {plugin.readme && (
          <div className="flex items-center gap-1">
            <BookOpen size={10} style={{ color: 'var(--text-tertiary)' }} />
            <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>文档</span>
          </div>
        )}
      </div>
    </div>
  );
}

// ── 子组件：加载骨架屏 ──

function SkeletonCard() {
  return (
    <div
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
        borderRadius: 'var(--radius-lg)',
        padding: '16px',
      }}
    >
      <div className="flex items-start gap-3 mb-3">
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: 'var(--radius-md)',
            backgroundColor: 'var(--bg-tertiary)',
          }}
        />
        <div className="flex-1">
          <div
            style={{
              height: 14,
              width: '60%',
              backgroundColor: 'var(--bg-tertiary)',
              borderRadius: 'var(--radius-sm)',
              marginBottom: 8,
            }}
          />
          <div
            style={{
              height: 10,
              width: '30%',
              backgroundColor: 'var(--bg-tertiary)',
              borderRadius: 'var(--radius-sm)',
            }}
          />
        </div>
      </div>
      <div
        style={{
          height: 10,
          width: '100%',
          backgroundColor: 'var(--bg-tertiary)',
          borderRadius: 'var(--radius-sm)',
          marginBottom: 6,
        }}
      />
      <div
        style={{
          height: 10,
          width: '70%',
          backgroundColor: 'var(--bg-tertiary)',
          borderRadius: 'var(--radius-sm)',
          marginBottom: 12,
        }}
      />
      <div className="flex gap-2">
        <div
          style={{
            height: 10,
            width: 60,
            backgroundColor: 'var(--bg-tertiary)',
            borderRadius: 'var(--radius-sm)',
          }}
        />
        <div
          style={{
            height: 10,
            width: 50,
            backgroundColor: 'var(--bg-tertiary)',
            borderRadius: 'var(--radius-sm)',
          }}
        />
      </div>
    </div>
  );
}

// ── 主组件 ──

export const OnlinePluginStore: React.FC<{
  onClose?: () => void;
  searchQuery?: string;
  onSearchChange?: (q: string) => void;
}> = ({ onClose, searchQuery: externalSearchQuery, onSearchChange }) => {
  // 商店容器样式：允许纵向滚动
  const containerStyle: React.CSSProperties = {};
  const [plugins, setPlugins] = useState<OnlinePluginInfo[]>([]);
  const [localVersions, setLocalVersions] = useState<Map<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [internalSearchQuery, setInternalSearchQuery] = useState('');
  const searchQuery = externalSearchQuery !== undefined ? externalSearchQuery : internalSearchQuery;
  const setSearchQuery = onSearchChange || setInternalSearchQuery;
  const [source, setSource] = useState<string>('');

  const fetchPlugins = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [indexResult, localResult] = await Promise.all([
        invoke<{ plugins: OnlinePluginInfo[]; updated_at: string; source: string }>(
          'plugin_store_fetch_index',
          { forceRefresh: false },
        ),
        invoke<LocalPluginVersion[]>('plugin_store_get_local_versions'),
      ]);

      setPlugins(indexResult.plugins);
      setSource(indexResult.source);

      const localMap = new Map<string, string>();
      for (const p of localResult) {
        localMap.set(p.id, p.version);
      }
      setLocalVersions(localMap);
    } catch (err) {
      const friendly = formatUserFriendlyError(String(err));
      setError(friendly.title + '：' + friendly.detail);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPlugins();
  }, [fetchPlugins]);

  const installPlugin = async (pluginId: string) => {
    setInstallingId(pluginId);
    setError(null);
    try {
      await invoke('plugin_store_install', { pluginId });
      const localResult = await invoke<LocalPluginVersion[]>('plugin_store_get_local_versions');
      const localMap = new Map<string, string>();
      for (const p of localResult) {
        localMap.set(p.id, p.version);
      }
      setLocalVersions(localMap);
    } catch (err) {
      const friendly = formatUserFriendlyError(String(err));
      setError(friendly.title + '：' + friendly.detail);
    } finally {
      setInstallingId(null);
    }
  };

  const getPluginStatus = (plugin: OnlinePluginInfo): PluginStatus => {
    if (installingId === plugin.id) return 'installing';
    const localVersion = localVersions.get(plugin.id);
    if (!localVersion) return 'not-installed';
    if (localVersion !== plugin.version) return 'update-available';
    return 'installed';
  };

  const filteredPlugins = plugins.filter((p) => {
    if (!searchQuery) return true;
    const q = searchQuery.toLowerCase();
    return (
      p.name.toLowerCase().includes(q) ||
      p.id.toLowerCase().includes(q) ||
      p.description.toLowerCase().includes(q) ||
      p.author.toLowerCase().includes(q)
    );
  });

  return (
    <div className="px-4 pb-4">
      {/* 错误提示 */}
      {error && (
        <div
          className="flex items-center gap-2 px-3 py-2 rounded-lg mb-4"
          style={{
            backgroundColor: 'rgba(239,68,68,0.08)',
            border: '1px solid rgba(239,68,68,0.2)',
          }}
        >
          <AlertCircle size={14} style={{ color: '#EF4444', flexShrink: 0 }} />
          <span style={{ fontSize: 'var(--fs-11)', color: '#EF4444', flex: 1 }}>{error}</span>
          <button
            onClick={fetchPlugins}
            className="pd-btn"
            style={{
              fontSize: 'var(--fs-10)',
              padding: '2px 8px',
              borderRadius: 'var(--radius-sm)',
              backgroundColor: 'rgba(239,68,68,0.1)',
              color: '#EF4444',
            }}
          >
            重试
          </button>
        </div>
      )}

      {/* 加载中：骨架屏 */}
      {loading && !error && (
        <div className="grid gap-3" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))' }}>
          {[1, 2, 3, 4].map((i) => <SkeletonCard key={i} />)}
        </div>
      )}

      {/* 空状态 */}
      {!loading && !error && filteredPlugins.length === 0 && (
        <div
          className="flex flex-col items-center justify-center py-12"
          style={{ color: 'var(--text-tertiary)' }}
        >
          <Package size={32} style={{ opacity: 0.3, marginBottom: 12 }} />
          <p style={{ fontSize: 'var(--fs-13)', marginBottom: 4 }}>
            {searchQuery ? '没有匹配的插件' : '商店暂无可用的插件'}
          </p>
          <p style={{ fontSize: 'var(--fs-11)' }}>
            {searchQuery ? '请尝试其他关键词' : '请稍后再来查看'}
          </p>
        </div>
      )}

      {/* 插件列表：网格布局 */}
      {!loading && !error && filteredPlugins.length > 0 && (
        <div
          className="grid gap-3"
          style={{
            gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))',
          }}
        >
          {filteredPlugins.map((plugin) => (
            <PluginCard
              key={plugin.id}
              plugin={plugin}
              status={getPluginStatus(plugin)}
              installing={installingId === plugin.id}
              onInstall={installPlugin}
            />
          ))}
        </div>
      )}

      {/* 底部统计 */}
      {!loading && !error && plugins.length > 0 && (
        <div
          className="flex items-center justify-between mt-4 pt-3"
          style={{ borderTop: '1px solid var(--border)' }}
        >
          <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
            共 {plugins.length} 个插件{searchQuery ? `，筛选后 ${filteredPlugins.length} 个` : ''}
          </span>
        </div>
      )}
    </div>
  );
};
