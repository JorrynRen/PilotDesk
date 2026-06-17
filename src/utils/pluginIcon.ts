/**
 * pluginIcon — 插件图标解析工具
 *
 * 支持三种图标格式：
 * 1. 网络地址（https://...）→ 直接作为 <img> src
 * 2. 插件本地路径（image/favicon.png）→ 拼接插件目录后通过 Tauri asset protocol 转换
 * 3. 空/未定义 → 返回 text 类型，由调用方显示默认图标
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

  const trimmed = icon.trim();

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
