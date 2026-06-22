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
import { Store, Download, RefreshCw, Search, Package, Shield, User, HardDrive, X, AlertCircle, CheckCircle, Loader2 } from 'lucide-react';
import { PluginReadmeDialog } from './PluginReadmeDialog';

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
  onReadme,
}: {
  plugin: OnlinePluginInfo;
  status: PluginStatus;
  installing: boolean;
  onInstall: (id: string) => void;
  onReadme: (plugin: OnlinePluginInfo) => void;
}) {
  const [iconError, setIconError] = useState(false);

  const isInstalled = status === 'installed';
  const isInstalling = status === 'installing';

  return (
    <div
      className="px-3 py-2.5 rounded-lg transition-colors"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
        cursor: isInstalled ? 'default' : 'pointer',
      }}
      onClick={() => !isInstalled && !isInstalling && onInstall(plugin.id)}
      onMouseEnter={(e) => {
        if (!isInstalled) {
          e.currentTarget.style.borderColor = 'var(--accent)';
        }
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.borderColor = 'var(--border)';
      }}
    >
      {/* 顶部行：图标 + 名称/版本 + 操作按钮 */}
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0 flex-1">
          {/* 图标 */}
          <div
            style={{
              width: 20,
              height: 20,
              borderRadius: '4px',
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
              <Package size={10} style={{ color: 'var(--text-tertiary)' }} />
            )}
          </div>
          <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
            {plugin.name}
          </span>
          <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            v{plugin.version}
          </span>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {isInstalled ? (
            <>
              <span
                style={{
                  fontSize: '10px',
                  color: '#10B981',
                  padding: '2px 8px',
                  borderRadius: '999px',
                  backgroundColor: 'rgba(16,185,129,0.1)',
                  fontWeight: 500,
                }}
              >
                已安装
              </span>
              <button
                onClick={(e) => { e.stopPropagation(); onReadme(plugin); }}
                className="pd-btn px-1.5 py-1 rounded text-[10px]"
                style={{ color: 'var(--accent)' }}
                title="查看 README"
              >
                📖
              </button>
            </>
          ) : (
            <button
              onClick={(e) => { e.stopPropagation(); onInstall(plugin.id); }}
              disabled={isInstalling}
              className="pd-btn px-2 py-1 rounded text-[10px] transition-all"
              style={{
                backgroundColor: 'var(--accent)',
                color: '#fff',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 3,
              }}
            >
              {isInstalling ? (
                <><Loader2 size={10} className="pd-animate-spin" /></>
              ) : status === 'update-available' ? (
                <><Download size={10} /> 更新</>
              ) : (
                <><Download size={10} /> 安装</>
              )}
            </button>
          )}
        </div>
      </div>

      {/* 分割线 */}
      <div className="my-1.5" style={{ height: '1px', backgroundColor: 'var(--border)' }} />

      {/* 描述 */}
      <p
        className="text-[10px] line-clamp-2"
        style={{
          color: 'var(--text-secondary)',
          margin: '0 0 4px 0',
          lineHeight: 1.5,
        }}
      >
        {plugin.description}
      </p>

      {/* 元信息：作者 + 大小 + 最小版本 */}
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-1">
          <User size={10} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>{plugin.author}</span>
        </div>
        {plugin.size && (
          <div className="flex items-center gap-1">
            <HardDrive size={10} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>{plugin.size}</span>
          </div>
        )}
        <div className="flex items-center gap-1">
          <Shield size={10} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>v{plugin.minAppVersion}+</span>
        </div>
      </div>
    </div>
  );
}

// ── 子组件：加载骨架屏 ──

function SkeletonCard() {
  return (
    <div
      className="px-3 py-2.5 rounded-lg"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 flex-1">
          <div
            style={{
              width: 20,
              height: 20,
              borderRadius: '4px',
              backgroundColor: 'var(--bg-tertiary)',
            }}
          />
          <div
            style={{
              height: 12,
              width: 100,
              backgroundColor: 'var(--bg-tertiary)',
              borderRadius: '2px',
            }}
          />
          <div
            style={{
              height: 10,
              width: 40,
              backgroundColor: 'var(--bg-tertiary)',
              borderRadius: '2px',
            }}
          />
        </div>
        <div
          style={{
            height: 22,
            width: 50,
            backgroundColor: 'var(--bg-tertiary)',
            borderRadius: '4px',
          }}
        />
      </div>
      <div className="my-1.5" style={{ height: '1px', backgroundColor: 'var(--border)' }} />
      <div className="flex items-center justify-between gap-2">
        <div
          style={{
            height: 10,
            width: '60%',
            backgroundColor: 'var(--bg-tertiary)',
            borderRadius: '2px',
          }}
        />
        <div
          style={{
            height: 10,
            width: 80,
            backgroundColor: 'var(--bg-tertiary)',
            borderRadius: '2px',
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
  /** 统计信息变更回调，用于父组件在标题行显示 */
  onStatsChange?: (total: number, filtered: number) => void;
}> = ({ onClose, searchQuery: externalSearchQuery, onSearchChange, onStatsChange }) => {
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
  const [readmePlugin, setReadmePlugin] = useState<OnlinePluginInfo | null>(null);

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

  useEffect(() => {
    fetchPlugins();
  }, [fetchPlugins]);

  // 统计信息变化时通知父组件
  useEffect(() => {
    if (!loading && !error) {
      onStatsChange?.(plugins.length, filteredPlugins.length);
    }
  }, [plugins.length, filteredPlugins.length, loading, error, onStatsChange]);

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



  return (
    <div>

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
        <div className="space-y-2">
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
          className="space-y-2"
        >
          {filteredPlugins.map((plugin) => (
            <PluginCard
              key={plugin.id}
              plugin={plugin}
              status={getPluginStatus(plugin)}
              installing={installingId === plugin.id}
              onInstall={installPlugin}
              onReadme={(p) => setReadmePlugin(p)}
            />
          ))}
        </div>
      )}

      {readmePlugin && (
        <PluginReadmeDialog
          basePath={readmePlugin.baseUrl}
          pluginName={readmePlugin.name}
          isRemote={true}
          onClose={() => setReadmePlugin(null)}
        />
      )}
    </div>
  );
};
