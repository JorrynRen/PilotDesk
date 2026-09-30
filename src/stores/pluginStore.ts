import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { PluginInstance, SandboxInfo, PanelContribution, CommandContribution, HookContribution } from '../types/plugin';
import { containsTemplate } from '../utils/templateValue';
import { errorMessage } from '../utils/errorMessage';
import { pluginRegistry } from '../plugin/PluginRegistry';

/** 注册的面板信息 */
export interface RegisteredPanel {
  pluginId: string;
  pluginName: string;
  pluginPath: string;
  /** manifest 顶层 icon（面板贡献点无 icon 时 fallback） */
  pluginIcon?: string;
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
    // 停用的插件不贡献任何东西：面板 / 命令 / 钩子都不该出现在消费方。
    // 消费方有三处：右栏「插件」面板、工作流插件节点的「选择插件」与「选择命令」下拉。
    // 在这里拦掉是单一源头 —— 否则每个消费方都得各记一次"要过滤 enabled"。
    if (!plugin.enabled) continue;

    const contributes = plugin.manifest.contributes;
    if (!contributes) continue;

    if (contributes.panels) {
      for (const panel of contributes.panels) {
        const key = plugin.path + ':' + panel.id;
        panels.set(key, {
          pluginId: plugin.manifest.id,
          pluginName: plugin.manifest.name,
          pluginPath: plugin.path,
          pluginIcon: plugin.manifest.icon,
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

/**
 * 启停插件时同步前端运行时。
 *
 * 后端的 `plugin_enable` / `plugin_disable` 只改标志位与磁盘记录；而插件的
 * **命令处理器、面板组件、前端节点类型注册表**都在 PluginRegistry 的运行时里。
 * 不同步的话，"停用"就只剩 UI 上的一个标记：面板组件还在、命令还能被工作流调起、
 * 它贡献的节点类型仍留在节点面板里。
 */
async function syncPluginRuntime(plugin: PluginInstance | undefined, enabled: boolean): Promise<void> {
  if (!plugin) return;
  if (enabled) {
    await pluginRegistry.loadPlugin(plugin);
  } else {
    await pluginRegistry.unloadPlugin(plugin.path);
  }
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
      set({ error: errorMessage(err), loading: false });
    }
  },

  list: async () => {
    set({ loading: true, error: null });
    try {
      const plugins = await invoke<PluginInstance[]>('plugin_list');
      set({ plugins, loading: false });
      get().refreshRegistrations();
    } catch (err) {
      set({ error: errorMessage(err), loading: false });
    }
  },

  enable: async (id: string) => {
    try {
      await invoke('plugin_enable', { id });
      await get().list();
      // 标志位改了还不够：入口要跟着执行，命令/面板/节点类型才会注册回来
      await syncPluginRuntime(get().plugins.find((p) => p.manifest.id === id), true);
    } catch (err) {
      set({ error: errorMessage(err) });
    }
  },

  disable: async (id: string) => {
    try {
      await invoke('plugin_disable', { id });
      await get().list();
      // 反向：卸载运行时，把它注册过的命令/面板/节点类型一并回收
      await syncPluginRuntime(get().plugins.find((p) => p.manifest.id === id), false);
    } catch (err) {
      set({ error: errorMessage(err) });
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

/** 取插件声明的命令贡献点（来自 manifest 的 contributes.commands） */
export function pluginCommands(pluginId: string): CommandContribution[] {
  const commands = usePluginStore.getState().registeredCommands;
  if (!commands) return [];
  return Array.from(commands.values())
    .filter((command) => command.pluginId === pluginId)
    .map((command) => command.contribution);
}

/** 取插件下指定命令的贡献点 */
export function findPluginCommand(pluginId: string, commandId?: string): CommandContribution | undefined {
  if (!pluginId || !commandId) return undefined;
  return pluginCommands(pluginId).find((command) => command.id === commandId);
}

/** 命令 input 模式声明的参数键；平台据此生成参数表单，并在切换命令时判断哪些键已失效 */
export function commandParamKeys(command?: CommandContribution): string[] {
  return Object.keys(command?.input?.properties || {});
}

/**
 * 用命令声明的 `default` 修正参数：声明了默认值的键，若取值**缺失、空串、或不在该键声明的
 * `enum` 内**（且不是 `{{变量}}`），一律换成默认值。
 *
 * 为什么平台要兜这一层：`input.properties[key].default` 是命令契约的一部分，而"控件的初始值"
 * 可能只停留在插件表单的本地 state 里（或写了别的命令的默认值）。这类偏差的表现是"**选默认项
 * 执行就报错、手动换一个选项就正常**"——用户没碰过的控件，handler 收到的是空值/非法值。
 * 补齐后 handler 拿到的取值与表单显示的一致，且与谁触发（编辑器 / 定时 / 其他插件）无关。
 *
 * 无变化时返回原对象引用，调用方据此判断是否需要写回。
 */
export function applyCommandParamDefaults(
  pluginId: string,
  commandId: string,
  params: Record<string, unknown> | null | undefined,
): Record<string, unknown> {
  const properties = findPluginCommand(pluginId, commandId)?.input?.properties;
  const source: Record<string, unknown> = params || {};
  if (!properties) return source;

  let result: Record<string, unknown> | undefined;
  for (const [key, property] of Object.entries(properties)) {
    if (property.default === undefined) continue;
    const value = source[key];
    if (typeof value === 'string' && containsTemplate(value)) continue;
    const isEmpty = value === undefined || value === null || value === '';
    const enumValues = Array.isArray(property.enum) ? property.enum : [];
    const outOfEnum = enumValues.length > 0 && !enumValues.some((item) => String(item) === String(value));
    if (!isEmpty && !outOfEnum) continue;
    result = { ...(result || source), [key]: property.default };
  }
  return result || source;
}

/** 插件是否可用于工作流节点：声明了配置组件，或至少一个命令声明了参数模式 */
export function pluginSupportsWorkflow(plugin: PluginInstance): boolean {
  const contributes = plugin.manifest?.contributes;
  if (!contributes) return false;
  if (contributes.workflow_config) return true;
  return (contributes.commands || []).some((command) => commandParamKeys(command).length > 0);
}
