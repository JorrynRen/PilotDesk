/**
 * PilotDesk 插件系统类型定义
 */

import type React from 'react';
import type { TemplateFieldProps } from '../components/workflow/TemplateField';
import type { TemplateVariableGroup } from '../components/workflow/templateVariables';
// 配置组件里拿到的 api 就是运行时实现类的实例；本文件的 `PluginAPI` 接口只是文档性契约，
// 故用别名引入实现类，避免同名覆盖。
import type { PluginAPI as PluginAPIRuntime } from '../plugin/PluginAPI';

/**
 * 通用 JSON 值。
 *
 * 插件 manifest、命令参数、事件载荷、宿主回传数据都属于「外部不可信数据」：
 * 类型上只能承诺是 JSON 可表达的取值，读取处需自行窄化（typeof / in / 类型守卫）。
 */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/** 插件清单 */
export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAppVersion: string;
  permissions: PluginPermission[];
  entry: {
    main: string;
  };
  icon?: string;
  contributes?: {
    panels?: PanelContribution[];
    commands?: CommandContribution[];
    hooks?: HookContribution[];
    /** 工作流节点类型（v2.0 新增） */
    node_types?: NodeTypeContribution[];
    /** 插件为工作流节点提供的配置组件 */
    workflow_config?: WorkflowConfigContribution;
  };
}

/** 权限声明 */
export type PluginPermission =
  | 'ui:panel'
  | 'ui:toast'
  | 'ui:modal'
  | 'session:read'
  | 'session:write'
  | 'data:invoke'
  | 'storage:*'
  | 'fs:read'
  | 'fs:write';

/** 插件为工作流节点提供的配置组件 */
export interface WorkflowConfigContribution {
  /** 插件入口 JS 中导出的组件名，作为该插件所有命令的默认参数表单 */
  component?: string;
  /** 命令级参数表单：键是 contributes.commands[].id，值是入口导出的组件名；优先于 component */
  components?: Record<string, string>;
}

/** 插件工作流节点配置组件的 props（由 PluginRegistry 透传给 PluginNodeConfig） */
export interface WorkflowConfigProps {
  params: Record<string, unknown>;
  onParamsChange: (key: string, value: unknown) => void;
  api?: PluginAPIRuntime;
  /** 节点当前选中的命令 id（未选时为空字符串） */
  commandId?: string;
  /** 节点当前选中命令的贡献点，含 input 参数模式 */
  command?: CommandContribution;
  /**
   * 该节点可引用的变量（输入映射的键名）。
   * 参数里的 `{{}}` 只能在"输入映射"的上下文里求值，所以这里给的就是这些键；
   * 为空表示当前没有可引用变量。
   */
  variables?: TemplateVariableGroup[];
  /**
   * 平台提供的模板输入组件：自带 `{{` 触发变量补全、自带"含变量引用不做类型校验"的语义。
   * 插件的参数输入框应当使用它，而不是自己写 `<input>`（自写的输入框没有补全）。
   */
  TemplateField?: React.ComponentType<TemplateFieldProps>;
}

/** 面板贡献点 */
export interface PanelContribution {
  id: string;
  title: string;
  icon?: string;
}

/** 命令贡献点 */
export interface CommandContribution {
  id: string;
  title: string;
  /**
   * 命令的输入参数模式：平台据此在插件未提供自定义组件时生成参数表单，
   * 因此这里是契约而不只是文档。属性名必须与 handler 读取的 params 键一致。
   */
  input?: {
    /** 可省略（省略即视为 object）；写了必须是 "object" */
    type?: 'object';
    properties?: Record<string, CommandInputProperty>;
    /** 必填属性名，必须是 properties 的子集 */
    required?: string[];
  };
  /**
   * 产出说明（**仅文档，界面不消费**）。
   * 命令既可以返回对象、也可以返回裸值（数字 / 字符串 / 布尔），所以这里不限定形状：
   * 用 `type` + `description` 说明产出是什么即可；对象型产出再用 `properties` 列字段。
   */
  output?: {
    type?: string;
    description?: string;
    properties?: Record<string, { type: string; description?: string }>;
  };
}

/** 命令输入属性的模式（JSON Schema 子集） */
export interface CommandInputProperty {
  type: string;
  description?: string;
  /** 参数未填写（或取值不在 enum 内）时使用的默认值；平台在切换命令、打开表单、执行前补齐到节点参数 */
  default?: JsonValue;
  /** 取值枚举，提供时表单渲染为下拉选择 */
  enum?: JsonValue[];
  /**
   * 枚举项的显示文案，**按下标与 `enum` 一一对应**（例：`enum: ["add","sub"]` +
   * `enumLabels: ["add（相加）","sub（相减）"]`）。
   *
   * 为什么单独放一个数组而不是把中文写进 `enum`：`enum` 的值会原样进工作流定义与命令调用，
   * 必须是稳定的机器值（handler 按它分支），中文只该出现在界面上。
   * 项数少于 `enum` 时按位回落为原始值，因此可以只给前几项起中文名。
   */
  enumLabels?: string[];
}

/** 事件钩子贡献点 */
export interface HookContribution {
  event: string;
  handler: string;
}

/** 工作流节点类型贡献点（v2.0 新增） */
export interface NodeTypeContribution {
  /** 节点类型唯一标识，如 "my-plugin.sentiment" */
  type_id: string;
  /** 节点显示名称 */
  name: string;
  /** 节点配置 JSON Schema */
  config_schema?: {
    type: 'object';
    properties?: Record<string, { type: string; description?: string; default?: JsonValue }>;
  };
  /** 执行此节点所需的权限 */
  permissions?: PluginPermission[];
}

/** 权限检查结果 */
export interface PermissionCheck {
  permission: string;
  allowed: boolean;
  reason: string | null;
}

/** 沙箱信息 */
export interface SandboxInfo {
  plugins_dir: string;
  sandbox_enabled: boolean;
  max_manifest_size: number;
  allowed_permissions: string[];
  high_risk_permissions: string[];
}

/** 插件实例（运行时状态） */
export interface PluginInstance {
  manifest: PluginManifest;
  enabled: boolean;
  loaded: boolean;
  path: string;
  error?: string;
  /** 权限检查结果 */
  permission_checks: PermissionCheck[];
  /** 是否有未授权的权限 */
  has_unauthorized_permissions: boolean;
}

/** 清单验证错误 */
export interface ManifestValidationError {
  field: string;
  message: string;
}

/** 命令 handler：params 由平台按命令 input 模式整理后传入（插件侧数据视作不可信），返回值按 JSON 契约回传 */
export type CommandHandler = (params: Record<string, unknown>) => Promise<JsonValue>;

/** 事件 handler：payload 为宿主事件载荷（不可信）；返回 false 可中断传播 */
export type EventHandler = (payload: unknown) => Promise<unknown>;

/** 命令执行结果 */
export interface CommandResult {
  success: boolean;
  data?: JsonValue;
  error?: string;
  duration?: number;
}

/** 插件前端 API */
export interface PluginAPI {
  ui: {
    addPanel(config: PanelContribution & { component: React.ComponentType }): void;
    removePanel(id: string): void;
    showToast(message: string, type: 'info' | 'success' | 'error'): void;
  };
  data: {
    invoke<T>(cmd: string, params?: Record<string, unknown>): Promise<T>;
  };
  events: {
    on(event: string, handler: (...args: unknown[]) => void): () => void;
    emit(event: string, ...args: unknown[]): void;
  };
  storage: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
  /** v2.0 新增：命令注册与执行 */
  commands: {
    register(commandId: string, handler: CommandHandler): void;
    execute<T = unknown>(commandId: string, params?: Record<string, unknown>): Promise<T>;
  };
  /** v2.0 新增：事件钩子注册 */
  hooks: {
    on(event: string, handler: EventHandler): () => void;
  };
  /** v2.0 新增：跨插件通信 */
  global: {
    on(event: string, handler: (payload: unknown) => void): () => void;
    emit(event: string, payload?: unknown): void;
    call(pluginId: string, commandId: string, params?: Record<string, unknown>): Promise<CommandResult>;
  };
  /** v2.0 新增：文件系统操作（沙箱禁用时可用） */
  fs: {
    readText(path: string): Promise<string>;
    writeText(path: string, content: string): Promise<void>;
    delete(path: string): Promise<void>;
    exists(path: string): Promise<boolean>;
    readDir(path: string): Promise<{ name: string; path: string; is_dir: boolean; size: number }[]>;
  };
  /** v2.0 新增：Shell 命令执行（沙箱禁用 + 二次确认） */
  shell: {
    exec(command: string, options?: { timeout_ms?: number; working_dir?: string }): Promise<{ stdout: string; stderr: string; exit_code: number }>;
  };
  /** v2.0 新增：Agent 会话管理 */
  agent: {
    createSession(agentType: string, options?: { system_prompt?: string }): Promise<{ session_id: string; agent_type: string; created_at: string }>;
    sendMessage(sessionId: string, content: string): Promise<{ content: string; session_id: string }>;
    getHistory(sessionId: string): Promise<{ role: string; content: string; timestamp: string }[]>;
    listSessions(): Promise<{ session_id: string; agent_type: string; created_at: string }[]>;
    deleteSession(sessionId: string): Promise<void>;
    listAgents(): Promise<{ agent_type: string; name: string; version: string }[]>;
  };
}

/** 插件入口模块 */
export interface PluginEntry {
  onLoad(api: PluginAPI): void | Promise<void>;
  onUnload(): void | Promise<void>;
}
