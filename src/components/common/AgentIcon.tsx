/**
 * AgentIcon — Agent 图标渲染组件
 *
 * 支持三种图标来源：
 * 1. "file:filename.ico" → 调用 Rust read_agent_icon 读取内置图标文件
 * 2. "https://..." 或 "http://..." → 直接渲染网络图片
 * 3. Emoji 或文本字符 → 直接渲染文本
 *
 * **图片图标统一加底板**：ACP 官方目录里的图标大量是「透明底 + 黑色矢量」，
 * 深色主题下等于隐形。这里在渲染层统一垫一块浅色圆角底板（`plate` 可覆盖/关闭），
 * 一处改动即覆盖消息气泡、会话列表、设置页、Agent 市集等全部入口。
 */

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** 图片图标的底板：浅色（默认）/ 深色（给浅色矢量图标用）/ 关闭 */
export type IconPlate = 'light' | 'dark' | 'none';

interface AgentIconProps {
  icon?: string;
  size?: number;
  className?: string;
  fallback?: React.ReactNode;
  /** 底板覆盖：默认 light；白色矢量图标传 dark，确实不需要底板时传 none */
  plate?: IconPlate;
}

/** 底板样式：外框固定为 `size`，图片按 78% 内缩，视觉尺寸与不加底板时接近 */
function plateStyle(size: number, plate: IconPlate): React.CSSProperties | undefined {
  if (plate === 'none') return undefined;
  return {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: size,
    height: size,
    borderRadius: Math.max(3, Math.round(size * 0.24)),
    backgroundColor: plate === 'light' ? 'rgba(255,255,255,0.94)' : 'rgba(17,24,39,0.88)',
    // 1px 内描边：浅色底板在浅色主题下也能与白色背景区分开
    boxShadow:
      plate === 'light'
        ? 'inset 0 0 0 1px rgba(15,23,42,0.10)'
        : 'inset 0 0 0 1px rgba(255,255,255,0.16)',
    flex: '0 0 auto',
    verticalAlign: 'middle',
  };
}

/** 图片图标：底板 + 按比例内缩的图片（加载失败由调用方决定是否退回 fallback） */
function IconImage({
  src,
  size,
  plate,
  className,
  onError,
}: {
  src: string;
  size: number;
  plate: IconPlate;
  className?: string;
  onError: () => void;
}) {
  const inner = plate === 'none' ? size : Math.max(8, Math.round(size * 0.78));
  return (
    <span className={className} style={plateStyle(size, plate)}>
      <img
        src={src}
        alt=""
        width={inner}
        height={inner}
        style={{ objectFit: 'contain', display: 'block' }}
        onError={onError}
      />
    </span>
  );
}

export function AgentIcon({ icon, size = 14, className = '', fallback, plate = 'light' }: AgentIconProps) {
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
      <IconImage
        src={dataUrl}
        size={size}
        plate={plate}
        className={className}
        onError={() => setFailed(true)}
      />
    );
  }

  // file: 前缀 — 加载中或失败，显示 fallback
  if (icon?.startsWith('file:')) {
    return <>{fallback}</>;
  }

  // 网络图片：加载失败退回 fallback（否则会留下一个空白占位）
  if (icon && (icon.startsWith('http://') || icon.startsWith('https://'))) {
    if (failed) return <>{fallback}</>;
    return (
      <IconImage
        src={icon}
        size={size}
        plate={plate}
        className={className}
        onError={() => setFailed(true)}
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

  // 无图标（含空串）：交给调用方兜底。
  // 未提供 fallback 时渲染 nothing，与旧行为一致；提供了就让"没有图标"也能有像样的占位。
  return <>{fallback}</>;
}
