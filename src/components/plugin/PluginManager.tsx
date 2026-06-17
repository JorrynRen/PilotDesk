import { useEffect, useState, useCallback } from 'react';
import { Trash2, Shield, ShieldOff, Copy, RefreshCw } from 'lucide-react';
import { usePluginStore } from '../../stores/pluginStore';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import type { PermissionCheck, PluginInstance } from '../../types/plugin';

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
  const { sandboxInfo, fetchSandboxInfo, setSandboxEnabled } = usePluginStore();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    fetchSandboxInfo();
  }, [fetchSandboxInfo]);

  if (!sandboxInfo) return null;

  const handleCopyPath = () => {
    navigator.clipboard.writeText(sandboxInfo.plugins_dir);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="mb-3 p-3 rounded-lg space-y-3" style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}>
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>沙箱保护</span>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => !sandboxInfo.sandbox_enabled && setSandboxEnabled(true)}
            className="pd-btn px-2 py-1 rounded text-[11px] flex items-center gap-1"
            style={{
              backgroundColor: sandboxInfo.sandbox_enabled ? 'var(--accent-light)' : 'var(--bg-secondary)',
              color: sandboxInfo.sandbox_enabled ? 'var(--accent)' : 'var(--text-tertiary)',
              opacity: sandboxInfo.sandbox_enabled ? 1 : 0.5,
              cursor: sandboxInfo.sandbox_enabled ? 'default' : 'pointer',
            }}
            title={sandboxInfo.sandbox_enabled ? '沙箱已启用' : '启用沙箱'}
          >
            <Shield size={12} />
            启用
          </button>
          <button
            onClick={() => sandboxInfo.sandbox_enabled && setSandboxEnabled(false)}
            className="pd-btn px-2 py-1 rounded text-[11px] flex items-center gap-1"
            style={{
              backgroundColor: !sandboxInfo.sandbox_enabled ? 'rgba(239,68,68,0.1)' : 'var(--bg-secondary)',
              color: !sandboxInfo.sandbox_enabled ? '#EF4444' : 'var(--text-tertiary)',
              opacity: !sandboxInfo.sandbox_enabled ? 1 : 0.5,
              cursor: !sandboxInfo.sandbox_enabled ? 'default' : 'pointer',
            }}
            title={!sandboxInfo.sandbox_enabled ? '沙箱已禁用' : '禁用沙箱'}
          >
            <ShieldOff size={12} />
            禁用
          </button>
        </div>
      </div>

      <div className="space-y-1.5 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
        <div className="flex items-center gap-1.5">
          <span className="shrink-0 font-medium">插件目录:</span>
          <code className="flex-1 break-all px-1.5 py-0.5 rounded text-[10px]" style={{ backgroundColor: 'var(--bg-secondary)' }}>
            {sandboxInfo.plugins_dir}
          </code>
          <button
            onClick={handleCopyPath}
            className="pd-btn shrink-0 px-1.5 py-0.5 rounded text-[10px]"
            style={{ color: copied ? '#10B981' : 'var(--accent)' }}
          >
            {copied ? '已复制' : <Copy size={11} />}
          </button>
        </div>
        <div className="flex items-center gap-3">
          <span>清单上限: <strong>{(sandboxInfo.max_manifest_size / 1024).toFixed(0)} KB</strong></span>
          <span>已注册权限: <strong>{sandboxInfo.allowed_permissions.length}</strong></span>
        </div>
      </div>

      <div>
        <div className="text-[11px] font-medium mb-1.5" style={{ color: 'var(--text-primary)' }}>权限列表</div>
        <div className="flex flex-wrap gap-1">
          {sandboxInfo.allowed_permissions.map((perm) => {
            const isHighRisk = sandboxInfo.high_risk_permissions.includes(perm);
            return (
              <span
                key={perm}
                className="text-[10px] px-1.5 py-0.5 rounded"
                style={{
                  backgroundColor: isHighRisk ? 'rgba(245,158,11,0.15)' : 'var(--accent-light)',
                  color: isHighRisk ? '#F59E0B' : 'var(--accent)',
                }}
                title={isHighRisk ? '高风险权限' : '低风险权限'}
              >
                {perm}
                {isHighRisk && ' ⚠️'}
              </span>
            );
          })}
        </div>
      </div>

      <div className="flex items-center gap-2 pt-1">
        <button
          onClick={handleCopyPath}
          className="pd-btn text-[10px] px-2 py-1 rounded flex items-center gap-1"
          style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)' }}
        >
          <Copy size={11} />
          复制路径
        </button>
        <button
          onClick={fetchSandboxInfo}
          className="pd-btn text-[10px] px-2 py-1 rounded flex items-center gap-1"
          style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)' }}
        >
          <RefreshCw size={11} />
          刷新
        </button>
      </div>
    </div>
  );
}

export function PluginManager() {
  const { plugins, loading, error, discover, enable, disable, installZip, uninstall, sandboxInfo } = usePluginStore();
  const [showSandbox, setShowSandbox] = useState(false);
  const [installStatus, setInstallStatus] = useState<string | null>(null);

  useEffect(() => {
    discover();
    usePluginStore.getState().fetchSandboxInfo();
  }, [discover]);

  // 插件列表变化时，同步注册面板组件并执行插件 JS
  useEffect(() => {
    (async () => {
      await pluginRegistry.loadAllPlugins(plugins);
    })();
  }, [plugins]);

  const handleInstallZip = useCallback(async () => {
    setInstallStatus(null);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        multiple: false,
        filters: [{ name: '插件压缩包', extensions: ['zip'] }],
      });
      if (!selected) return;

      setInstallStatus('正在安装...');
      await installZip(selected as string);
      setInstallStatus('安装成功');
      await discover();
      setTimeout(() => setInstallStatus(null), 3000);
    } catch (err) {
      setInstallStatus('安装失败: ' + String(err));
    }
  }, [installZip, discover]);

  const handleUninstall = useCallback(async (plugin: PluginInstance) => {
    try {
      await uninstall(plugin.manifest.id);
      await pluginRegistry.unloadPlugin(plugin.path);
      await discover();
    } catch (err) {
      console.error('Uninstall failed:', err);
    }
  }, [uninstall, discover]);

  return (
    <div className="p-4">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <h3 className="text-sm" style={{ color: 'var(--text-primary)' }}>
            插件管理
          </h3>
          <span
            className="text-[9px] px-1.5 py-0.5 rounded font-medium"
            style={{
              backgroundColor: sandboxInfo?.sandbox_enabled
                ? 'rgba(16,185,129,0.15)'
                : 'rgba(239,68,68,0.15)',
              color: sandboxInfo?.sandbox_enabled
                ? '#10B981'
                : '#EF4444',
            }}
          >
            {sandboxInfo?.sandbox_enabled ? '沙箱已启用' : '沙箱已禁用'}
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            onClick={handleInstallZip}
            className="pd-btn text-[10px] px-2 py-1 rounded transition-all"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            title="从压缩包安装插件"
          >
            + 安装
          </button>
          <button
            onClick={() => setShowSandbox(!showSandbox)}
            className="pd-btn text-[10px] px-2 py-1 rounded"
            style={{
              backgroundColor: showSandbox ? 'var(--accent-light)' : 'var(--bg-tertiary)',
              color: showSandbox ? 'var(--accent)' : 'var(--text-tertiary)',
            }}
            title={showSandbox ? '收起沙箱信息' : '查看沙箱信息'}
          >
            沙箱
          </button>
          <button
            onClick={discover}
            className="pd-btn text-[10px] px-2 py-1 rounded transition-all"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            刷新
          </button>
        </div>
      </div>

      {showSandbox ? (
        <SandboxInfoPanel />
      ) : (
        <>
          {installStatus && (
            <div
              className="text-[10px] py-1.5 px-3 rounded mb-3"
              style={{
                backgroundColor: installStatus.startsWith('安装失败')
                  ? 'rgba(239,68,68,0.1)'
                  : 'rgba(16,185,129,0.1)',
                color: installStatus.startsWith('安装失败') ? '#EF4444' : '#10B981',
              }}
            >
              {installStatus}
            </div>)}

          {loading && (
            <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>
              扫描插件中...
            </div>)}

          {error && (
            <div
              className="text-xs py-2 px-3 rounded mb-3"
              style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}
            >
              {error}
            </div>)}

          {!loading && plugins.length === 0 && (
            <div className="text-xs py-8 text-center" style={{ color: 'var(--text-tertiary)' }}>
              <p className="mb-2">暂无已安装的插件</p>
              <p className="text-[10px] mb-3">
                点击「+ 安装」按钮选择 .zip 压缩包安装插件
              </p>
              <p className="text-[10px]">
                或将插件目录放置到{' '}
                <code className="px-1 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                  ~/.pilotdesk/plugins/
                </code>
              </p>
            </div>)}

          <div className="space-y-2">
            {plugins.map((plugin) => {
              const loadState = pluginRegistry.getPluginLoadState(plugin.path);
              return (
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
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
                          {plugin.manifest.name}
                        </span>
                        <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                          v{plugin.manifest.version}
                        </span>
                        {loadState?.loaded && (
                          <span className="text-[9px] px-1 py-0.5 rounded shrink-0" style={{ backgroundColor: 'rgba(16,185,129,0.1)', color: '#10B981' }}>
                            已加载
                          </span>)}
                      </div>
                      <p className="text-[10px] mt-0.5 line-clamp-2 pr-2" style={{ color: 'var(--text-secondary)' }}>
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
                      {plugin.manifest.contributes && (
                        <div className="flex items-center gap-1.5 mt-1">
                          {plugin.manifest.contributes.panels && plugin.manifest.contributes.panels.length > 0 && (
                            <span className="text-[9px] px-1 py-0.5 rounded" style={{ backgroundColor: 'rgba(59,130,246,0.1)', color: '#3B82F6' }}>
                              {plugin.manifest.contributes.panels.length} 面板
                            </span>)}
                          {plugin.manifest.contributes.commands && plugin.manifest.contributes.commands.length > 0 && (
                            <span className="text-[9px] px-1 py-0.5 rounded" style={{ backgroundColor: 'rgba(139,92,246,0.1)', color: '#8B5CF6' }}>
                              {plugin.manifest.contributes.commands.length} 命令
                            </span>)}
                          {plugin.manifest.contributes.hooks && plugin.manifest.contributes.hooks.length > 0 && (
                            <span className="text-[9px] px-1 py-0.5 rounded" style={{ backgroundColor: 'rgba(245,158,11,0.1)', color: '#F59E0B' }}>
                              {plugin.manifest.contributes.hooks.length} 钩子
                            </span>)}
                        </div>)}
                    </div>
                    <div className="flex items-center gap-1 shrink-0 ml-2">
                      <button
                        onClick={() => handleUninstall(plugin)}
                        className="pd-btn px-1.5 py-1 rounded text-[10px]"
                        style={{ color: '#EF4444' }}
                        title="卸载插件"
                      >
                        <Trash2 size={11} />
                      </button>
                      <button
                        onClick={() => (plugin.enabled ? disable(plugin.manifest.id) : enable(plugin.manifest.id))}
                        className="px-2.5 py-1 rounded text-[10px] transition-all"
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
                  </div>
                  {plugin.error && (
                    <p className="text-[10px] mt-1.5" style={{ color: '#EF4444' }}>
                      {plugin.error}
                    </p>)}
                  {plugin.has_unauthorized_permissions && (
                    <p className="text-[10px] mt-1" style={{ color: '#F59E0B' }}>
                      包含未授权权限声明，请联系插件开发者或检查 manifest.json
                    </p>)}
                  {loadState?.error && (
                    <p className="text-[10px] mt-1" style={{ color: '#EF4444' }}>
                      加载错误: {loadState.error}
                    </p>)}
                </div>
              );
            })}
          </div>
        </>)}
    </div>
  );
}
