import { Download, Check, Loader2, RefreshCw } from 'lucide-react';
import { formatBytes, type Updater } from '../../hooks/useUpdater';

/**
 * 更新检查结果行。
 *
 * 标题与「检查更新」按钮由「关于 PilotDesk」区块的 actions 承担，产品名与当前版本
 * 也由该区块显示 —— 这里只呈现检查结果与安装动作，避免同一块内容里重复两遍。
 */
export function UpdateChecker({ u }: { u: Updater }) {
  const {
    checking, installing, checkedAt, update, error,
    percent, downloaded, contentLength, hasUpdate, install, openReleases,
  } = u;

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 flex-wrap text-xs">
        {checking ? (
          <RefreshCw size={12} className="animate-spin" style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
        ) : hasUpdate ? (
          <Download size={12} style={{ color: 'var(--status-warning)', flexShrink: 0 }} />
        ) : (
          <Check size={12} style={{ color: 'var(--status-success)', flexShrink: 0 }} />
        )}
        <span style={{ color: hasUpdate ? 'var(--status-warning)' : 'var(--text-secondary)' }}>
          {checking
            ? '正在检查更新…'
            : hasUpdate
              ? `发现新版本 v${update?.version}`
              : checkedAt
                ? '已是最新版本'
                : '尚未检查更新'}
        </span>
        {checkedAt && !checking && (
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>上次检查: {checkedAt}</span>
        )}
        {hasUpdate && (
          <div className="ml-auto flex items-center gap-2">
            <button
              onClick={() => void install()}
              disabled={installing}
              className="pd-btn flex items-center gap-1 px-2 py-1 rounded text-[10px] transition-colors disabled:opacity-60"
              style={{ backgroundColor: 'var(--accent)', color: 'var(--text-on-accent)' }}
              title="下载并安装新版本，随后自动重启"
            >
              {installing ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
              {installing ? '安装中...' : '下载并安装'}
            </button>
            <button
              onClick={() => void openReleases()}
              className="pd-btn px-2 py-1 rounded text-[10px] transition-colors"
              style={{ color: 'var(--accent)' }}
              title="前往 GitHub Releases 手动下载"
            >
              GitHub Releases
            </button>
          </div>
        )}
      </div>

      {installing && (
        <div
          className="px-3 py-2 rounded-md text-[10px]"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
        >
          正在下载安装包{percent !== null ? ` (${percent}%)` : ''}
          {contentLength ? ` · ${formatBytes(downloaded)} / ${formatBytes(contentLength)}` : ''}
        </div>
      )}

      {error && (
        <div
          className="px-3 py-2 rounded-lg text-xs"
          style={{ backgroundColor: 'var(--danger-bg)', color: 'var(--danger)' }}
        >
          {error}
        </div>
      )}

      <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
        Agent 更新检查请前往「环境配置」页面
      </p>
    </div>
  );
}
