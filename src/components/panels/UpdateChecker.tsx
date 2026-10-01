import { useState } from 'react';
import { Download, Check, RefreshCw, Loader2, ExternalLink, Rocket } from 'lucide-react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { check, type Update as UpdateHandle } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { errorMessage } from '../../utils/errorMessage';

// 版本号单一来源：由 vite.config.ts 从根 package.json 注入
const APP_VERSION = import.meta.env.VITE_APP_VERSION as string;

const RELEASES_URL = 'https://github.com/JorrynRen/PilotDesk/releases';

function formatBytes(bytes: number): string {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function UpdateChecker() {
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [currentVersion, setCurrentVersion] = useState<string>(APP_VERSION);
  const [update, setUpdate] = useState<UpdateHandle | null>(null);
  const [downloaded, setDownloaded] = useState(0);
  const [contentLength, setContentLength] = useState<number | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  const handleOpenReleasePage = async () => {
    try {
      await openUrl(RELEASES_URL);
    } catch {
      window.open(RELEASES_URL, '_blank');
    }
  };

  const fetchUpdates = async () => {
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

  const handleInstall = async () => {
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

  return (
    <div className="p-4 space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
          更新检查
        </h3>
        <button
          onClick={fetchUpdates}
          disabled={checking || installing}
          className="pd-btn flex items-center gap-1 px-2 py-1 rounded text-[10px] transition-colors disabled:opacity-50"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
        >
          {checking ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <RefreshCw size={12} />
          )}
          {checking ? '检查中...' : '检查更新'}
        </button>
      </div>

      {/* Last checked time */}
      {checkedAt && (
        <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          上次检查: {checkedAt}
        </p>
      )}

      {/* Error banner */}
      {error && (
        <div
          className="px-3 py-2 rounded-lg text-xs"
          style={{ backgroundColor: 'rgba(239, 68, 68, 0.08)', color: 'var(--danger)' }}
        >
          {error}
        </div>
      )}

      {/* PilotDesk version card */}
      <div
        className="flex items-center justify-between px-3 py-2.5 rounded-lg transition-colors"
        style={{
          backgroundColor: hasUpdate ? 'rgba(245, 158, 11, 0.06)' : 'var(--bg-tertiary)',
          border: hasUpdate ? '1px solid rgba(245, 158, 11, 0.2)' : '1px solid transparent',
        }}
      >
        <div className="flex items-center gap-2 min-w-0">
          {hasUpdate ? (
            <Download size={14} style={{ color: '#F59E0B', flexShrink: 0 }} />
          ) : (
            <Check size={14} style={{ color: '#10B981', flexShrink: 0 }} />
          )}
          <div className="min-w-0">
            <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
              PilotDesk
            </span>
            <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
              当前: v{currentVersion}
              {hasUpdate && (
                <span style={{ color: '#F59E0B' }}> | 最新: v{update?.version}</span>
              )}
            </div>
            {installing && (
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                正在下载安装包{percent !== null ? ` (${percent}%)` : ''}
                {contentLength ? ` · ${formatBytes(downloaded)} / ${formatBytes(contentLength)}` : ''}
              </div>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2 shrink-0">
          {hasUpdate && (
            <button
              onClick={handleInstall}
              disabled={installing}
              className="pd-btn flex items-center gap-1 px-2 py-1 rounded text-[10px]  transition-colors disabled:opacity-60"
              style={{ backgroundColor: '#F59E0B', color: '#fff' }}
              title="下载并安装新版本，随后自动重启"
            >
              {installing ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
              {installing ? '安装中...' : '下载并安装'}
            </button>
          )}
          {!hasUpdate && !checking && (
            <span className="text-[10px]" style={{ color: '#10B981' }}>
              已是最新
            </span>
          )}
        </div>
      </div>

      {/* Update notice banner */}
      {hasUpdate && (
        <div
          className="flex items-start gap-2 px-3 py-2.5 rounded-lg"
          style={{ backgroundColor: 'rgba(245, 158, 11, 0.06)', border: '1px solid rgba(245, 158, 11, 0.15)' }}
        >
          <Rocket size={14} style={{ color: '#F59E0B', flexShrink: 0, marginTop: 1 }} />
          <div>
            <p className="text-xs " style={{ color: '#F59E0B' }}>
              发现新版本 v{update?.version}
            </p>
            <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-secondary)' }}>
              点击「下载并安装」将自动下载并重启应用。也可前往{' '}
              <span
                className="cursor-pointer underline"
                style={{ color: 'var(--accent)' }}
                onClick={handleOpenReleasePage}
              >
                GitHub Releases
              </span>{' '}
              手动下载。
            </p>
          </div>
        </div>
      )}

      {/* Fallback entry when update exists but install failed */}
      {hasUpdate && error && (
        <button
          onClick={handleOpenReleasePage}
          className="pd-btn w-full flex items-center justify-center gap-1 px-2 py-1.5 rounded text-[10px] transition-colors"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)', border: '1px solid var(--border)' }}
        >
          <ExternalLink size={11} />
          前往 GitHub Releases 手动下载
        </button>
      )}

      {/* Footer */}
      <p className="text-[10px] text-center" style={{ color: 'var(--text-tertiary)' }}>
        Agent 更新检查请前往「环境配置」页面
      </p>
    </div>
  );
}
