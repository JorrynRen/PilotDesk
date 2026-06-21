/**
 * CommandDispatcher — 插件命令调度器
 *
 * 管理插件注册的命令 handler，支持参数校验、超时控制和返回值处理。
 * 工作流引擎和跨插件调用通过此模块执行命令。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import type { CommandHandler, CommandResult } from '../types/plugin';

// ── 类型定义 ──

/** 已注册的命令信息 */
interface RegisteredCommandHandler {
  pluginId: string;
  commandId: string;
  handler: CommandHandler;
  /** 注册时间戳，用于调试 */
  registeredAt: number;
}

/** 命令执行选项 */
export interface CommandExecuteOptions {
  /** 超时时间（毫秒），默认 30000 */
  timeout?: number;
}

// ── 调度器 ──

class CommandDispatcher {
  /** 命令 handler 映射 key: pluginId:commandId */
  private handlers: Map<string, RegisteredCommandHandler> = new Map();

  // ── 注册管理 ──

  /**
   * 注册命令 handler
   */
  register(pluginId: string, commandId: string, handler: CommandHandler): void {
    const key = `${pluginId}:${commandId}`;
    if (this.handlers.has(key)) {
      console.warn(`[CommandDispatcher] 命令 ${key} 已被覆盖注册`);
    }
    this.handlers.set(key, {
      pluginId,
      commandId,
      handler,
      registeredAt: Date.now(),
    });
    console.log(`[CommandDispatcher] 命令已注册: ${key}`);
  }

  /**
   * 注销命令 handler
   */
  unregister(pluginId: string, commandId: string): void {
    const key = `${pluginId}:${commandId}`;
    this.handlers.delete(key);
  }

  /**
   * 注销指定插件的所有命令
   */
  unregisterAll(pluginId: string): void {
    for (const key of this.handlers.keys()) {
      if (key.startsWith(`${pluginId}:`)) {
        this.handlers.delete(key);
      }
    }
  }

  /**
   * 检查命令是否已注册
   */
  has(pluginId: string, commandId: string): boolean {
    return this.handlers.has(`${pluginId}:${commandId}`);
  }

  /**
   * 获取已注册的命令列表
   */
  list(): { pluginId: string; commandId: string }[] {
    return Array.from(this.handlers.values()).map((h) => ({
      pluginId: h.pluginId,
      commandId: h.commandId,
    }));
  }

  // ── 命令执行 ──

  /**
   * 执行命令
   */
  async execute<T = any>(
    pluginId: string,
    commandId: string,
    params?: any,
    options?: CommandExecuteOptions,
  ): Promise<CommandResult> {
    const key = `${pluginId}:${commandId}`;
    const registered = this.handlers.get(key);

    if (!registered) {
      return {
        success: false,
        error: `命令未注册: ${key}`,
      };
    }

    const timeout = options?.timeout ?? 30000;
    const startTime = Date.now();

    try {
      const result = await this.executeWithTimeout(registered.handler, params, timeout);
      return {
        success: true,
        data: result,
        duration: Date.now() - startTime,
      };
    } catch (err: any) {
      return {
        success: false,
        error: err.message || String(err),
        duration: Date.now() - startTime,
      };
    }
  }

  /**
   * 通过完整命令 ID 执行（格式: pluginId:commandId）
   */
  async executeCommand<T = any>(
    commandId: string,
    params?: any,
    options?: CommandExecuteOptions,
  ): Promise<CommandResult> {
    const colonIndex = commandId.indexOf(':');
    if (colonIndex === -1) {
      return {
        success: false,
        error: `命令 ID 格式无效，需要 pluginId:commandId 格式: ${commandId}`,
      };
    }

    const pluginId = commandId.substring(0, colonIndex);
    const cmdId = commandId.substring(colonIndex + 1);
    return this.execute(pluginId, cmdId, params, options);
  }

  // ── 内部方法 ──

  private async executeWithTimeout(
    handler: CommandHandler,
    params: any,
    timeoutMs: number,
  ): Promise<any> {
    if (timeoutMs <= 0) {
      return handler(params);
    }

    return new Promise<any>((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(`命令执行超时 (${timeoutMs}ms)`));
      }, timeoutMs);

      handler(params)
        .then((result) => {
          clearTimeout(timer);
          resolve(result);
        })
        .catch((err) => {
          clearTimeout(timer);
          reject(err);
        });
    });
  }
}

/** 全局单例 */
export const commandDispatcher = new CommandDispatcher();
