import { useState } from 'react';
import { Lightbulb, Package, History, RefreshCw } from 'lucide-react';
import { InspirationPanel } from './InspirationPanel';
import { SessionFileHistory } from '../panels/SessionFileHistory';
import { PluginPanelRenderer } from '../plugin/PluginPanelRenderer';
import { usePluginStore } from '../../stores/pluginStore';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

interface RightPanelProps {
  isOpen: boolean;
  /** 当前主视图模式：终端模式下隐藏与 Agent 会话无关的 tab（文件历史/插件面板） */
  mode: 'session' | 'terminal';
}

/**
 * 会话右侧面板。
 *
 * tab 归属原则：**只放与当前会话相关、需要常驻的内容**。
 * - 灵感：会话中随时要取用（插入输入框）。
 * - 文件历史：当前会话的写入快照，运行中会实时刷新、可就地撤销（会话级）。
 * - 插件：仅承载插件提供的**运行时面板**（`contributes.panels`）。插件的安装/卸载/启停/商店
 *   属于全局低频运维，已移到 设置 › 插件管理；面板本身常驻在会话里才顺手。
 * 已移出的 tab（只读浏览类，且有更顺手的会话内入口，不配常驻占位）：
 *   - 记忆 → 输入框工具栏的弹窗（设置 › 记忆管理 才是编辑入口）；
 *   - 技能 → 设置 › 技能管理（会话内取用技能走输入框的技能选择器 Ctrl+K）。
 */
export function RightPanel({ isOpen, mode }: RightPanelProps) {
  const isTerminalMode = mode === 'terminal';
  const [activeTab, setActiveTab] = useState<string>('inspiration');
  const [refreshing, setRefreshing] = useState(false);

  // 直接从 store 订阅 registeredPanels（精确订阅，仅当面板变化时重渲染）
  const registeredPanels = usePluginStore((s) => s.registeredPanels);
  const plugins = usePluginStore((s) => s.plugins);
  const discoverPlugins = usePluginStore((s) => s.discover);

  /** 运行时插件面板入口（按插件 id + 面板 id 唯一）。
   *  `buildRegistrations` 已按 `enabled` 过滤过一次（停用插件不贡献面板），
   *  这里再过滤一次是廉价的兜底：即便某条路径只更新了 `plugins` 而没重建注册表，右栏也不会漏出停用插件的面板。 */
  const enabledPluginIds = new Set(plugins.filter((p) => p.enabled).map((p) => p.manifest.id));
  const panelList = Array.from(registeredPanels.values())
    .filter((p) => enabledPluginIds.has(p.pluginId))
    .map((p) => ({
      uniqueKey: `${p.pluginId}:${p.contribution.id}`,
      panelId: p.contribution.id,
      title: p.contribution.title,
    }));
  const [activePanelKey, setActivePanelKey] = useState<string | null>(null);
  // 未显式选择时用第一个面板（面板消失后自动回落到新的第一个，不留空白）
  const activePanelEntry = panelList.find((p) => p.uniqueKey === activePanelKey) ?? panelList[0] ?? null;
  const showPluginTab = !isTerminalMode && panelList.length > 0;

  /**
   * 刷新插件。
   *
   * 两个动作缺一不可：
   * 1. `discover()` 重新扫描磁盘 → 更新列表与各贡献点（新装的插件在这里才被看见）；
   * 2. `loadAllPlugins()` 执行插件入口 → `api.ui.addPanel()` 才会注册**真实面板组件**。
   * 只做第 1 步的话，面板仍是"未加载"占位（App 只在启动时统一加载一次），
   * 这正是"新装插件不刷新就出不来"的原因。
   */
  const refreshPlugins = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await discoverPlugins();
      await pluginRegistry.loadAllPlugins(usePluginStore.getState().plugins);
      showToast('插件已刷新', 'success');
    } catch (err) {
      showToast(`插件刷新失败: ${errorMessage(err)}`, 'error');
    } finally {
      setRefreshing(false);
    }
  };

  if (!isOpen) return null;

  // 插件面板全部消失（插件被停用/卸载）、或终端模式下隐藏了文件历史时，别让面板停在空白上
  const effectiveTab =
    (activeTab === 'plugins' && !showPluginTab) || (activeTab === 'files' && isTerminalMode)
      ? 'inspiration'
      : activeTab;

  const renderContent = () => {
    switch (effectiveTab) {
      case 'files':
        return <SessionFileHistory />;
      case 'plugins': {
        if (!activePanelEntry) return null;
        return (
          <div className="h-full flex flex-col overflow-hidden">
            {/* 插件一多，平铺的按钮条会把整个面板撑满 —— 统一走下拉。
                右侧是刷新：刚安装的插件必须刷新（重新发现 + 执行入口）才会注册进这里。 */}
            <div
              className="flex items-center gap-1 px-2 py-1.5 shrink-0"
              style={{ borderBottom: '1px solid var(--border)' }}
            >
              <Select
                size="xs"
                className="flex-1 min-w-0"
                value={activePanelEntry.uniqueKey}
                onChange={setActivePanelKey}
                options={panelList.map((p) => ({ value: p.uniqueKey, label: p.title }))}
                title={activePanelEntry.title}
                panelMinWidth={160}
              />
              <button
                onClick={refreshPlugins}
                disabled={refreshing}
                className="pd-btn p-1 rounded shrink-0"
                style={{ color: 'var(--text-secondary)', cursor: refreshing ? 'wait' : 'pointer' }}
                title="重新发现并加载插件（刚安装的插件要刷新后才会出现在这里）"
              >
                <RefreshCw size={12} className={refreshing ? 'animate-spin' : undefined} />
              </button>
            </div>
            <div className="flex-1 overflow-hidden">
              <PluginPanelRenderer activePanelId={activePanelEntry.panelId} />
            </div>
          </div>
        );
      }
      case 'inspiration':
        return <InspirationPanel />;
      default:
        return <InspirationPanel />;
    }
  };

  return (
    <aside
      className="w-[260px] flex flex-col shrink-0 rounded-lg"
      // 壳层风格：去掉 border-left，靠"底色 + 圆角 + 相邻面板之间的 8px 缝"区分。
      // 这里**不加 overflow-hidden**：面板里挂着插件面板/灵感库等，可能出现绝对定位的下拉，
      // 裁切会把它们切掉；面板内各层底色基本透明，圆角处不会露出色块。
      style={{ backgroundColor: 'var(--bg-side)' }}
    >
      {/* Header */}
      <div className="flex items-center px-3 h-9 gap-0.5" style={{ borderBottom: '1px solid var(--border)' }}>
        {/* Fixed tabs */}
        <button
          onClick={() => setActiveTab('inspiration')}
          className="pd-btn px-2 py-1 rounded text-xs shrink-0"
          style={{
            color: effectiveTab === 'inspiration' ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: effectiveTab === 'inspiration' ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <Lightbulb size={12} />
          灵感
        </button>
        {/* 当前会话的文件修改历史（运行中实时刷新 + 逐条撤销） */}
        {!isTerminalMode && (
          <button
            onClick={() => setActiveTab('files')}
            className="pd-btn px-2 py-1 rounded text-xs shrink-0"
            style={{
              color: effectiveTab === 'files' ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: effectiveTab === 'files' ? 'var(--accent-light)' : 'transparent',
            }}
            title="当前会话的文件修改历史（可撤销到修改前版本）"
          >
            <History size={12} />
            文件历史
          </button>
        )}
        {/* 无运行时面板的插件不占位：管理入口在 设置 › 插件管理 */}
        {showPluginTab && (
          <button
            onClick={() => setActiveTab('plugins')}
            className="pd-btn px-2 py-1 rounded text-xs shrink-0"
            style={{
              color: effectiveTab === 'plugins' ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: effectiveTab === 'plugins' ? 'var(--accent-light)' : 'transparent',
            }}
            title="插件提供的运行时面板（插件安装与启停在 设置 › 插件管理）"
          >
            <Package size={12} />
            插件
          </button>
        )}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {renderContent()}
      </div>
    </aside>
  );
}
