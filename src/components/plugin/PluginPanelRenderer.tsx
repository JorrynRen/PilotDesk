/**
 * PluginPanelRenderer — 插件面板渲染器
 *
 * 在右侧面板中显示所有已注册的插件面板。
 * 每个插件面板作为一个独立的标签页。
 */

import { useEffect, useState } from 'react';
import { pluginRegistry, type RegisteredPanel } from '../../plugin/PluginRegistry';

interface PluginPanelRendererProps {
  /** 当前选中的插件面板 ID */
  activePanelId?: string;
  /** 面板切换回调 */
  onPanelChange?: (panelId: string) => void;
}

export function PluginPanelRenderer({ activePanelId, onPanelChange }: PluginPanelRendererProps) {
  const [panels, setPanels] = useState<RegisteredPanel[]>([]);

  useEffect(() => {
    // 初始加载
    setPanels(pluginRegistry.getPanels());

    // 订阅变更
    const unsub = pluginRegistry.subscribe(() => {
      setPanels(pluginRegistry.getPanels());
    });

    return unsub;
  }, []);

  if (panels.length === 0) {
    return null;
  }

  const activePanel = panels.find((p) => p.contribution.id === activePanelId);

  return (
    <div className="plugin-panels">
      {/* 面板标签页导航 */}
      <div
        className="flex items-center gap-0.5 px-2 py-1 overflow-x-auto"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        {panels.map((panel) => (
          <button
            key={`${panel.pluginId}:${panel.contribution.id}`}
            onClick={() => onPanelChange?.(panel.contribution.id)}
            className="px-2 py-1 rounded text-[10px] whitespace-nowrap transition-colors"
            style={{
              color:
                activePanelId === panel.contribution.id
                  ? 'var(--accent)'
                  : 'var(--text-secondary)',
              backgroundColor:
                activePanelId === panel.contribution.id
                  ? 'var(--border)'
                  : 'transparent',
            }}
          >
            {panel.contribution.icon || '📦'} {panel.contribution.title}
          </button>
        ))}
      </div>

      {/* 面板内容 */}
      {activePanel && (
        <div className="p-3">
          <div className="text-[10px] mb-2" style={{ color: 'var(--text-tertiary)' }}>
            来自插件: {activePanel.pluginName}
          </div>
          {activePanel.component ? (
            <activePanel.component pluginId={activePanel.pluginId} />
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
