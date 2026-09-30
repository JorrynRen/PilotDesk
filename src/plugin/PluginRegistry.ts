/**
 * PluginRegistry — 插件面板组件注册表 + 插件运行时
 *
 * 管理面板组件的注册与加载状态。
 * 加载插件时读取并执行入口 JS 文件，调用 onLoad/onUnload 生命周期。
 */

import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { PluginInstance, WorkflowConfigProps } from '../types/plugin';
import type { WorkflowNode } from '../types/workflow';
import { DefaultPluginPanel } from '../components/plugin/DefaultPluginPanel';
import { errorMessage } from '../utils/errorMessage';
import { PluginAPI } from './PluginAPI';
import { workflowNodeTypeRegistry } from '../utils/WorkflowNodeTypeRegistry';

// ── 类型定义 ──

/** 插件加载状态 */
export interface PluginLoadState {
  pluginId: string;
  loaded: boolean;
  error?: string;
}

/**
 * 插件入口 JS 执行后得到的模块对象。
 * 生命周期函数是约定字段；其余导出（如 workflow_config 声明的配置组件）由插件任意提供，
 * 运行时无法静态约束，读取处需自行收窄。
 */
interface PluginEntryModule {
  onLoad?: (api: PluginAPI) => void | Promise<void>;
  onUnload?: () => void | Promise<void>;
  [name: string]: unknown;
}

/** 插件运行时实例 */
interface PluginRuntime {
  api: PluginAPI;
  module: PluginEntryModule | null;
}

// ── 插件 JS 执行 ──

/**
 * 插件代码执行时被"以参数形式遮蔽"的危险全局标识符。
 *
 * 插件代码作为 `new Function` 的函数体运行，这些名字会以同名参数传入：
 * 插件里写 `window`/`fetch`/`eval` 命中的是宿主传入的参数，而不是宿主 realm 的真实全局。
 *
 * 注意：这只是**纵深防御**，不是绝对边界 —— 插件与宿主仍在同一个 JS realm，
 * 无法在语言层面彻底阻断逃逸（例如通过未遮蔽的内建对象原型链）。真正的安全边界
 * 在 Rust 侧（命令层的权限 + 白名单校验）。不要据此对外宣称"已完全沙箱"。
 */
const PLUGIN_SHADOWED_GLOBALS = [
  'window',
  'globalThis',
  'self',
  'top',
  'parent',
  'frameElement',
  'document',
  'fetch',
  'XMLHttpRequest',
  'WebSocket',
  'localStorage',
  'sessionStorage',
  'indexedDB',
  'Function',
  // 注意：**不能**把 `eval` 放进来。函数体是严格模式，而严格模式下 `eval` / `arguments`
  // 不允许作形参名，会导致 `new Function(...)` 直接抛
  // `SyntaxError: Unexpected eval or arguments in strict mode` ——
  // 那样每个插件的入口都执行不了（onLoad 不跑、addPanel 不注册），面板会静默回落成占位。
  // 漏掉的 `eval` 影响很小：直接 `eval('window')` 仍在**当前函数作用域**求值，
  // `window` 照样命中被遮蔽的形参；真正的逃逸口是 `Function` / 原型链，那些已遮蔽或本就在 Rust 侧兜住。
] as const;

/** 给插件的最小只读 navigator 替身：只放行"写剪贴板"这类低风险能力 */
const PLUGIN_NAVIGATOR_STANDIN = Object.freeze({
  userAgent: 'PilotDesk/PluginSandbox',
  clipboard: Object.freeze({
    writeText: (text: string) => navigator.clipboard.writeText(String(text)),
  }),
});

class PluginRegistry {
  /** 面板组件映射 key: pluginPath:panelId */
  private panelComponents: Map<string, React.ComponentType<{ pluginId: string }>> = new Map();
  /** 加载状态 key: pluginPath */
  private loadStates: Map<string, PluginLoadState> = new Map();
  /** 运行时实例 key: pluginPath */
  private runtimes: Map<string, PluginRuntime> = new Map();
  /** 工作流配置组件 key: pluginId（插件级默认表单）或 pluginId::commandId（命令级表单） */
  private workflowConfigComponents: Map<string, React.ComponentType<WorkflowConfigProps>> = new Map();

  // ── 工作流配置组件管理 ──

  private static workflowConfigKey(pluginId: string, commandId?: string): string {
    return commandId ? pluginId + '::' + commandId : pluginId;
  }

  /** 注册配置组件；commandId 为空表示该插件所有命令的默认表单 */
  setWorkflowConfig(
    pluginId: string,
    component: React.ComponentType<WorkflowConfigProps>,
    commandId?: string,
  ): void {
    this.workflowConfigComponents.set(PluginRegistry.workflowConfigKey(pluginId, commandId), component);
  }

  /** 取配置组件：优先命令级，回退插件级 */
  getWorkflowConfig(pluginId: string, commandId?: string): React.ComponentType<WorkflowConfigProps> | undefined {
    const commandComponent = commandId
      ? this.workflowConfigComponents.get(PluginRegistry.workflowConfigKey(pluginId, commandId))
      : undefined;
    return commandComponent ?? this.workflowConfigComponents.get(pluginId);
  }

  unsetWorkflowConfig(pluginId: string): void {
    this.workflowConfigComponents.delete(pluginId);
    for (const key of Array.from(this.workflowConfigComponents.keys())) {
      if (key.startsWith(pluginId + '::')) {
        this.workflowConfigComponents.delete(key);
      }
    }
  }

  /** 获取插件的 PluginAPI 实例 */
  getPluginAPI(pluginId: string): PluginAPI | null {
    for (const [, runtime] of this.runtimes) {
      if (runtime.api.pluginId === pluginId) {
        return runtime.api;
      }
    }
    return null;
  }

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

  /** 通过插件路径获取第一个面板组件（供工作流节点配置渲染插件 UI） */
  getFirstPanelByPluginPath(pluginPath: string): React.ComponentType<{ pluginId: string }> | undefined {
    for (const [key, comp] of this.panelComponents.entries()) {
      if (key.startsWith(pluginPath + ':')) {
        return comp;
      }
    }
    return undefined;
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

  /**
   * 读取并执行插件入口文件
   * 将 export default { onLoad, onUnload } 转换为可调用的模块
   */
  private async executePluginEntry(plugin: PluginInstance): Promise<PluginEntryModule | null> {
    try {
      // 1. 读取入口文件内容
      const source = await invoke<string>('plugin_read_entry', { pluginId: plugin.manifest.id });

      // 2. 将 export default 替换为 return，包装为函数体
      const wrapped = source.replace(/export\s+default\s*/, 'return ');
      // 3. 危险全局以"同名参数"形式遮蔽，并强制严格模式（见 PLUGIN_SHADOWED_GLOBALS 注释）
      const factory = new Function(
        ...PLUGIN_SHADOWED_GLOBALS,
        'navigator',
        'React',
        "'use strict';\n" + wrapped,
      );

      // 4. 执行并获取完整模块对象（含 onLoad/onUnload/PluginNodeConfig/...）
      const shadowArgs: unknown[] = PLUGIN_SHADOWED_GLOBALS.map(() => undefined);
      const module = factory(...shadowArgs, PLUGIN_NAVIGATOR_STANDIN, React);
      return module;
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
    // 停用的插件不执行入口：命令处理器、面板组件、前端节点类型都不注册。
    // 刻意不写 loadStates（留空 = 未加载），这样重新启用时还能被正常加载。
    if (!plugin.enabled) {
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
            const PluginNodeComponent: React.FC<{ data: WorkflowNode }> = (props) => {
              return React.createElement('div', { className: 'workflow-node workflow-node--plugin' },
                React.createElement('div', { className: 'workflow-node__header' },
                  React.createElement('span', { className: 'workflow-node__type-badge' }, '[插件]'),
                  React.createElement('span', { className: 'workflow-node__label' }, props.data?.label || typeId),
                ),
              );
            };
            return PluginNodeComponent as React.ComponentType<{ data: WorkflowNode; selected?: boolean }>;
          },
        );
      }

      // 2. 执行插件入口 JS
      const api = new PluginAPI(plugin.path, plugin.manifest.id, plugin.manifest.name);
      const module = await this.executePluginEntry(plugin);

      // 2b. 注册工作流配置组件（必须在 executePluginEntry 之后）
      const workflowConfig = plugin.manifest.contributes?.workflow_config;
      if (workflowConfig) {
        const registerComponent = (componentName: string | undefined, commandId?: string) => {
          if (!componentName) return;
          const component = module?.[componentName];
          if (!component) {
            console.warn(
              `[PluginRegistry] 插件 ${plugin.manifest.name} 声明了配置组件 ${componentName}` +
              `${commandId ? `（命令 ${commandId}）` : ''}，但入口 export default 里没有该导出：该节点不会出现参数表单`,
            );
            return;
          }
          // 插件导出的组件由插件任意提供，只能按 workflow_config 的约定断言成配置组件
          this.setWorkflowConfig(
            plugin.manifest.id,
            component as React.ComponentType<WorkflowConfigProps>,
            commandId,
          );
          console.log(
            '[PluginRegistry] 注册工作流配置组件: ' + plugin.manifest.id +
            (commandId ? ' -> ' + commandId + ' -> ' : ' -> ') + componentName,
          );
        };

        registerComponent(workflowConfig.component);
        for (const [commandId, componentName] of Object.entries(workflowConfig.components || {})) {
          registerComponent(componentName, commandId);
        }
      }

      // 3. 调用 onLoad 生命周期
      if (module?.onLoad) {
        await module.onLoad(api);
      }

      // 3b. 接线声明式钩子（contributes.hooks）：{ event, handler } 里的 handler 是入口
      //     export default 上的**函数名**，把它注册到应用事件总线（与 api.events.on 同一实现）。
      //     声明了但导出里没有同名函数时只 warn，不阻断加载（与 workflow_config 的处理一致）。
      const hooks = plugin.manifest.contributes?.hooks;
      if (hooks) {
        for (const hook of hooks) {
          const hookHandler = module?.[hook.handler];
          if (typeof hookHandler !== 'function') {
            console.warn(
              `[PluginRegistry] 插件 ${plugin.manifest.name} 声明了钩子 ${hook.event} -> ${hook.handler}，` +
              `但入口 export default 里没有该函数：该钩子不会生效`,
            );
            continue;
          }
          api.events.on(hook.event, hookHandler as (payload: unknown) => void);
          console.log(`[PluginRegistry] 注册声明式钩子: ${plugin.manifest.id} -> ${hook.event} (${hook.handler})`);
        }
      }

      // 4. 保存运行时实例
      this.runtimes.set(key, { api, module });

      this.loadStates.set(key, { pluginId: plugin.manifest.id, loaded: true });
      console.log('[PluginRegistry] 插件 ' + plugin.manifest.name + ' (' + plugin.path + ') 已加载');
    } catch (err) {
      const msg = String(err);
      this.loadStates.set(key, { pluginId: plugin.manifest.id, loaded: false, error: errorMessage(err) });
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
      workflowNodeTypeRegistry.unregisterPlugin(runtime.api.pluginId);
    }

    // 2b. 注销工作流配置组件
    if (runtime) {
      this.unsetWorkflowConfig(runtime.api.pluginId);
    }

    // 3. 清理 API 资源（自动注销命令/事件/全局订阅）
    runtime?.api.dispose();

    // 3. 注销面板组件
    this.unsetPluginPanelComponents(pluginPath);

    // 4. 清理状态
    this.runtimes.delete(pluginPath);
    this.loadStates.delete(pluginPath);
  }

  /**
   * 批量加载插件（**增量**：只卸掉已消失的、只加载还没加载的）。
   *
   * 两条不能违反的约束：
   * 1. **已加载的插件不能重跑 `onLoad`**：`onLoad` 里注册的事件/全局订阅会被叠加，
   *    而且面板组件是**新的函数引用**，React 会把面板整体重挂载 ——
   *    表现为"点一下刷新，面板里选中的命令被打回默认值"（以及插件面板内的其他本地状态被清空）。
   * 2. **已消失的插件要卸干净**：卸载会调 `onUnload` + `api.dispose()`，
   *    把命令/事件/面板组件回收；顺带把它的加载状态删掉，
   *    这样"卸载后重装到同一路径"能重新加载（路径没变也不会被当成"已加载"而跳过）。
   *
   * `loadPlugin` 自身对"已加载"会早退，所以第 2 步天然幂等。
   */
  async loadAllPlugins(plugins: PluginInstance[]): Promise<void> {
    const present = new Set(plugins.map((p) => p.path));
    for (const path of Array.from(this.loadStates.keys())) {
      if (!present.has(path)) {
        await this.unloadPlugin(path);
      }
    }
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
