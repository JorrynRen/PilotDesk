/**
 * PluginRegistry — 插件贡献点全局注册表
 *
 * 管理插件注册的面板、命令、事件钩子等前端组件。
 * 插件系统通过此注册表实现"前端插件入口"功能。
 */

import type { PluginInstance, PanelContribution, CommandContribution, HookContribution } from '../types/plugin';

// ── 类型定义 ──

/** 已注册的面板实例 */
export interface RegisteredPanel {
  pluginId: string;
  pluginName: string;
  contribution: PanelContribution;
  /** 面板组件（由宿主应用注册） */
  component?: React.ComponentType<{ pluginId: string }>;
}

/** 已注册的命令实例 */
export interface RegisteredCommand {
  pluginId: string;
  pluginName: string;
  contribution: CommandContribution;
  handler?: (...args: unknown[]) => void;
}

/** 已注册的事件钩子实例 */
export interface RegisteredHook {
  pluginId: string;
  pluginName: string;
  contribution: HookContribution;
  handler?: (...args: unknown[]) => void;
}

/** 插件加载状态 */
export interface PluginLoadState {
  pluginId: string;
  loaded: boolean;
  error?: string;
}

// ── 注册表 ──

type Listener = () => void;

class PluginRegistry {
  private panels: Map<string, RegisteredPanel> = new Map();
  private commands: Map<string, RegisteredCommand> = new Map();
  private hooks: Map<string, RegisteredHook[]> = new Map();
  private loadStates: Map<string, PluginLoadState> = new Map();
  private listeners: Set<Listener> = new Set();

  // ── 订阅 ──

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify() {
    this.listeners.forEach((fn) => fn());
  }

  // ── 面板管理 ──

  /** 注册插件面板 */
  registerPanel(plugin: PluginInstance, contribution: PanelContribution): void {
    const key = `${plugin.manifest.id}:${contribution.id}`;
    this.panels.set(key, {
      pluginId: plugin.manifest.id,
      pluginName: plugin.manifest.name,
      contribution,
    });
    this.notify();
  }

  /** 注销插件面板 */
  unregisterPanel(pluginId: string, panelId: string): void {
    const key = `${pluginId}:${panelId}`;
    this.panels.delete(key);
    this.notify();
  }

  /** 注销插件所有面板 */
  unregisterPluginPanels(pluginId: string): void {
    for (const [key, panel] of this.panels) {
      if (panel.pluginId === pluginId) {
        this.panels.delete(key);
      }
    }
    this.notify();
  }

  /** 获取所有已注册面板 */
  getPanels(): RegisteredPanel[] {
    return Array.from(this.panels.values());
  }

  /** 获取指定插件的面板 */
  getPluginPanels(pluginId: string): RegisteredPanel[] {
    return this.getPanels().filter((p) => p.pluginId === pluginId);
  }

  /** 设置面板组件 */
  setPanelComponent(
    pluginId: string,
    panelId: string,
    component: React.ComponentType<{ pluginId: string }>,
  ): void {
    const key = `${pluginId}:${panelId}`;
    const panel = this.panels.get(key);
    if (panel) {
      panel.component = component;
      this.notify();
    }
  }

  // ── 命令管理 ──

  /** 注册插件命令 */
  registerCommand(plugin: PluginInstance, contribution: CommandContribution): void {
    const key = `${plugin.manifest.id}:${contribution.id}`;
    this.commands.set(key, {
      pluginId: plugin.manifest.id,
      pluginName: plugin.manifest.name,
      contribution,
    });
    this.notify();
  }

  /** 注销插件命令 */
  unregisterCommand(pluginId: string, commandId: string): void {
    const key = `${pluginId}:${commandId}`;
    this.commands.delete(key);
    this.notify();
  }

  /** 注销插件所有命令 */
  unregisterPluginCommands(pluginId: string): void {
    for (const [key, cmd] of this.commands) {
      if (cmd.pluginId === pluginId) {
        this.commands.delete(key);
      }
    }
    this.notify();
  }

  /** 获取所有已注册命令 */
  getCommands(): RegisteredCommand[] {
    return Array.from(this.commands.values());
  }

  /** 执行命令 */
  executeCommand(pluginId: string, commandId: string, ...args: unknown[]): void {
    const key = `${pluginId}:${commandId}`;
    const cmd = this.commands.get(key);
    if (cmd?.handler) {
      cmd.handler(...args);
    }
  }

  // ── 事件钩子管理 ──

  /** 注册事件钩子 */
  registerHook(plugin: PluginInstance, contribution: HookContribution): void {
    const hooks = this.hooks.get(contribution.event) || [];
    hooks.push({
      pluginId: plugin.manifest.id,
      pluginName: plugin.manifest.name,
      contribution,
    });
    this.hooks.set(contribution.event, hooks);
    this.notify();
  }

  /** 注销插件所有钩子 */
  unregisterPluginHooks(pluginId: string): void {
    for (const [event, hooks] of this.hooks) {
      const filtered = hooks.filter((h) => h.pluginId !== pluginId);
      if (filtered.length === 0) {
        this.hooks.delete(event);
      } else {
        this.hooks.set(event, filtered);
      }
    }
    this.notify();
  }

  /** 触发事件（调用所有注册的钩子） */
  emitEvent(event: string, ...args: unknown[]): void {
    const hooks = this.hooks.get(event) || [];
    for (const hook of hooks) {
      try {
        hook.handler?.(...args);
      } catch (err) {
        console.warn(`[PluginRegistry] Hook error (${hook.pluginId}/${hook.contribution.handler}):`, err);
      }
    }
  }

  /** 获取指定事件的所有钩子 */
  getHooks(event: string): RegisteredHook[] {
    return this.hooks.get(event) || [];
  }

  // ── 插件生命周期 ──

  /** 加载插件（注册贡献点） */
  loadPlugin(plugin: PluginInstance): void {
    const id = plugin.manifest.id;
    if (this.loadStates.get(id)?.loaded) {
      return; // 已加载
    }

    try {
      // 注册面板贡献点
      if (plugin.manifest.contributes?.panels) {
        for (const panel of plugin.manifest.contributes.panels) {
          this.registerPanel(plugin, panel);
        }
      }

      // 注册命令贡献点
      if (plugin.manifest.contributes?.commands) {
        for (const cmd of plugin.manifest.contributes.commands) {
          this.registerCommand(plugin, cmd);
        }
      }

      // 注册事件钩子
      if (plugin.manifest.contributes?.hooks) {
        for (const hook of plugin.manifest.contributes.hooks) {
          this.registerHook(plugin, hook);
        }
      }

      this.loadStates.set(id, { pluginId: id, loaded: true });
      console.log(`[PluginRegistry] 插件 '${plugin.manifest.name}' 已加载`);
    } catch (err) {
      const msg = String(err);
      this.loadStates.set(id, { pluginId: id, loaded: false, error: msg });
      console.warn(`[PluginRegistry] 插件 '${plugin.manifest.name}' 加载失败:`, msg);
    }

    this.notify();
  }

  /** 卸载插件（注销所有贡献点） */
  unloadPlugin(pluginId: string): void {
    this.unregisterPluginPanels(pluginId);
    this.unregisterPluginCommands(pluginId);
    this.unregisterPluginHooks(pluginId);
    this.loadStates.delete(pluginId);
    this.notify();
  }

  /** 获取插件加载状态 */
  getPluginLoadState(pluginId: string): PluginLoadState | undefined {
    return this.loadStates.get(pluginId);
  }

  /** 批量加载所有已启用的插件 */
  loadAllPlugins(plugins: PluginInstance[]): void {
    for (const plugin of plugins) {
      if (plugin.enabled && !plugin.has_unauthorized_permissions) {
        this.loadPlugin(plugin);
      }
    }
  }

  /** 批量卸载所有插件 */
  unloadAllPlugins(): void {
    const ids = Array.from(this.loadStates.keys());
    for (const id of ids) {
      this.unloadPlugin(id);
    }
  }
}

/** 全局单例 */
export const pluginRegistry = new PluginRegistry();
