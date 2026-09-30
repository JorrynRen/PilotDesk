/**
 * PluginAPI — 插件运行时 API 实现
 *
 * 插件通过此 API 与 PilotDesk 交互。
 * 每个插件实例拥有独立的 API 实例，卸载时自动清理。
 */

import { invoke } from '@tauri-apps/api/core';
import { pluginRegistry } from './PluginRegistry';
import { commandDispatcher } from './CommandDispatcher';
import { globalEventBus } from './GlobalEventBus';
import { PluginFSAPI } from './PluginAPI.fs';
import { PluginShellAPI } from './PluginAPI.shell';
import { PluginAgentAPI } from './PluginAPI.agent';
import type { PanelContribution, CommandHandler, EventHandler, CommandResult } from '../types/plugin';

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

export class PluginAPI {
  readonly ui: {
    addPanel: (config: PanelContribution & { component: React.ComponentType<{ pluginId: string }> }) => void;
    removePanel: (id: string) => void;
    showToast: (message: string, type: 'info' | 'success' | 'error') => void;
  };
  readonly data: {
    invoke: <T>(cmd: string, params?: Record<string, unknown>) => Promise<T>;
  };
  /**
   * 应用事件订阅（宿主 → 插件）。
   * 与 `global` 走**同一条** globalEventBus 通道：`on` 收到宿主事件
   * （session:created / session:deleted / message:sent / workflow:*）与其他插件的广播；
   * `emit` 为兼容保留，等价于 `global.emit`（广播给包括自己在内的所有订阅者）。
   */
  readonly events: {
    on: (event: string, handler: (payload: unknown) => void) => () => void;
    emit: (event: string, ...args: unknown[]) => void;
  };
  readonly storage: PluginStorage;
  readonly fs: PluginFSAPI;
  readonly shell: PluginShellAPI;
  readonly agent: PluginAgentAPI;
  readonly commands: {
    register: (commandId: string, handler: CommandHandler) => void;
    execute: (commandId: string, params?: Record<string, unknown>) => Promise<CommandResult>;
  };
  readonly hooks: {
    on: (event: string, handler: EventHandler) => () => void;
  };
  readonly global: {
    on: (event: string, handler: (payload: unknown) => void) => () => void;
    emit: (event: string, payload?: unknown) => void;
    call: (targetPluginId: string, commandId: string, params?: Record<string, unknown>) => Promise<CommandResult>;
  };

  private pluginPath: string;
  readonly pluginId: string;

  constructor(pluginPath: string, pluginId: string, pluginName: string) {
    this.pluginPath = pluginPath;
    this.pluginId = pluginId;

    this.events = {
      // 订阅宿主事件总线：宿主（如会话/消息/工作流）与其它插件的广播都从这里来
      on: (event, handler) => globalEventBus.on(pluginId, event, handler),
      // 兼容保留：等价于 global.emit
      emit: (event, ...args) => {
        globalEventBus.emit(event, args.length === 1 ? args[0] : args);
      },
    };
    this.storage = new PluginStorage(pluginId);
    this.fs = new PluginFSAPI(pluginId);
    this.shell = new PluginShellAPI(pluginId);
    this.agent = new PluginAgentAPI(pluginId);

    this.ui = {
      addPanel: (config) => {
        pluginRegistry.setPanelComponent(pluginPath, config.id, config.component);
        console.log('[PluginAPI] ' + pluginName + ' 注册面板: ' + config.title);
      },
      removePanel: (id) => {
        pluginRegistry.unsetPanelComponent(pluginPath, id);
      },
      showToast: (message, type) => {
        console.log('[PluginAPI] Toast [' + type + ']: ' + message);
      },
    };

    this.data = {
      // 不再裸透传：统一走 plugin_data_invoke（Rust 侧校验 data:invoke 权限 + 命令白名单）
      invoke: <T>(cmd: string, params?: Record<string, unknown>) => {
        return invoke<T>('plugin_data_invoke', {
          pluginId: this.pluginId,
          command: cmd,
          args: params || {},
        });
      },
    };

    // v2.0: 命令注册与执行
    this.commands = {
      register: (commandId: string, handler: CommandHandler) => {
        commandDispatcher.register(pluginId, commandId, handler);
      },
      execute: (commandId: string, params?: Record<string, unknown>) => {
        return commandDispatcher.execute(pluginId, commandId, params);
      },
    };

    // v2.0: 事件钩子注册 —— 与 api.events.on **等价**（同一实现），
    // 保留该 API 只为兼容已有插件与文档。两者都订阅 globalEventBus。
    this.hooks = {
      on: (event: string, handler: EventHandler) => {
        return globalEventBus.on(pluginId, event, handler);
      },
    };

    // v2.0: 跨插件通信
    this.global = {
      on: (event: string, handler: (payload: unknown) => void) => {
        return globalEventBus.on(pluginId, event, handler);
      },
      emit: (event: string, payload?: unknown) => {
        globalEventBus.emit(event, payload);
      },
      call: (targetPluginId: string, commandId: string, params?: Record<string, unknown>) => {
        return globalEventBus.call(targetPluginId, commandId, params);
      },
    };
  }

  /** 清理所有资源 */
  dispose(): void {
    // events / hooks / global 的订阅都落在同一张 globalEventBus 上，
    // offAll 一次把该插件的全部订阅（含 api.events.on）清干净。
    commandDispatcher.unregisterAll(this.pluginId);
    globalEventBus.offAll(this.pluginId);
  }
}
