import { useState } from 'react';
import { Download, Check, RefreshCw, Loader2, ExternalLink, Rocket } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface VersionCheckResult {
  name: string;
  current: string | null;
  latest: string | null;
  hasUpdate: boolean;
  error: string | null;
}

interface UpdateCheckResponse {
  pilotdesk: VersionCheckResult;
  checkedAt: string;
}

export function UpdateChecker() {
  const [updateResult, setUpdateResult] = useState<UpdateCheckResponse | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchUpdates = async () => {
    setChecking(true);
    setError(null);
    try {
      const result = await invoke<UpdateCheckResponse>('check_pilotdesk_update');
      setUpdateResult(result);
    } catch (e: any) {
      const msg = e?.message || (typeof e === 'string' ? e : JSON.stringify(e));
      setError(`检查更新失败: ${msg}`);
    } finally {
      setChecking(false);
    }
  };

  const pd = updateResult?.pilotdesk;

  const handleOpenReleasePage = async () => {
    const { open: openUrl } = await import('@tauri-apps/plugin-shell');
    try {
      await openUrl('https://github.com/jorryn/pilotdesk/releases');
    } catch {
      window.open('https://github.com/jorryn/pilotdesk/releases', '_blank');
    }
  };

  return (
    <div className="p-4 space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          更新检查
        </h3>
        <button
          onClick={fetchUpdates}
          disabled={checking}
          className="flex items-center gap-1 px-2 py-1 rounded text-[10px] transition-colors disabled:opacity-50"
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
      {updateResult && (
        <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          上次检查: {updateResult.checkedAt}
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
      {pd && (
        <div
          className="flex items-center justify-between px-3 py-2.5 rounded-lg transition-colors"
          style={{
            backgroundColor: pd.hasUpdate ? 'rgba(245, 158, 11, 0.06)' : 'var(--bg-tertiary)',
            border: pd.hasUpdate ? '1px solid rgba(245, 158, 11, 0.2)' : '1px solid transparent',
          }}
        >
          <div className="flex items-center gap-2 min-w-0">
            {pd.hasUpdate ? (
              <Download size={14} style={{ color: '#F59E0B', flexShrink: 0 }} />
            ) : (
              <Check size={14} style={{ color: '#10B981', flexShrink: 0 }} />
            )}
            <div className="min-w-0">
              <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                PilotDesk
              </span>
              <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                当前: v{pd.current}
                {pd.latest && pd.hasUpdate && (
                  <span style={{ color: '#F59E0B' }}> | 最新: v{pd.latest}</span>
                )}
                {!pd.hasUpdate && pd.latest && (
                  <span> ({pd.latest})</span>
                )}
              </div>
              {pd.error && (
                <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {pd.error}
                </div>
              )}
            </div>
          </div>

          <div className="flex items-center gap-2 shrink-0">
            {pd.hasUpdate && (
              <button
                onClick={handleOpenReleasePage}
                className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium transition-colors"
                style={{ backgroundColor: '#F59E0B', color: '#fff' }}
                title="前往 GitHub 下载新版本"
              >
                <ExternalLink size={11} />
                下载
              </button>
            )}
            {!pd.hasUpdate && (
              <span className="text-[10px]" style={{ color: '#10B981' }}>
                已是最新
              </span>
            )}
          </div>
        </div>
      )}

      {/* Upgrade notice banner */}
      {pd?.hasUpdate && (
        <div
          className="flex items-start gap-2 px-3 py-2.5 rounded-lg"
          style={{ backgroundColor: 'rgba(245, 158, 11, 0.06)', border: '1px solid rgba(245, 158, 11, 0.15)' }}
        >
          <Rocket size={14} style={{ color: '#F59E0B', flexShrink: 0, marginTop: 1 }} />
          <div>
            <p className="text-xs font-medium" style={{ color: '#F59E0B' }}>
              发现新版本
            </p>
            <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-secondary)' }}>
              建议更新至最新版本以获得最新功能和安全修复。前往{' '}
              <span
                className="cursor-pointer underline"
                style={{ color: 'var(--accent)' }}
                onClick={handleOpenReleasePage}
              >
                GitHub Releases
              </span>{' '}
              下载。
            </p>
          </div>
        </div>
      )}

      {/* Footer */}
      <p className="text-[10px] text-center" style={{ color: 'var(--text-tertiary)' }}>
        Claude Code / Hermes Agent 更新检查请前往「环境配置」页面
      </p>
    </div>
  );
}
