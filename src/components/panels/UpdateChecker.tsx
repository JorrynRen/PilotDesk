import { Download, Check, Loader2, ExternalLink, Rocket } from 'lucide-react';
import { formatBytes, type Updater } from '../../hooks/useUpdater';

/**
 * 更新检查正文。标题与「检查更新」按钮由外层区块承担（见设置页「关于」的版本更新区块），
 * 所以这里不自带标题、也不自带内边距 —— 自带 p-4 会让内容比同页其它行右缩一截。
 */
export function UpdateChecker({ u }: { u: Updater }) {
  const {
    checking, installing, checkedAt, currentVersion, update, error,
    percent, downloaded, contentLength, hasUpdate, install, openReleases,
  } = u;

  return (
    <div className="space-y-4">
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
              onClick={() => void install()}
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
                onClick={() => void openReleases()}
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
          onClick={() => void openReleases()}
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
