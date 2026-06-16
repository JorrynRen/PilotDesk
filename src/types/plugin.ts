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
    styles?: string;
  };
  contributes?: {
    panels?: PanelContribution[];
    commands?: CommandContribution[];
    hooks?: HookContribution[];
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
}

/** 事件钩子贡献点 */
export interface HookContribution {
  event: string;
  handler: string;
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
}

/** 插件入口模块 */
export interface PluginEntry {
  onLoad(api: PluginAPI): void | Promise<void>;
  onUnload(): void | Promise<void>;
}
