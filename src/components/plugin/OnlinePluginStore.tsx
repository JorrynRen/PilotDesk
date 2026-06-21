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

// ── 类型定义 ──

interface OnlinePluginInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAppVersion: string;
  permissions: string[];
  baseUrl: string;
  icon?: string;
  size?: string;
  tags?: string[];
  createdAt?: string;
  updatedAt?: string;
}

interface LocalPluginVersion {
  id: string;
  version: string;
}

type PluginStatus = 'installed' | 'update-available' | 'not-installed' | 'installing';

// ── 组件 ──

export const OnlinePluginStore: React.FC<{ onClose?: () => void }> = ({ onClose }) => {
  const [plugins, setPlugins] = useState<OnlinePluginInfo[]>([]);
  const [localVersions, setLocalVersions] = useState<Map<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [source, setSource] = useState<string>('');

  // ── 获取插件列表 ──

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
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPlugins();
  }, [fetchPlugins]);

  // ── 安装插件 ──

  const installPlugin = async (pluginId: string) => {
    setInstallingId(pluginId);
    try {
      await invoke('plugin_store_install', { pluginId });
      // 刷新本地版本
      const localResult = await invoke<LocalPluginVersion[]>('plugin_store_get_local_versions');
      const localMap = new Map<string, string>();
      for (const p of localResult) {
        localMap.set(p.id, p.version);
      }
      setLocalVersions(localMap);
    } catch (err) {
      console.error('安装失败:', err);
    } finally {
      setInstallingId(null);
    }
  };

  // ── 获取插件状态 ──

  const getPluginStatus = (plugin: OnlinePluginInfo): PluginStatus => {
    const localVersion = localVersions.get(plugin.id);
    if (!localVersion) return 'not-installed';
    if (localVersion !== plugin.version) return 'update-available';
    return 'installed';
  };

  // ── 过滤 ──

  const filteredPlugins = plugins.filter((p) => {
    if (!searchQuery) return true;
    const q = searchQuery.toLowerCase();
    return (
      p.name.toLowerCase().includes(q) ||
      p.id.toLowerCase().includes(q) ||
      p.description.toLowerCase().includes(q) ||
      p.author.toLowerCase().includes(q) ||
      (p.tags && p.tags.some((t) => t.toLowerCase().includes(q)))
    );
  });

  // ── 渲染 ──

  return (
    <div className="online-plugin-store">
      {/* 头部 */}
      <div className="store-header">
        <h2>在线插件商店</h2>
        <div className="store-search">
          <input
            type="text"
            placeholder="搜索插件..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="search-input"
          />
        </div>
        <button onClick={fetchPlugins} disabled={loading} className="refresh-btn">
          {loading ? '加载中...' : '刷新'}
        </button>
      </div>

      {/* 来源信息 */}
      {source && (
        <div className="store-source">
          数据来源: {source === 'cdn' ? 'CDN' : 'GitHub Raw'}
        </div>
      )}

      {/* 错误提示 */}
      {error && (
        <div className="store-error">
          <p>加载失败: {error}</p>
          <button onClick={fetchPlugins}>重试</button>
        </div>
      )}

      {/* 加载中 */}
      {loading && !error && (
        <div className="store-loading">
          <p>正在加载插件列表...</p>
        </div>
      )}

      {/* 插件列表 */}
      {!loading && !error && (
        <div className="store-plugin-list">
          {filteredPlugins.length === 0 ? (
            <p className="store-empty">
              {searchQuery ? '没有匹配的插件' : '商店暂无可用的插件'}
            </p>
          ) : (
            filteredPlugins.map((plugin) => {
              const status = getPluginStatus(plugin);
              return (
                <div key={plugin.id} className="store-plugin-card">
                  <div className="plugin-card-header">
                    <h3>{plugin.name}</h3>
                    <span className="plugin-version">v{plugin.version}</span>
                  </div>
                  <p className="plugin-description">{plugin.description}</p>
                  <div className="plugin-meta">
                    <span className="plugin-author">作者: {plugin.author}</span>
                    {plugin.size && <span className="plugin-size">大小: {plugin.size}</span>}
                  </div>
                  {plugin.tags && plugin.tags.length > 0 && (
                    <div className="plugin-tags">
                      {plugin.tags.map((tag) => (
                        <span key={tag} className="plugin-tag">{tag}</span>
                      ))}
                    </div>
                  )}
                  <div className="plugin-actions">
                    {status === 'installed' && (
                      <span className="status-installed">已安装</span>
                    )}
                    {status === 'update-available' && (
                      <button
                        onClick={() => installPlugin(plugin.id)}
                        disabled={installingId === plugin.id}
                        className="btn-update"
                      >
                        {installingId === plugin.id ? '更新中...' : '更新'}
                      </button>
                    )}
                    {status === 'not-installed' && (
                      <button
                        onClick={() => installPlugin(plugin.id)}
                        disabled={installingId === plugin.id}
                        className="btn-install"
                      >
                        {installingId === plugin.id ? '安装中...' : '安装'}
                      </button>
                    )}
                  </div>
                </div>
              );
            })
          )}
        </div>
      )}
    </div>
  );
};
