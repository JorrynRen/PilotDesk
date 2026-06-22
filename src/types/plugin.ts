/**
 * PilotDesk 插件系统类型定义
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v1.0.md
 */

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
  input?: {
    type: 'object';
    properties?: Record<string, { type: string; description?: string }>;
  };
  output?: {
    type: 'object';
    properties?: Record<string, { type: string; description?: string }>;
  };
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
    properties?: Record<string, { type: string; description?: string; default?: any }>;
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

/** 命令 handler */
export type CommandHandler = (params: any) => Promise<any>;

/** 事件 handler */
export type EventHandler = (payload: any) => Promise<any | void>;

/** 命令执行结果 */
export interface CommandResult {
  success: boolean;
  data?: any;
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
    execute<T = any>(commandId: string, params?: any): Promise<T>;
  };
  /** v2.0 新增：事件钩子注册 */
  hooks: {
    on(event: string, handler: EventHandler): () => void;
  };
  /** v2.0 新增：跨插件通信 */
  global: {
    on(event: string, handler: (payload: any) => void): () => void;
    emit(event: string, payload?: any): void;
    call<T = any>(pluginId: string, commandId: string, params?: any): Promise<CommandResult>;
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
