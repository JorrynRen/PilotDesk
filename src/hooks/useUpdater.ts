/**
 * 更新检查的状态与动作。
 *
 * 为什么单独成文件：`react-refresh/only-export-components` 要求一个文件要么只导出组件、
 * 要么只导出非组件（与 TerminalProvider 当初拆分的理由相同）。hook 与组件分处两个文件后，
 * 「检查更新」按钮（挂在 SettingsSection 的 actions 槽里，与区块标题同一行）和正文才能共享同一份状态。
 */
import { useState } from 'react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { check, type Update as UpdateHandle } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { errorMessage } from '../utils/errorMessage';

/** 版本号单一来源：由 vite.config.ts 从根 package.json 注入 */
const APP_VERSION = import.meta.env.VITE_APP_VERSION as string;

/** 手动下载入口（更新说明里的「GitHub Releases」与安装失败时的兜底按钮共用） */
const RELEASES_URL = 'https://github.com/JorrynRen/PilotDesk/releases';

/** 字节数的人类可读格式（安装包大小展示用） */
export function formatBytes(bytes: number): string {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function useUpdater() {
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [currentVersion, setCurrentVersion] = useState<string>(APP_VERSION);
  const [update, setUpdate] = useState<UpdateHandle | null>(null);
  const [downloaded, setDownloaded] = useState(0);
  const [contentLength, setContentLength] = useState<number | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  const openReleases = async () => {
    try {
      await openUrl(RELEASES_URL);
    } catch {
      window.open(RELEASES_URL, '_blank');
    }
  };

  // ⚠️ 不能叫 check：会遮蔽上面从 plugin-updater 导入的 check()，变成自我递归
  const checkUpdates = async () => {
    setChecking(true);
    setError(null);
    try {
      // 释放上一次检查留下、且未安装的 Update 资源（避免 Rust 侧句柄泄漏）
      if (update) {
        await update.close().catch(() => {});
        setUpdate(null);
      }
      const result = await check();
      setUpdate(result);
      setCurrentVersion(result?.currentVersion ?? APP_VERSION);
      setCheckedAt(new Date().toLocaleString());
    } catch (e) {
      const msg = errorMessage(e);
      setError(`检查更新失败: ${msg}`);
    } finally {
      setChecking(false);
    }
  };

  const install = async () => {
    if (!update) return;
    setInstalling(true);
    setError(null);
    setDownloaded(0);
    setContentLength(undefined);
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          setContentLength(event.data.contentLength);
        } else if (event.event === 'Progress') {
          setDownloaded((prev) => prev + event.data.chunkLength);
        }
      });
      // Windows 下安装器启动后应用会自动退出；macOS / Linux 需要主动重启以运行新版本
      await relaunch();
    } catch (e) {
      const msg = errorMessage(e);
      setError(`安装更新失败: ${msg}。可前往 GitHub Releases 手动下载。`);
      setInstalling(false);
    }
  };

  const hasUpdate = update !== null;
  const percent =
    contentLength && contentLength > 0
      ? Math.min(100, Math.round((downloaded / contentLength) * 100))
      : null;

  return {
    checking, installing, checkedAt, currentVersion, update, error,
    downloaded, contentLength, hasUpdate, percent,
    check: checkUpdates, install, openReleases,
  };
}

export type Updater = ReturnType<typeof useUpdater>;
