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
 * - `light`（默认）：**纯白**圆角底板 + 中性细描边。白色最干净——淡染
 *   （品牌色 16% + 白）在深色主题下会显得发灰发脏，已弃用。
 * - `accent`：纯 Agent 品牌色实底。适合本身是彩色/白色矢量的图标；
 *   黑色矢量压在上面只有 3.5–6:1，慎用。
 * - `dark`：深色底，给浅色（白色）矢量图标用。
 * - `none`：不垫底板（外层已有底板、或图标自带不透明背景时）。
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
   * Agent 品牌色（如 `#8B5CF6`）：`accent` 档位的实底颜色。
   * 不传时退回应用主题色 `var(--accent)`。
   */
  accentColor?: string;
}

/** 底板样式：外框固定为 `size`，图片**铺满**整块底板（图标自带的背景不再露出外圈） */
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
    // 图片铺满 + 裁圆角：不透明图标（自带方底）不会露出外圈底板
    overflow: 'hidden',
    // border-box + 真实 border：内容被裁到 padding box，边框**始终可见**。
    // （以前用 inset box-shadow 画边，图片铺满后会盖住它，看起来像"没边框"）
    boxSizing: 'border-box',
    flex: '0 0 auto',
    verticalAlign: 'middle',
  };

  if (plate === 'dark') {
    return {
      ...base,
      backgroundColor: '#111827',
      border: '1px solid rgba(255,255,255,0.18)',
    };
  }

  if (plate === 'accent') {
    return {
      ...base,
      backgroundColor: tint,
      border: '1px solid rgba(0,0,0,0.14)',
    };
  }

  // light：纯白 + 中性描边（浅色主题下也能与白色背景区分）
  return {
    ...base,
    backgroundColor: '#ffffff',
    border: '1px solid rgba(15,23,42,0.14)',
  };
}

/**
 * 图片图标：底板 + 铺满底板的图片（加载失败由调用方决定是否退回 fallback）。
 *
 * 图片**不内缩**（= size）：不同厂商图标本身留白不同，内缩会让自带方底的图标
 * 露出外圈底板（看起来像两层背景）；铺满后不透明图标直接盖住底板，透明图标
 * 仍由底板提供对比度。
 */
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
  // 有底板时图片按 padding box 的 100% 铺满（边框占 1px，靠 border-box 消化）；
  // 无底板时没有可参照的容器尺寸，退回显式 size。
  const fill: number | string = plate === 'none' ? size : '100%';
  return (
    <span className={className} style={plateStyle(size, plate, accentColor)}>
      <img
        src={src}
        alt=""
        style={{ objectFit: 'contain', display: 'block', width: fill, height: fill }}
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
