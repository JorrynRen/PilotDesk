/**
 * PluginIcon — 插件图标渲染组件
 *
 * 渲染流程：
 * 1. parsePluginIcon(icon) 同步解析类型
 * 2. 网络地址 → 直接渲染 <img>
 * 3. 插件本地路径 → 调用 Rust 命令读取文件返回 base64 data URL
 * 4. 空/未定义 → 显示默认图标 📦
 */

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { parsePluginIcon } from '../../utils/pluginIcon';

interface PluginIconProps {
  icon?: string;
  pluginId: string;
  size?: number;
}

export function PluginIcon({ icon, pluginId, size = 14 }: PluginIconProps) {
  const [loadFailed, setLoadFailed] = useState(false);
  const [localSrc, setLocalSrc] = useState<string | null>(null);
  const [localLoading, setLocalLoading] = useState(false);

  const parsed = parsePluginIcon(icon);

  // 本地路径：通过 Rust IPC 读取文件
  useEffect(() => {
    if (parsed.type !== 'local' || !parsed.value) return;

    let cancelled = false;
    setLocalLoading(true);

    invoke<string>('plugin_read_icon_file', {
      pluginId,
      iconPath: parsed.value,
    })
      .then((dataUrl) => {
        if (!cancelled) {
          setLocalSrc(dataUrl);
          setLocalLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          console.warn('[PluginIcon] Failed to read local icon:', err);
          setLocalLoading(false);
        }
      });

    return () => { cancelled = true; };
  }, [parsed.type, parsed.value, pluginId]);

  // 网络图片
  if (parsed.type === 'network' && parsed.value && !loadFailed) {
    return (
      <img
        src={parsed.value}
        alt=""
        width={size}
        height={size}
        className="inline-block object-contain shrink-0 rounded-sm"
        style={{ verticalAlign: 'middle' }}
        onError={() => setLoadFailed(true)}
      />
    );
  }

  // 本地图片（已加载完成）
  if (parsed.type === 'local' && localSrc && !loadFailed) {
    return (
      <img
        src={localSrc}
        alt=""
        width={size}
        height={size}
        className="inline-block object-contain shrink-0 rounded-sm"
        style={{ verticalAlign: 'middle' }}
        onError={() => setLoadFailed(true)}
      />
    );
  }

  // 本地图片（加载中）
  if (parsed.type === 'local' && localLoading) {
    return <span style={{ fontSize: size, opacity: 0.4 }}>📦</span>;
  }

  // 文本图标（emoji、文字符号等）
  if (parsed.type === 'text' && parsed.value) {
    return <span style={{ fontSize: size, lineHeight: 1 }}>{parsed.value}</span>;
  }

  // 文本模式或图片加载失败：显示默认图标
  return <span style={{ fontSize: size }}>📦</span>;
}
