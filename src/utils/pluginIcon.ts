/**
 * pluginIcon — 插件图标解析工具
 *
 * 支持三种图标格式：
 * 1. "icon:" 前缀 + 网络地址 → 如 "icon: https://example.com/icon.png"
 * 2. "icon:" 前缀 + 插件本地路径 → 如 "icon: image/favicon.png"
 * 3. 纯网络地址 → 如 "https://example.com/icon.png"
 * 4. 纯本地路径 → 如 "image/favicon.png"
 * 5. 空/未定义 → 返回 text 类型，由调用方显示默认图标
 */

import { convertFileSrc } from '@tauri-apps/api/core';

export interface ParsedIcon {
  type: 'image' | 'text';
  src?: string;
}

/**
 * 解析插件图标字段
 * @param icon - manifest.json 中的 icon 字段值
 * @param pluginPath - 插件安装目录的绝对路径
 */
export function parsePluginIcon(icon: string | undefined, pluginPath: string): ParsedIcon {
  if (!icon || icon.trim() === '') {
    return { type: 'text' };
  }

  let trimmed = icon.trim();

  // 去掉 "icon:" 前缀（如果有）
  const iconPrefix = 'icon:';
  if (trimmed.startsWith(iconPrefix)) {
    trimmed = trimmed.slice(iconPrefix.length).trim();
  }

  if (!trimmed) {
    return { type: 'text' };
  }

  // 网络地址：直接使用
  if (trimmed.startsWith('http://') || trimmed.startsWith('https://')) {
    return { type: 'image', src: trimmed };
  }

  // 插件本地路径：拼接后通过 Tauri asset protocol 转换
  const normalizedPath = trimmed.replace(/\//g, '\\');
  const fullPath = pluginPath.endsWith('\\')
    ? pluginPath + normalizedPath
    : pluginPath + '\\' + normalizedPath;

  return { type: 'image', src: convertFileSrc(fullPath) };
}
