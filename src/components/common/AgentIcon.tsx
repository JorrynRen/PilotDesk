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

  // icon 变化时重置派生状态。用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮
  // 级联渲染（`react-hooks/set-state-in-effect`），而"外部 prop 变了就把本地状态重置"正是 React
  // 推荐的 adjust-during-render 场景，两者行为一致。
  const [prevIcon, setPrevIcon] = useState(icon);
  if (prevIcon !== icon) {
    setPrevIcon(icon);
    setDataUrl(null);
    setFailed(false);
  }

  useEffect(() => {
    // 仅 file: 前缀需要异步读取内置图标；其余分支的同步重置已在上面的渲染期修正完成
    if (!icon || !icon.startsWith('file:')) return;

    const fileName = icon.slice(5);
    let cancelled = false;

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

  // 文本模式（Emoji、Unicode 转义序列或字符）
  if (icon) {
    // 解析 Unicode 转义序列（如 \U0001f916 → 🤖）
    const displayText = icon.replace(/\\U([0-9a-fA-F]{8})/g, (_, hex) =>
      String.fromCodePoint(parseInt(hex, 16))
    ).replace(/\\u([0-9a-fA-F]{4})/g, (_, hex) =>
      String.fromCodePoint(parseInt(hex, 16))
    );
    return <span style={{ fontSize: size, lineHeight: 1 }}>{displayText}</span>;
  }

  // 无图标
  return null;
}
