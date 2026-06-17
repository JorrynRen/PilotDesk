import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { PluginInstance, SandboxInfo, PanelContribution, CommandContribution, HookContribution } from '../types/plugin';

/** 注册的面板信息 */
export interface RegisteredPanel {
  pluginId: string;
  pluginName: string;
  pluginPath: string;
  contribution: PanelContribution;
}

/** 注册的命令信息 */
export interface RegisteredCommand {
  pluginId: string;
  pluginName: string;
  contribution: CommandContribution;
}

/** 注册的钩子信息 */
export interface RegisteredHook {
  pluginId: string;
  pluginName: string;
  pluginPath: string;
  contribution: HookContribution;
}

interface PluginStoreState {
  plugins: PluginInstance[];
  sandboxInfo: SandboxInfo | null;
  loading: boolean;
  error: string | null;

  /** 注册的贡献点 */
  registeredPanels: Map<string, RegisteredPanel>;
  registeredCommands: Map<string, RegisteredCommand>;
  registeredHooks: Map<string, RegisteredHook[]>;

  discover: () => Promise<void>;
  list: () => Promise<void>;
  enable: (id: string) => Promise<void>;
  disable: (id: string) => Promise<void>;
  installZip: (zipPath: string) => Promise<PluginInstance>;
  uninstall: (id: string) => Promise<void>;
  fetchSandboxInfo: () => Promise<void>;
  setSandboxEnabled: (enabled: boolean) => Promise<void>;

  /** 刷新所有注册（基于当前 plugins 列表重建） */
  refreshRegistrations: () => void;
}

/** 构建插件注册表（面板/命令/钩子） */
function buildRegistrations(plugins: PluginInstance[]) {
  const panels = new Map<string, RegisteredPanel>();
  const commands = new Map<string, RegisteredCommand>();
  const hooks = new Map<string, RegisteredHook[]>();

  for (const plugin of plugins) {
    if (plugin.has_unauthorized_permissions) continue;

    const contributes = plugin.manifest.contributes;
    if (!contributes) continue;

    if (contributes.panels) {
      for (const panel of contributes.panels) {
        const key = plugin.path + ':' + panel.id;
        panels.set(key, {
          pluginId: plugin.manifest.id,
          pluginName: plugin.manifest.name,
          pluginPath: plugin.path,
          contribution: panel,
        });
      }
    }

    if (contributes.commands) {
      for (const cmd of contributes.commands) {
        const key = plugin.path + ':' + cmd.id;
        commands.set(key, {
          pluginId: plugin.manifest.id,
          pluginName: plugin.manifest.name,
          contribution: cmd,
        });
      }
    }

    if (contributes.hooks) {
      for (const hook of contributes.hooks) {
        const hooksList = hooks.get(hook.event) || [];
        hooksList.push({
          pluginId: plugin.manifest.id,
          pluginName: plugin.manifest.name,
          pluginPath: plugin.path,
          contribution: hook,
        });
        hooks.set(hook.event, hooksList);
      }
    }
  }

  return { panels, commands, hooks };
}

export const usePluginStore = create<PluginStoreState>((set, get) => ({
  plugins: [],
  sandboxInfo: null,
  loading: false,
  error: null,
  registeredPanels: new Map(),
  registeredCommands: new Map(),
  registeredHooks: new Map(),

  refreshRegistrations: () => {
    const { plugins } = get();
    const { panels, commands, hooks } = buildRegistrations(plugins);
    set({ registeredPanels: panels, registeredCommands: commands, registeredHooks: hooks });
  },

  discover: async () => {
    set({ loading: true, error: null });
    try {
      const plugins = await invoke<PluginInstance[]>('plugin_discover');
      set({ plugins, loading: false });
      get().refreshRegistrations();
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  list: async () => {
    set({ loading: true, error: null });
    try {
      const plugins = await invoke<PluginInstance[]>('plugin_list');
      set({ plugins, loading: false });
      get().refreshRegistrations();
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  enable: async (id: string) => {
    try {
      await invoke('plugin_enable', { id });
      await get().list();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  disable: async (id: string) => {
    try {
      await invoke('plugin_disable', { id });
      await get().list();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  installZip: async (zipPath: string) => {
    const instance = await invoke<PluginInstance>('plugin_install_zip', { zipPath });
    return instance;
  },

  uninstall: async (id: string) => {
    await invoke('plugin_uninstall', { id });
  },

  fetchSandboxInfo: async () => {
    try {
      const info = await invoke<SandboxInfo>('plugin_get_sandbox_info');
      set({ sandboxInfo: info });
    } catch (err) {
      console.warn('Failed to fetch sandbox info:', err);
    }
  },

  setSandboxEnabled: async (enabled: boolean) => {
    try {
      await invoke('plugin_set_sandbox_enabled', { enabled });
      await get().fetchSandboxInfo();
      await get().discover();
    } catch (err) {
      console.warn('Failed to set sandbox enabled:', err);
    }
  },
}));
