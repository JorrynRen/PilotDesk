/**
 * PluginRegistry — 插件面板组件注册表 + 插件运行时
 *
 * 管理面板组件的注册与加载状态。
 * 加载插件时读取并执行入口 JS 文件，调用 onLoad/onUnload 生命周期。
 */

import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { PluginInstance } from '../types/plugin';
import { DefaultPluginPanel } from '../components/plugin/DefaultPluginPanel';
import { PluginAPI } from './PluginAPI';
import { workflowNodeTypeRegistry } from '../utils/WorkflowNodeTypeRegistry';

// ── 类型定义 ──

/** 插件加载状态 */
export interface PluginLoadState {
  pluginId: string;
  loaded: boolean;
  error?: string;
}

/** 插件运行时实例 */
interface PluginRuntime {
  api: PluginAPI;
  module: { onLoad?: (api: PluginAPI) => void | Promise<void>; onUnload?: () => void | Promise<void> } | null;
}

// ── 注册表 ──

class PluginRegistry {
  /** 面板组件映射 key: pluginPath:panelId */
  private panelComponents: Map<string, React.ComponentType<{ pluginId: string }>> = new Map();
  /** 加载状态 key: pluginPath */
  private loadStates: Map<string, PluginLoadState> = new Map();
  /** 运行时实例 key: pluginPath */
  private runtimes: Map<string, PluginRuntime> = new Map();

  // ── 面板组件管理 ──

  setPanelComponent(
    pluginPath: string,
    panelId: string,
    component: React.ComponentType<{ pluginId: string }>,
  ): void {
    this.panelComponents.set(pluginPath + ':' + panelId, component);
  }

  getPanelComponent(pluginPath: string, panelId: string): React.ComponentType<{ pluginId: string }> | undefined {
    return this.panelComponents.get(pluginPath + ':' + panelId);
  }

  unsetPanelComponent(pluginPath: string, panelId: string): void {
    this.panelComponents.delete(pluginPath + ':' + panelId);
  }

  unsetPluginPanelComponents(pluginPath: string): void {
    for (const key of this.panelComponents.keys()) {
      if (key.startsWith(pluginPath + ':')) {
        this.panelComponents.delete(key);
      }
    }
  }

  // ── 加载状态管理 ──

  setLoadState(pluginPath: string, state: PluginLoadState): void {
    this.loadStates.set(pluginPath, state);
  }

  getPluginLoadState(pluginPath: string): PluginLoadState | undefined {
    return this.loadStates.get(pluginPath);
  }

  clearLoadState(pluginPath: string): void {
    this.loadStates.delete(pluginPath);
  }

  clearAllLoadStates(): void {
    this.loadStates.clear();
  }

  // ── 插件 JS 执行 ──

  /**
   * 读取并执行插件入口文件
   * 将 export default { onLoad, onUnload } 转换为可调用的模块
   */
  private async executePluginEntry(plugin: PluginInstance): Promise<{ onLoad?: ((api: PluginAPI) => void | Promise<void>) | undefined; onUnload?: (() => void | Promise<void>) | undefined; } | null> {
    try {
      // 1. 读取入口文件内容
      const source = await invoke<string>('plugin_read_entry', { pluginId: plugin.manifest.id });

      // 2. 将 export default 替换为 return，包装为函数体
      // 这样整个源码（含函数声明）都能被执行
      const wrapped = source.replace(/export\s+default\s*/, 'return ');
      const factory = new Function('React', wrapped);

      // 3. 执行并获取模块对象
      const module = factory(React);
      return {
        onLoad: typeof module.onLoad === 'function' ? module.onLoad : undefined,
        onUnload: typeof module.onUnload === 'function' ? module.onUnload : undefined,
      };
    } catch (err) {
      console.warn('[PluginRegistry] 执行插件 ' + plugin.manifest.name + ' 入口失败:', err);
      return null;
    }
  }

  // ── 插件生命周期 ──

  /** 加载插件 */
  async loadPlugin(plugin: PluginInstance): Promise<void> {
    const key = plugin.path;
    if (this.loadStates.get(key)?.loaded) {
      return;
    }

    try {
      // 1. 注册默认面板组件
      if (plugin.manifest.contributes?.panels) {
        for (const panel of plugin.manifest.contributes.panels) {
          this.setPanelComponent(plugin.path, panel.id, DefaultPluginPanel);
        }
      }

      // 1b. 注册工作流节点类型（如果插件声明了 node_types）
      if (plugin.manifest.contributes?.node_types) {
        workflowNodeTypeRegistry.registerFromPlugin(
          plugin.manifest.id,
          plugin.manifest.contributes.node_types,
          (typeId) => {
            // 使用通用插件节点组件，运行时通过 PluginNodeExecutor 执行
            const PluginNodeComponent: React.FC<{ data: any }> = (props) => {
              return React.createElement('div', { className: 'workflow-node workflow-node--plugin' },
                React.createElement('div', { className: 'workflow-node__header' },
                  React.createElement('span', { className: 'workflow-node__type-badge' }, '[插件]'),
                  React.createElement('span', { className: 'workflow-node__label' }, props.data?.label || typeId),
                ),
              );
            };
            return PluginNodeComponent as React.ComponentType<any>;
          },
        );
      }

      // 2. 执行插件入口 JS
      const api = new PluginAPI(plugin.path, plugin.manifest.id, plugin.manifest.name);
      const module = await this.executePluginEntry(plugin);

      // 3. 调用 onLoad 生命周期
      if (module?.onLoad) {
        await module.onLoad(api);
      }

      // 4. 保存运行时实例
      this.runtimes.set(key, { api, module });

      this.loadStates.set(key, { pluginId: plugin.manifest.id, loaded: true });
      console.log('[PluginRegistry] 插件 ' + plugin.manifest.name + ' (' + plugin.path + ') 已加载');
    } catch (err) {
      const msg = String(err);
      this.loadStates.set(key, { pluginId: plugin.manifest.id, loaded: false, error: msg });
      console.warn('[PluginRegistry] 插件 ' + plugin.manifest.name + ' (' + plugin.path + ') 加载失败: ' + msg);
    }
  }

  /** 卸载插件 */
  async unloadPlugin(pluginPath: string): Promise<void> {
    // 1. 调用 onUnload 生命周期
    const runtime = this.runtimes.get(pluginPath);
    if (runtime?.module?.onUnload) {
      try {
        await runtime.module.onUnload();
      } catch (err) {
        console.warn('[PluginRegistry] 插件卸载 onUnload 失败:', err);
      }
    }

    // 2. 注销工作流节点类型
    if (runtime) {
      workflowNodeTypeRegistry.unregisterPlugin((runtime.api as any).pluginId);
    }

    // 3. 清理 API 资源（自动注销命令/事件/全局订阅）
    runtime?.api.dispose();

    // 3. 注销面板组件
    this.unsetPluginPanelComponents(pluginPath);

    // 4. 清理状态
    this.runtimes.delete(pluginPath);
    this.loadStates.delete(pluginPath);
  }

  /** 批量加载所有插件 */
  async loadAllPlugins(plugins: PluginInstance[]): Promise<void> {
    this.loadStates.clear();
    for (const plugin of plugins) {
      if (!plugin.has_unauthorized_permissions) {
        await this.loadPlugin(plugin);
      }
    }
  }

  /** 批量卸载所有插件 */
  async unloadAllPlugins(): Promise<void> {
    const paths = Array.from(this.loadStates.keys());
    for (const path of paths) {
      await this.unloadPlugin(path);
    }
  }
}

/** 全局单例 */
export const pluginRegistry = new PluginRegistry();
