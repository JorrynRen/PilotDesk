import type { LifecycleSpec } from '../types';

/**
 * 当前平台键（与 Rust 侧 `lifecycle::current_platform_key` 同源）。
 *
 * 只用于在 UI 上提前判断「要不要提示未校验」；真正的平台匹配与安装判定在后端，
 * 这里误判最多是多一次确认弹窗或少一次提示，不会放行不该放行的安装。
 */
export function currentPlatformKey(): string {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent.toLowerCase();
  const os = ua.includes('windows') ? 'windows' : ua.includes('mac') ? 'darwin' : 'linux';
  const arch = ua.includes('arm64') || ua.includes('aarch64') ? 'aarch64' : 'x86_64';
  return `${os}-${arch}`;
}

/**
 * 这次安装是否需要用户显式确认「可能不校验完整性」。
 *
 * 仅在 binary + 当前平台制品可自动安装（zip、启动器非 .cmd）且
 * **没有 registry 校验值**时为 true——后端仍会先尝试厂商的 `.sha256` / `checksums.txt`，
 * 取到就照常校验，取不到才按「不校验」安装。
 */
export function needsUnverifiedConsent(lifecycle?: LifecycleSpec | null): boolean {
  if (!lifecycle || lifecycle.kind !== 'binary') return false;
  const key = currentPlatformKey();
  const artifact = (lifecycle.artifacts ?? []).find((a) => a.platform === key);
  if (!artifact) return false;
  if (!artifact.archive.toLowerCase().endsWith('.zip')) return false;
  if (artifact.cmd.toLowerCase().endsWith('.cmd')) return false;
  return !artifact.sha256?.trim();
}
