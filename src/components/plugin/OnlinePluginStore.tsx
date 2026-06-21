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

// ── 友好错误提示 ──

/** 将后端返回的内部错误信息转换为用户友好的提示文字 */
function formatUserFriendlyError(raw: string): { title: string; detail: string } {
  // 网络连接类
  if (raw.includes('连接失败') || raw.includes('连接超时') || raw.includes('请求超时')) {
    return {
      title: '网络连接失败',
      detail: '无法连接到插件商店服务器，请检查网络连接后重试。',
    };
  }

  // HTTP 5xx 服务器错误
  if (raw.includes('HTTP 5') || raw.includes('HTTP 502') || raw.includes('HTTP 503')) {
    return {
      title: '商店服务器繁忙',
      detail: '插件商店暂时无法响应，请稍后重试。',
    };
  }

  // HTTP 4xx 客户端错误
  if (raw.includes('HTTP 4') || raw.includes('HTTP 404')) {
    return {
      title: '插件数据异常',
      detail: '未找到对应的插件文件，请刷新后重试。',
    };
  }

  // 清单验证失败
  if (raw.includes('清单验证失败')) {
    return {
      title: '插件验证未通过',
      detail: '该插件不符合安全规范，已拒绝安装。',
    };
  }

  // 解析失败
  if (raw.includes('解析索引失败') || raw.includes('解析 manifest')) {
    return {
      title: '插件数据异常',
      detail: '插件数据格式错误，请刷新后重试。',
    };
  }

  // 未找到插件
  if (raw.includes('未找到插件')) {
    return {
      title: '插件不存在',
      detail: '该插件在商店中已不存在，请刷新列表。',
    };
  }

  // 目录/文件写入失败
  if (raw.includes('写入') || raw.includes('创建目录') || raw.includes('清理')) {
    return {
      title: '安装失败',
      detail: '无法写入插件文件，请检查磁盘空间和权限。',
    };
  }

  // 重试耗尽
  if (raw.includes('重试') && raw.includes('失败')) {
    return {
      title: '商店服务器繁忙',
      detail: '多次尝试后仍无法连接，请稍后重试。',
    };
  }

  // 锁定失败
  if (raw.includes('锁定失败')) {
    return {
      title: '系统繁忙',
      detail: '插件系统暂时无法响应，请稍后重试。',
    };
  }

  // 兜底：提取关键信息，隐藏 URL 和内部细节
  const cleaned = raw
    .replace(/https?:\/\/[^\s]+/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  
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
      const friendly = formatUserFriendlyError(String(err));
      setError(friendly.title + '：' + friendly.detail);
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
    setError(null);
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
      const friendly = formatUserFriendlyError(String(err));
      setError(friendly.title + '：' + friendly.detail);
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
      p.author.toLowerCase().includes(q)
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
                  {/* tags 字段已从索引中移除，如需展示请从 manifest.json 读取 */}
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
