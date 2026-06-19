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
}

export function AgentIcon({ icon, size = 14, className = '' }: AgentIconProps) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!icon) {
      setDataUrl(null);
      setLoading(false);
      setFailed(false);
      return;
    }

    // file: 前缀 → 调用 Rust 命令读取内置图标
    if (icon.startsWith('file:')) {
      const fileName = icon.slice(5); // 去掉 "file:" 前缀
      let cancelled = false;
      setLoading(true);
      setFailed(false);

      invoke<string>('read_agent_icon', { iconName: fileName })
        .then((url) => {
          if (!cancelled) {
            setDataUrl(url);
            setLoading(false);
          }
        })
        .catch((err) => {
          if (!cancelled) {
            console.warn('[AgentIcon] Failed to read icon:', err);
            setFailed(true);
            setLoading(false);
          }
        });

      return () => { cancelled = true; };
    }

    // 非 file: 前缀，重置为文本模式
    setDataUrl(null);
    setLoading(false);
    setFailed(false);
  }, [icon]);

  // 加载中
  if (loading) {
    return (
      <span
        className={className}
        style={{ width: size, height: size, display: 'inline-block', borderRadius: '2px', backgroundColor: 'var(--bg-tertiary)' }}
      />
    );
  }

  // 图片模式（file: 前缀已加载完成）
  if (dataUrl && !failed) {
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
  if (icon && !icon.startsWith('file:')) {
    return <span style={{ fontSize: size, lineHeight: 1 }}>{icon}</span>;
  }

  // 无图标
  return null;
}
