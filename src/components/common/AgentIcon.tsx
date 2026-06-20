/**
 * AgentIcon — Agent 图标渲染组件
 *
 * 支持三种图标来源：
 * 1. "file:filename.ico" → 调用 Rust read_agent_icon 读取内置图标文件
 * 2. "https://..." 或 "http://..." → 直接渲染网络图片
 * 3. Emoji 或文本字符 → 直接渲染文本
 */

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface AgentIconProps {
  icon?: string;
  size?: number;
  className?: string;
  fallback?: React.ReactNode;
}

export function AgentIcon({ icon, size = 14, className = '', fallback }: AgentIconProps) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!icon) {
      setDataUrl(null);
      setFailed(false);
      return;
    }

    // file: 前缀 → 调用 Rust 命令读取内置图标
    if (icon.startsWith('file:')) {
      const fileName = icon.slice(5);
      let cancelled = false;
      setDataUrl(null);
      setFailed(false);

      invoke<string>('read_agent_icon', { iconName: fileName })
        .then((url) => {
          if (!cancelled) { console.log('[AgentIcon] Loaded icon:', icon, 'url length:', url.length); setDataUrl(url); }
        })
        .catch((err) => {
          if (!cancelled) {
            console.warn('[AgentIcon] Failed to read icon:', JSON.stringify(err));
            setFailed(true);
          }
        });

      return () => { cancelled = true; };
    }

    // 非 file: 前缀，重置为文本模式
    setDataUrl(null);
    setFailed(false);
  }, [icon]);

  // file: 前缀 — 已加载完成，显示图片
  if (icon?.startsWith('file:') && dataUrl && !failed) {
    return (
      <img
        src={dataUrl}
        alt=""
        width={size}
        height={size}
        className={className}
        style={{ objectFit: 'contain', verticalAlign: 'middle' }}
        onError={() => setFailed(true)}
      />
    );
  }

  // file: 前缀 — 加载中或失败，显示 fallback
  if (icon?.startsWith('file:')) {
    return <>{fallback}</>;
  }

  // 网络图片
  if (icon && (icon.startsWith('http://') || icon.startsWith('https://'))) {
    return (
      <img
        src={icon}
        alt=""
        width={size}
        height={size}
        className={className}
        style={{ objectFit: 'contain', verticalAlign: 'middle' }}
        onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
      />
    );
  }

  // 文本模式（Emoji 或字符）
  if (icon) {
    return <span style={{ fontSize: size, lineHeight: 1 }}>{icon}</span>;
  }

  // 无图标
  return null;
}
