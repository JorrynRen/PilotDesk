/**
 * PluginAPI — 插件运行时 API 实现
 *
 * 插件通过此 API 与 PilotDesk 交互。
 * 每个插件实例拥有独立的 API 实例，卸载时自动清理。
 */

import { invoke } from '@tauri-apps/api/core';
import { pluginRegistry } from './PluginRegistry';
import type { PanelContribution } from '../types/plugin';

/** 插件存储（基于 localStorage，按插件 ID 隔离） */
class PluginStorage {
  private prefix: string;

  constructor(pluginId: string) {
    this.prefix = 'pilotdesk:plugin:' + pluginId + ':';
  }

  async get(key: string): Promise<string | null> {
    return localStorage.getItem(this.prefix + key);
  }

  async set(key: string, value: string): Promise<void> {
    localStorage.setItem(this.prefix + key, value);
  }

  async delete(key: string): Promise<void> {
    localStorage.removeItem(this.prefix + key);
  }
}

/** 插件事件系统 */
class PluginEventBus {
  private listeners: Map<string, Set<(...args: unknown[]) => void>> = new Map();

  on(event: string, handler: (...args: unknown[]) => void): () => void {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event)!.add(handler);
    return () => {
      this.listeners.get(event)?.delete(handler);
    };
  }

  emit(event: string, ...args: unknown[]): void {
    this.listeners.get(event)?.forEach((handler) => {
      try {
        handler(...args);
      } catch (err) {
        console.warn('[PluginEventBus] Handler error for ' + event + ':', err);
      }
    });
  }

  clear(): void {
    this.listeners.clear();
  }
}

export class PluginAPI {
  readonly ui: {
    addPanel: (config: PanelContribution & { component: React.ComponentType }) => void;
    removePanel: (id: string) => void;
    showToast: (message: string, type: 'info' | 'success' | 'error') => void;
  };
  readonly data: {
    invoke: <T>(cmd: string, params?: Record<string, unknown>) => Promise<T>;
  };
  readonly events: PluginEventBus;
  readonly storage: PluginStorage;

  private pluginPath: string;
  private pluginId: string;

  constructor(pluginPath: string, pluginId: string, pluginName: string) {
    this.pluginPath = pluginPath;
    this.pluginId = pluginId;

    this.events = new PluginEventBus();
    this.storage = new PluginStorage(pluginId);

    this.ui = {
      addPanel: (config) => {
        pluginRegistry.setPanelComponent(pluginPath, config.id, config.component);
        console.log('[PluginAPI] ' + pluginName + ' 注册面板: ' + config.title);
      },
      removePanel: (id) => {
        pluginRegistry.unsetPanelComponent(pluginPath, id);
      },
      showToast: (message, type) => {
        // 使用 Tauri dialog 或自定义 toast
        console.log('[PluginAPI] Toast [' + type + ']: ' + message);
        // 可通过 Tauri 事件发送到前端
      },
    };

    this.data = {
      invoke: <T>(cmd: string, params?: Record<string, unknown>) => {
        return invoke<T>(cmd, params || {});
      },
    };
  }

  /** 清理所有资源 */
  dispose(): void {
    this.events.clear();
  }
}
