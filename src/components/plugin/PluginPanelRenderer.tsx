/**
 * PluginPanelRenderer — 插件面板渲染器
 *
 * 在右侧面板中显示所有已注册的插件面板。
 * 面板数据来自 pluginStore.registeredPanels，组件来自 PluginRegistry。
 */

import React from 'react';
import { ArrowLeft } from 'lucide-react';
import { usePluginStore } from '../../stores/pluginStore';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import { PluginIcon } from './PluginIcon';

interface PluginPanelRendererProps {
  /** 当前选中的插件面板 ID */
  activePanelId?: string;
  /** 返回插件列表（面板二级视图的返回入口） */
  onBack?: () => void;
}

export function PluginPanelRenderer({ activePanelId, onBack }: PluginPanelRendererProps) {
  const registeredPanels = usePluginStore((s) => s.registeredPanels);
  const panels = Array.from(registeredPanels.values());

  if (panels.length === 0) {
    return null;
  }

  const activePanel = panels.find((p) => p.contribution.id === activePanelId);
  // 面板组件由注册表按 pluginPath 解析（引用稳定，不随渲染重建）
  const Component = activePanel
    ? pluginRegistry.getPanelComponent(activePanel.pluginPath, activePanel.contribution.id)
    : undefined;

  return (
    <div className="plugin-panels" style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
      {/* 当前插件标题栏 */}
      {activePanel && (
        <div
          className="flex items-center gap-2 px-3 py-2"
          style={{ borderBottom: '1px solid var(--border)' }}
        >
          {onBack && (
            <button
              onClick={onBack}
              className="pd-btn px-1.5 py-0.5 rounded shrink-0"
              style={{ color: 'var(--text-secondary)' }}
              title="返回插件列表"
            >
              <ArrowLeft size={12} />
            </button>
          )}
          <PluginIcon icon={activePanel.contribution.icon || activePanel.pluginIcon} pluginId={activePanel.pluginId} size={16} />
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
        <div className="p-3" style={{ flex: 1, overflowY: "auto" }}>
          {Component ? (
            React.createElement(Component, { pluginId: activePanel.pluginId })
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
