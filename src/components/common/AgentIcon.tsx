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

/**
 * 图片图标的底板。
 *
 * - `light`（默认）：**Agent 品牌色淡染**的浅底（品牌色 16% + 白）。既保留该 Agent
 *   的识别色，又保证亮度足够承托「透明底黑色矢量」；`color-mix` 不可用时退回纯白。
 * - `accent`：纯 Agent 品牌色实底。视觉最贴合品牌，但黑色矢量的对比度会掉到
 *   3.5–6:1（取决于品牌色），只建议给彩色/白色图标用。
 * - `dark`：深色底，给浅色（白色）矢量图标用。
 * - `none`：不垫底板（外层已有底板时）。
 */
export type IconPlate = 'light' | 'accent' | 'dark' | 'none';

interface AgentIconProps {
  icon?: string;
  size?: number;
  className?: string;
  fallback?: React.ReactNode;
  /** 底板覆盖：默认 light；白色矢量图标传 dark，纯品牌色实底传 accent，已有底板传 none */
  plate?: IconPlate;
  /**
   * Agent 品牌色（如 `#8B5CF6`）：底板按它淡染/着色。
   * 不传时退回应用主题色 `var(--accent)`。
   */
  accentColor?: string;
}

/** 底板样式：外框固定为 `size`，图片按 78% 内缩，视觉尺寸与不加底板时接近 */
function plateStyle(
  size: number,
  plate: IconPlate,
  accentColor?: string,
): React.CSSProperties | undefined {
  if (plate === 'none') return undefined;
  const tint = accentColor && accentColor.trim() ? accentColor.trim() : 'var(--accent)';
  const base: React.CSSProperties = {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: size,
    height: size,
    borderRadius: Math.max(3, Math.round(size * 0.24)),
    flex: '0 0 auto',
    verticalAlign: 'middle',
  };

  if (plate === 'dark') {
    return {
      ...base,
      backgroundColor: 'rgba(17,24,39,0.88)',
      boxShadow: 'inset 0 0 0 1px rgba(255,255,255,0.16)',
    };
  }

  if (plate === 'accent') {
    return {
      ...base,
      backgroundColor: tint,
      boxShadow: 'inset 0 0 0 1px rgba(0,0,0,0.12)',
    };
  }

  // light：品牌色淡染。先写 backgroundColor 作兜底，再用 background 覆盖——
  // 浏览器不支持 color-mix 时整条 background 声明会被丢弃，底色退回白色。
  return {
    ...base,
    backgroundColor: '#ffffff',
    background: `color-mix(in srgb, ${tint} 16%, #ffffff)`,
    boxShadow: `inset 0 0 0 1px color-mix(in srgb, ${tint} 30%, transparent)`,
  };
}

/** 图片图标：底板 + 按比例内缩的图片（加载失败由调用方决定是否退回 fallback） */
function IconImage({
  src,
  size,
  plate,
  accentColor,
  className,
  onError,
}: {
  src: string;
  size: number;
  plate: IconPlate;
  accentColor?: string;
  className?: string;
  onError: () => void;
}) {
  const inner = plate === 'none' ? size : Math.max(8, Math.round(size * 0.78));
  return (
    <span className={className} style={plateStyle(size, plate, accentColor)}>
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

export function AgentIcon({ icon, size = 14, className = '', fallback, plate = 'light', accentColor }: AgentIconProps) {
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
        accentColor={accentColor}
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
        accentColor={accentColor}
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
