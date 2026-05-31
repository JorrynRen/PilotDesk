import { useState, useEffect } from 'react';
import { Download, Check, RefreshCw, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type { EnvInfo } from '../../types';

interface VersionInfo {
  name: string;
  current: string | null;
  latest: string | null;
  hasUpdate: boolean;
  checking: boolean;
  updating: boolean;
}

export function UpdateChecker() {
  const [versions, setVersions] = useState<VersionInfo[]>([
    { name: 'Claude Code', current: null, latest: null, hasUpdate: false, checking: false, updating: false },
    { name: 'Hermes Agent', current: null, latest: null, hasUpdate: false, checking: false, updating: false },
  ]);
  const [lastChecked, setLastChecked] = useState<string | null>(null);

  useEffect(() => {
    fetchVersions();
  }, []);

  const fetchVersions = async () => {
    try {
      const envInfo = await invoke<EnvInfo>('detect_env');

      setVersions((prev) =>
        prev.map((v) => {
          if (v.name === 'Claude Code') {
            return { ...v, current: envInfo.claudeCodeVersion };
          }
          if (v.name === 'Hermes Agent') {
            return { ...v, current: envInfo.hermesVersion };
          }
          return v;
        })
      );

      // Check for latest versions (simulated - actual implementation would query npm/pip)
      setVersions((prev) =>
        prev.map((v) => {
          if (v.current) {
            // For now, mark as "latest" since we can't actually query registries
            return { ...v, latest: v.current, hasUpdate: false };
          }
          return v;
        })
      );

      setLastChecked(new Date().toLocaleTimeString('zh-CN'));
    } catch {
      // Silent fail
    }
  };

  const handleUpdate = (name: string) => {
    setVersions((prev) =>
      prev.map((v) => (v.name === name ? { ...v, updating: true } : v))
    );
    setTimeout(() => {
      setVersions((prev) =>
        prev.map((v) =>
          v.name === name ? { ...v, updating: false, hasUpdate: false } : v
        )
      );
    }, 2000);
  };

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          更新检查
        </h3>
        <button
          onClick={fetchVersions}
          className="flex items-center gap-1 px-2 py-1 rounded text-[10px]"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
        >
          <RefreshCw size={12} />
          检查更新
        </button>
      </div>

      {lastChecked && (
        <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          上次检查: {lastChecked}
        </p>
      )}

      <div className="space-y-2">
        {versions.map((v) => (
          <div
            key={v.name}
            className="flex items-center justify-between px-3 py-2.5 rounded-lg"
            style={{ backgroundColor: 'var(--bg-tertiary)' }}
          >
            <div className="flex items-center gap-2">
              {v.hasUpdate ? (
                <Download size={14} style={{ color: '#F59E0B' }} />
              ) : v.current ? (
                <Check size={14} style={{ color: '#10B981' }} />
              ) : (
                <span className="w-3.5 h-3.5 rounded-full" style={{ backgroundColor: 'var(--border)' }} />
              )}
              <div>
                <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                  {v.name}
                </span>
                <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                  {v.current ? `当前: ${v.current}` : '未安装'}
                  {v.latest && v.hasUpdate && ` | 最新: ${v.latest}`}
                </div>
              </div>
            </div>
            <div className="flex items-center gap-2">
              {v.hasUpdate && (
                <button
                  onClick={() => handleUpdate(v.name)}
                  disabled={v.updating}
                  className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium"
                  style={{ backgroundColor: '#F59E0B', color: '#fff' }}
                >
                  {v.updating ? (
                    <>
                      <Loader2 size={12} className="animate-spin" />
                      更新中
                    </>
                  ) : (
                    <>
                      <Download size={12} />
                      更新
                    </>
                  )}
                </button>
              )}
              {!v.hasUpdate && v.current && (
                <span className="text-[10px]" style={{ color: '#10B981' }}>已是最新</span>
              )}
            </div>
          </div>
        ))}
      </div>

      <p className="text-[10px] text-center" style={{ color: 'var(--text-tertiary)' }}>
        PilotDesk v0.1.0 | Claude Code SDK | Hermes Agent
      </p>
    </div>
  );
}
