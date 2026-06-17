/**
 * pluginIcon — 插件图标解析工具
 *
 * 解析 manifest.json 中 icon 字段的值，返回图标类型和原始值。
 * 支持格式：
 * - "icon: https://..." 或 "https://..." → 网络图片
 * - "icon: image/favicon.png" 或 "image/favicon.png" → 插件本地文件
 * - 空/未定义 → text 类型，由调用方显示默认图标
 */

export type IconType = 'network' | 'local' | 'text';

export interface ParsedIcon {
  type: IconType;
  /** 网络地址的 URL，或本地文件的相对路径 */
  value?: string;
}

/**
 * 解析插件图标字段（同步，不涉及文件读取）
 * @param icon - manifest.json 中的 icon 字段值
 */
export function parsePluginIcon(icon: string | undefined): ParsedIcon {
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

  // 网络地址
  if (trimmed.startsWith('http://') || trimmed.startsWith('https://')) {
    return { type: 'network', value: trimmed };
  }

  // 插件本地路径
  return { type: 'local', value: trimmed };
}
