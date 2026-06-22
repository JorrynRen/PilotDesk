/// 在线商店服务器源（按优先级排列）
/// 1. jsdelivr CDN（主源，速度快，全球加速）
/// 2. GitHub Raw（降级源，直连仓库）
export const SERVER_SOURCES = [
  'https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main',
  'https://raw.githubusercontent.com/JorrynRen/PilotDesk/main',
];

/// 在线商店路径
export const AGENTS_CONFIG_PATH = '/server/market/agents-config/agents-config.json';
export const PLUGINS_INDEX_PATH = '/server/market/plugins/index.json';
export const PLUGINS_DIR_PATH = '/server/market/plugins';

/// 根据路径构建所有服务器源的完整 URL 列表
export function buildUrls(path: string): string[] {
  return SERVER_SOURCES.map((base) => `${base.replace(/\/$/, '')}${path}`);
}
