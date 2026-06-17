/**
 * PluginIcon — 插件图标渲染组件
 *
 * 根据 parsePluginIcon 的解析结果渲染：
 * - image 类型：渲染 <img> 标签，限定尺寸，加载失败时隐藏
 * - text 类型：渲染文本或默认图标
 */

import { useState } from 'react';
import { parsePluginIcon } from '../../utils/pluginIcon';

interface PluginIconProps {
  icon?: string;
  pluginPath: string;
  size?: number;
}

export function PluginIcon({ icon, pluginPath, size = 14 }: PluginIconProps) {
  const [loadFailed, setLoadFailed] = useState(false);
  const parsed = parsePluginIcon(icon, pluginPath);

  if (parsed.type === 'image' && parsed.src && !loadFailed) {
    return (
      <img
        src={parsed.src}
        alt=""
        width={size}
        height={size}
        className="inline-block object-contain shrink-0 rounded-sm"
        style={{ verticalAlign: 'middle' }}
        onError={() => setLoadFailed(true)}
      />
    );
  }

  // 文本模式或图片加载失败：显示默认图标
  return <span style={{ fontSize: size }}>📦</span>;
}
