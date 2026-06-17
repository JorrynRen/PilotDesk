/**
 * PluginPanelRenderer — 插件面板渲染器
 *
 * 在右侧面板中显示所有已注册的插件面板。
 * 面板数据来自 pluginStore.registeredPanels，组件来自 PluginRegistry。
 */

import { useCallback } from 'react';
import { usePluginStore } from '../../stores/pluginStore';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import { PluginIcon } from './PluginIcon';

interface PluginPanelRendererProps {
  /** 当前选中的插件面板 ID */
  activePanelId?: string;
  /** 面板切换回调 */
  onPanelChange?: (panelId: string) => void;
}

export function PluginPanelRenderer({ activePanelId, onPanelChange }: PluginPanelRendererProps) {
  const registeredPanels = usePluginStore((s) => s.registeredPanels);
  const panels = Array.from(registeredPanels.values());

  /** 获取面板组件 */
  const getComponent = useCallback((panelId: string) => {
    for (const panel of registeredPanels.values()) {
      if (panel.contribution.id === panelId) {
        return pluginRegistry.getPanelComponent(panel.pluginPath, panelId);
      }
    }
    return undefined;
  }, [registeredPanels]);

  if (panels.length === 0) {
    return null;
  }

  const activePanel = panels.find((p) => p.contribution.id === activePanelId);
  const Component = activePanelId ? getComponent(activePanelId) : undefined;

  return (
    <div className="plugin-panels">
      {/* 当前插件标题栏 */}
      {activePanel && (
        <div
          className="flex items-center gap-2 px-3 py-2"
          style={{ borderBottom: '1px solid var(--border)' }}
        >
          <PluginIcon icon={activePanel.contribution.icon} pluginId={activePanel.pluginId} size={16} />
          <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>
            {activePanel.contribution.title}
          </span>
          <span className="text-[9px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            {activePanel.pluginName}
          </span>
        </div>
      )}

      {/* 面板内容 */}
      {activePanel && (
        <div className="p-3">
          {Component ? (
            <Component pluginId={activePanel.pluginId} />
          ) : (
            <div
              className="text-xs py-8 text-center rounded"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
            >
              <p>面板组件未加载</p>
              <p className="text-[10px] mt-1">
                插件面板 '{activePanel.contribution.title}' 需要前端组件注册
              </p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
