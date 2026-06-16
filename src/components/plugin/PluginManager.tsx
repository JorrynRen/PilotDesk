import { useEffect, useState } from 'react';
import { usePluginStore } from '../../stores/pluginStore';
import type { PermissionCheck } from '../../types/plugin';

function PermissionBadge({ check }: { check: PermissionCheck }) {
  return (
    <span
      className="text-[9px] px-1.5 py-0.5 rounded cursor-help"
      title={check.reason || undefined}
      style={{
        backgroundColor: check.allowed
          ? check.reason?.includes('高风险')
            ? 'rgba(245,158,11,0.15)'
            : 'var(--accent-light)'
          : 'rgba(239,68,68,0.15)',
        color: check.allowed
          ? check.reason?.includes('高风险')
            ? '#F59E0B'
            : 'var(--accent)'
          : '#EF4444',
      }}
    >
      {check.permission}
      {check.reason?.includes('高风险') && ' ⚠️'}
    </span>
  );
}

function SandboxInfoPanel() {
  const { sandboxInfo, fetchSandboxInfo } = usePluginStore();

  useEffect(() => {
    fetchSandboxInfo();
  }, [fetchSandboxInfo]);

  if (!sandboxInfo) return null;

  return (
    <details className="mb-3">
      <summary
        className="text-[10px] cursor-pointer select-none"
        style={{ color: 'var(--text-tertiary)' }}
      >
        沙箱信息
      </summary>
      <div
        className="mt-1.5 p-2 rounded text-[9px] space-y-1"
        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
      >
        <div className="flex items-center gap-2">
          <span>沙箱状态:</span>
          <span
            className="px-1.5 py-0.5 rounded text-[9px]"
            style={{
              backgroundColor: sandboxInfo.sandbox_enabled
                ? 'rgba(16,185,129,0.15)'
                : 'rgba(239,68,68,0.15)',
              color: sandboxInfo.sandbox_enabled ? '#10B981' : '#EF4444',
            }}
          >
            {sandboxInfo.sandbox_enabled ? '已启用' : '已禁用'}
          </span>
        </div>
        <div>插件目录: <code className="px-1 rounded" style={{ backgroundColor: 'var(--bg-secondary)' }}>{sandboxInfo.plugins_dir}</code></div>
        <div>最大清单大小: {(sandboxInfo.max_manifest_size / 1024).toFixed(0)} KB</div>
        <div>合法权限: {sandboxInfo.allowed_permissions.join(', ')}</div>
        <div>高风险权限: {sandboxInfo.high_risk_permissions.join(', ')}</div>
      </div>
    </details>
  );
}

export function PluginManager() {
  const { plugins, loading, error, discover, enable, disable } = usePluginStore();
  const [showSandbox, setShowSandbox] = useState(false);

  useEffect(() => {
    discover();
  }, [discover]);

  return (
    <div className="p-4">
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
          插件管理
        </h3>
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => setShowSandbox(!showSandbox)}
            className="text-[10px] px-2 py-1 rounded transition-all"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            沙箱
          </button>
          <button
            onClick={discover}
            className="text-[10px] px-2 py-1 rounded transition-all"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            刷新
          </button>
        </div>
      </div>

      {showSandbox && <SandboxInfoPanel />}

      {loading && (
        <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>
          扫描插件中...
        </div>
      )}

      {error && (
        <div
          className="text-xs py-2 px-3 rounded mb-3"
          style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}
        >
          {error}
        </div>
      )}

      {!loading && plugins.length === 0 && (
        <div className="text-xs py-8 text-center" style={{ color: 'var(--text-tertiary)' }}>
          <p className="mb-2">暂无已安装的插件</p>
          <p className="text-[10px]">
            将插件目录放置到{' '}
            <code className="px-1 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
              ~/.pilotdesk/plugins/
            </code>
          </p>
        </div>
      )}

      <div className="space-y-2">
        {plugins.map((plugin) => (
          <div
            key={plugin.manifest.id}
            className="px-3 py-2.5 rounded-lg transition-colors"
            style={{
              backgroundColor: 'var(--bg-secondary)',
              border: '1px solid var(--border)',
              opacity: plugin.enabled ? 1 : 0.5,
            }}
          >
            <div className="flex items-start justify-between">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>
                    {plugin.manifest.name}
                  </span>
                  <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                    v{plugin.manifest.version}
                  </span>
                </div>
                <p className="text-[10px] mt-0.5 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>
                  {plugin.manifest.description}
                </p>
                <div className="flex items-center gap-1.5 mt-1.5">
                  <span
                    className="text-[9px] px-1.5 py-0.5 rounded"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
                  >
                    {plugin.manifest.author}
                  </span>
                  {plugin.permission_checks.map((check) => (
                    <PermissionBadge key={check.permission} check={check} />
                  ))}
                </div>
              </div>
              <button
                onClick={() => (plugin.enabled ? disable(plugin.manifest.id) : enable(plugin.manifest.id))}
                className="shrink-0 ml-2 px-2.5 py-1 rounded text-[10px] font-medium transition-all"
                style={{
                  backgroundColor: plugin.enabled
                    ? 'rgba(16,185,129,0.15)'
                    : 'var(--bg-tertiary)',
                  color: plugin.enabled ? '#10B981' : 'var(--text-secondary)',
                }}
                disabled={!plugin.enabled && plugin.has_unauthorized_permissions}
                title={
                  !plugin.enabled && plugin.has_unauthorized_permissions
                    ? '包含未授权权限，无法启用'
                    : undefined
                }
              >
                {plugin.enabled
                  ? '已启用'
                  : plugin.has_unauthorized_permissions
                    ? '权限异常'
                    : '已禁用'}
              </button>
            </div>
            {plugin.error && (
              <p className="text-[10px] mt-1.5" style={{ color: '#EF4444' }}>
                {plugin.error}
              </p>
            )}
            {plugin.has_unauthorized_permissions && (
              <p className="text-[10px] mt-1" style={{ color: '#F59E0B' }}>
                包含未授权权限声明，请联系插件开发者或检查 manifest.json
              </p>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
