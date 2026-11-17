import { useEffect, useState, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  AlertTriangle,
  BookOpen,
  Copy,
  Package,
  Plus,
  RefreshCw,
  Shield,
  ShieldOff,
  Store,
  Trash2,
  X,
} from 'lucide-react';
import { usePluginStore, pluginSupportsWorkflow } from '../../stores/pluginStore';
import { PluginReadmeDialog } from './PluginReadmeDialog';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import { confirmDialog } from '../../stores/confirmStore';
import { PluginIcon as SharedPluginIcon } from './PluginIcon';
import type { PermissionCheck, PluginInstance } from '../../types/plugin';
import { errorMessage } from '../../utils/errorMessage';
import { SettingsButton, SettingsSection, SettingsToggle } from '../settings';

/** 单个插件面板入口（来自 RightPanel 按插件分组的 registeredPanels） */
export interface PluginPanelEntry {
  uniqueKey: string;
  panelId: string;
  title: string;
  icon?: string;
  pluginId: string;
}

interface PluginManagerProps {
  /** 各插件已注册的面板入口：用于在插件卡片中直接跳转打开面板 */
  panelsByPluginId?: Map<string, PluginPanelEntry[]>;
  /** 点击面板入口的回调：参数为 uniqueKey = `${pluginId}:${panelId}` */
  onOpenPanel?: (uniqueKey: string) => void;
}

function PermissionBadge({ check }: { check: PermissionCheck }) {
  return (
    <span
      className="inline-flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded cursor-help"
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
      {check.reason?.includes('高风险') && <AlertTriangle size={9} />}
    </span>
  );
}

function SandboxInfoPanel({ onClose }: { onClose: () => void }) {
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
    <SettingsSection
      title="沙箱保护"
      description="插件只能使用清单声明且已授权的权限；目录与清单大小也会在加载前校验。"
      actions={
        <>
          <span
            className="text-[10px] px-2 py-1 rounded-md font-medium"
            style={{
              backgroundColor: sandboxInfo.sandbox_enabled
                ? 'rgba(16,185,129,0.12)'
                : 'rgba(239,68,68,0.10)',
              color: sandboxInfo.sandbox_enabled ? '#10B981' : '#EF4444',
            }}
          >
            {sandboxInfo.sandbox_enabled ? '已启用' : '已禁用'}
          </span>
          <SettingsButton
            onClick={() => void setSandboxEnabled(!sandboxInfo.sandbox_enabled)}
            variant={sandboxInfo.sandbox_enabled ? 'secondary' : 'success'}
            icon={sandboxInfo.sandbox_enabled ? <ShieldOff size={12} /> : <Shield size={12} />}
          >
            {sandboxInfo.sandbox_enabled ? '禁用沙箱' : '启用沙箱'}
          </SettingsButton>
          <SettingsButton
            onClick={() => void fetchSandboxInfo()}
            icon={<RefreshCw size={12} />}
            title="刷新沙箱信息"
          >
            刷新
          </SettingsButton>
          <SettingsButton
            onClick={onClose}
            variant="ghost"
            icon={<X size={12} />}
            title="收起沙箱信息"
          />
        </>
      }
    >
      <div className="space-y-3 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
        <div className="pd-settings-inline-note flex items-center gap-2">
          <span className="shrink-0 font-medium">插件目录</span>
          <code className="min-w-0 flex-1 break-all font-mono text-[10px]" title={sandboxInfo.plugins_dir}>
            {sandboxInfo.plugins_dir}
          </code>
          <SettingsButton
            onClick={handleCopyPath}
            variant="ghost"
            icon={copied ? undefined : <Copy size={12} />}
            title={copied ? '已复制' : '复制插件目录'}
          >
            {copied ? '已复制' : undefined}
          </SettingsButton>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <span className="px-2 py-1 rounded-md" style={{ backgroundColor: 'var(--bg-field)' }}>
            清单上限 <strong>{(sandboxInfo.max_manifest_size / 1024).toFixed(0)} KB</strong>
          </span>
          <span className="px-2 py-1 rounded-md" style={{ backgroundColor: 'var(--bg-field)' }}>
            已注册权限 <strong>{sandboxInfo.allowed_permissions.length}</strong>
          </span>
        </div>

        <div>
          <div className="mb-1.5 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>允许的插件权限</div>
          <div className="flex flex-wrap gap-1.5">
            {sandboxInfo.allowed_permissions.map((perm) => {
              const isHighRisk = sandboxInfo.high_risk_permissions.includes(perm);
              return (
                <span
                  key={perm}
                  className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded break-all"
                  style={{
                    backgroundColor: isHighRisk ? 'rgba(245,158,11,0.15)' : 'var(--accent-light)',
                    color: isHighRisk ? '#F59E0B' : 'var(--accent)',
                  }}
                  title={isHighRisk ? '高风险权限' : '低风险权限'}
                >
                  {perm}
                  {isHighRisk && <AlertTriangle size={9} />}
                </span>
              );
            })}
          </div>
        </div>
      </div>
    </SettingsSection>
  );
}

function PluginIcon({ plugin }: { plugin: PluginInstance }) {
  const [iconSrc, setIconSrc] = useState<string | null>(null);
  const [iconError, setIconError] = useState(false);

  // 优先使用 manifest 顶层 icon 字段，fallback 到第一个面板的 icon
  // （抽出变量：依赖数组需要可静态检查的简单表达式）
  const panelIcon = plugin.manifest.contributes?.panels?.[0]?.icon;

  useEffect(() => {
    const iconPath = plugin.manifest.icon || panelIcon;
    if (iconPath) {
      // 去掉 "icon: " 前缀（如果存在）
      const cleanPath = iconPath.replace(/^icon:\s*/i, '');
      invoke<string>('plugin_read_icon_file', {
        pluginId: plugin.manifest.id,
        iconPath: cleanPath,
      })
        .then((dataUrl) => setIconSrc(dataUrl))
        .catch(() => setIconError(true));
    } else {
      // 不在 effect 体内同步 setState（`react-hooks/set-state-in-effect` 判为级联渲染）：
      // 推到微任务 —— 同一个任务、早于绘制，行为一致。
      void Promise.resolve().then(() => setIconError(true));
    }
  }, [plugin.manifest.id, plugin.manifest.icon, panelIcon]);

  return (
    <div
      title={plugin.manifest.name}
      style={{
        width: 20,
        height: 20,
        borderRadius: '4px',
        backgroundColor: 'var(--bg-tertiary)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexShrink: 0,
        overflow: 'hidden',
        border: '1px solid var(--border)',
      }}
    >
      {iconSrc && !iconError ? (
        <img
          src={iconSrc}
          alt={plugin.manifest.name}
          style={{ width: '100%', height: '100%', objectFit: 'cover' }}
          onError={() => setIconError(true)}
        />
      ) : (
        <Package size={14} style={{ color: 'var(--text-tertiary)' }} />
      )}
    </div>
  );
}

export function PluginManager({ panelsByPluginId, onOpenPanel }: PluginManagerProps) {
  const navigate = useNavigate();
  const { plugins, loading, error, discover, enable, disable, installZip, uninstall } = usePluginStore();
  const [showSandbox, setShowSandbox] = useState(false);
  const [installStatus, setInstallStatus] = useState<string | null>(null);
  const [readmePlugin, setReadmePlugin] = useState<PluginInstance | null>(null);

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
      
      const selected = await openDialog({
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
      setInstallStatus('安装失败: ' + errorMessage(err));
    }
  }, [installZip, discover]);

  const handleUninstall = useCallback(async (plugin: PluginInstance) => {
    // 卸载前二次确认（统一走全局确认弹窗）
    const ok = await confirmDialog({
      title: '确认卸载',
      message: `确定要卸载插件「${plugin.manifest.name}」吗？此操作将删除插件文件。`,
      confirmText: '卸载',
    });
    if (!ok) return;
    try {
      await uninstall(plugin.manifest.id);
      await pluginRegistry.unloadPlugin(plugin.path);
      await discover();
    } catch (err) {
      console.error('Uninstall failed:', err);
    }
  }, [uninstall, discover]);

  return (
    <div className="settings-page-stack">
      <SettingsSection
        title="插件管理"
        description="安装、卸载与启停本地插件。在线插件请在资源市集的插件页获取。"
        actions={
          <>
            <SettingsButton
              onClick={() => void handleInstallZip()}
              variant="primary"
              icon={<Plus size={12} />}
              title="从压缩包安装插件"
            >
              安装插件
            </SettingsButton>
            <SettingsButton
              onClick={() => navigate('/market?tab=plugins')}
              icon={<Store size={12} />}
              title="前往资源市集 › 插件"
            >
              资源市集
            </SettingsButton>
            <SettingsButton
              onClick={() => setShowSandbox((value) => !value)}
              variant={showSandbox ? 'secondary' : 'ghost'}
              icon={<Shield size={12} />}
              title={showSandbox ? '收起沙箱信息' : '查看沙箱信息'}
            >
              沙箱
            </SettingsButton>
          </>
        }
      >
        {installStatus && (
          <div
            className="pd-settings-inline-note mb-3"
            style={{
              color: installStatus.startsWith('安装失败') ? 'var(--danger)' : 'var(--success)',
              borderColor: installStatus.startsWith('安装失败') ? 'var(--danger)' : 'var(--success)',
            }}
          >
            {installStatus}
          </div>
        )}

        {error && (
          <div
            className="pd-settings-inline-note mb-3"
            style={{ color: 'var(--danger)', borderColor: 'var(--danger)' }}
          >
            {error}
          </div>
        )}

        <div className="mb-2 flex items-center justify-between gap-2">
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            {loading ? '正在扫描插件…' : `已安装插件${plugins.length > 0 ? ` · ${plugins.length} 个` : ''}`}
          </div>
          <SettingsButton
            variant="ghost"
            onClick={() => void discover()}
            icon={<RefreshCw size={12} />}
            title="刷新插件列表"
          />
        </div>

        {loading ? (
          <div className="pd-settings-empty">
            <Package size={20} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>扫描插件中…</span>
          </div>
        ) : plugins.length === 0 ? (
          <div className="pd-settings-empty">
            <Package size={20} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
              暂无已安装插件
            </p>
            <p className="max-w-[440px] text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              点击「安装插件」选择 .zip 压缩包，或将插件目录放入{' '}
              <code className="px-1 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-field)' }}>
                ~/.pilotdesk/plugins/
              </code>
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            {plugins.map((plugin) => {
              const loadState = pluginRegistry.getPluginLoadState(plugin.path);
              const myPanels = panelsByPluginId?.get(plugin.manifest.id) ?? [];
              return (
                <div
                  key={plugin.manifest.id}
                  className="rounded-md px-3 py-3 transition-colors"
                  style={{
                    backgroundColor: 'var(--bg-raised)',
                    border: '1px solid var(--border)',
                    opacity: plugin.enabled ? 1 : 0.62,
                  }}
                >
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex min-w-0 flex-1 items-center gap-2">
                      <PluginIcon plugin={plugin} />
                      <span className="text-xs truncate font-medium" style={{ color: 'var(--text-primary)' }} title={plugin.manifest.name}>
                        {plugin.manifest.name}
                      </span>
                      <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }} title={`版本 v${plugin.manifest.version}`}>
                        v{plugin.manifest.version}
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      <SettingsButton
                        variant="ghost"
                        onClick={() => setReadmePlugin(plugin)}
                        icon={<BookOpen size={12} />}
                        title="查看 README"
                      />
                      <SettingsButton
                        variant="ghost"
                        onClick={() => void handleUninstall(plugin)}
                        icon={<Trash2 size={12} />}
                        title="卸载插件"
                        style={{ color: 'var(--danger)' }}
                      />
                      {!plugin.enabled && plugin.has_unauthorized_permissions && (
                        <span className="text-[9px]" style={{ color: 'var(--danger)' }}>
                          权限异常
                        </span>
                      )}
                      <SettingsToggle
                        checked={plugin.enabled}
                        onChange={() => void (plugin.enabled ? disable(plugin.manifest.id) : enable(plugin.manifest.id))}
                        disabled={!plugin.enabled && plugin.has_unauthorized_permissions}
                        title={
                          !plugin.enabled && plugin.has_unauthorized_permissions
                            ? '存在未授权权限，无法启用'
                            : plugin.enabled
                              ? '点击禁用插件'
                              : '点击启用插件'
                        }
                        aria-label={plugin.enabled ? '插件已启用，点击禁用' : '插件已禁用，点击启用'}
                      />
                    </div>
                  </div>

                  <div className="my-2.5" style={{ height: 1, backgroundColor: 'var(--border)' }} />

                  <p className="line-clamp-2 break-words text-[10px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
                    {plugin.manifest.description}
                  </p>
                  <div className="mt-2 flex min-w-0 flex-wrap items-center gap-1.5">
                    <span className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-tertiary)' }}>
                      {plugin.manifest.author}
                    </span>
                    {plugin.permission_checks.map((check) => (
                      <PermissionBadge key={check.permission} check={check} />
                    ))}
                  </div>
                  {plugin.manifest.contributes && (
                    <div className="mt-1.5 flex min-w-0 flex-wrap items-center gap-1.5">
                      {plugin.manifest.contributes.panels && plugin.manifest.contributes.panels.length > 0 && (
                        <span className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'rgba(59,130,246,0.1)', color: '#3B82F6' }}>
                          {plugin.manifest.contributes.panels.length} 面板
                        </span>)}
                      {plugin.manifest.contributes.commands && plugin.manifest.contributes.commands.length > 0 && (
                        <span className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'rgba(139,92,246,0.1)', color: '#8B5CF6' }}>
                          {plugin.manifest.contributes.commands.length} 命令
                        </span>)}
                      {plugin.manifest.contributes.hooks && plugin.manifest.contributes.hooks.length > 0 && (
                        <span className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'rgba(245,158,11,0.1)', color: '#F59E0B' }}>
                          {plugin.manifest.contributes.hooks.length} 钩子
                        </span>)}
                      {pluginSupportsWorkflow(plugin) && (
                        <span
                          className="text-[9px] px-1.5 py-0.5 rounded"
                          style={{ backgroundColor: 'rgba(34,197,94,0.1)', color: '#22C55E' }}
                          title="可在工作流的「插件」节点里选用"
                        >
                          工作流可用
                        </span>)}
                    </div>)}

                  {plugin.enabled && onOpenPanel && myPanels.length > 0 && (
                    <div className="mt-2 flex flex-wrap items-center gap-1.5">
                      <span className="shrink-0 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>面板</span>
                      {myPanels.map((panel) => (
                        <button
                          key={panel.uniqueKey}
                          onClick={() => onOpenPanel(panel.uniqueKey)}
                          className="pd-btn inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] transition-colors"
                          style={{
                            backgroundColor: 'var(--bg-field)',
                            color: 'var(--accent)',
                            border: '1px solid var(--border)',
                          }}
                          title={`打开面板：${panel.title}`}
                        >
                          <SharedPluginIcon icon={panel.icon} pluginId={panel.pluginId} size={10} />
                          <span className="max-w-[140px] truncate">{panel.title}</span>
                        </button>
                      ))}
                    </div>
                  )}
                  {plugin.error && <p className="mt-1.5 break-words text-[10px]" style={{ color: 'var(--danger)' }}>{plugin.error}</p>}
                  {plugin.has_unauthorized_permissions && (
                    <p className="mt-1 break-words text-[10px]" style={{ color: 'var(--status-warning)' }}>
                      包含未授权权限声明，请联系插件开发者或检查 manifest.json
                    </p>
                  )}
                  {loadState?.error && <p className="mt-1 break-words text-[10px]" style={{ color: 'var(--danger)' }}>加载错误: {loadState.error}</p>}
                </div>
              );
            })}
          </div>
        )}
      </SettingsSection>

      {showSandbox && <SandboxInfoPanel onClose={() => setShowSandbox(false)} />}

      {readmePlugin && (
        <PluginReadmeDialog
          basePath={readmePlugin.manifest.id}
          pluginName={readmePlugin.manifest.name}
          onClose={() => setReadmePlugin(null)}
        />
      )}
    </div>
  );
}
