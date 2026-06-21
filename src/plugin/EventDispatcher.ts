/**
 * EventDispatcher — 插件事件分发器
 *
 * 核心应用事件分发到插件 hook handler。
 * 串行执行，支持中断传播和 payload 修改。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import type { EventHandler } from '../types/plugin';

// ── 类型定义 ──

/** 已注册的事件 handler 信息 */
interface RegisteredEventHandler {
  pluginId: string;
  event: string;
  handler: EventHandler;
  /** 注册顺序，用于调试 */
  order: number;
}

/** 事件分发选项 */
export interface EventEmitOptions {
  /** 是否在 handler 返回 false 时中断传播（默认 true） */
  breakOnFalse?: boolean;
}

/** 事件分发结果 */
export interface EventEmitResult {
  event: string;
  handled: number;
  interrupted: boolean;
  results: any[];
}

// ── 事件分发器 ──

class EventDispatcher {
  /** 事件 handler 映射 key: event */
  private handlers: Map<string, RegisteredEventHandler[]> = new Map();
  private registerOrder: number = 0;

  // ── 注册管理 ──

  /**
   * 注册事件 handler
   * @param pluginId 插件 ID
   * @param event 事件名称
   * @param handler 事件处理函数
   * @returns 取消注册的函数
   */
  register(pluginId: string, event: string, handler: EventHandler): () => void {
    const list = this.handlers.get(event) || [];
    const entry: RegisteredEventHandler = {
      pluginId,
      event,
      handler,
      order: this.registerOrder++,
    };
    list.push(entry);
    this.handlers.set(event, list);

    console.log(`[EventDispatcher] 事件 handler 已注册: ${pluginId} -> ${event}`);

    // 返回取消注册函数
    return () => {
      this.unregister(pluginId, event);
    };
  }

  /**
   * 注销事件 handler
   */
  unregister(pluginId: string, event: string): void {
    const list = this.handlers.get(event);
    if (!list) return;

    const filtered = list.filter((h) => h.pluginId !== pluginId);
    if (filtered.length === 0) {
      this.handlers.delete(event);
    } else {
      this.handlers.set(event, filtered);
    }
  }

  /**
   * 注销指定插件的所有事件 handler
   */
  unregisterAll(pluginId: string): void {
    for (const [event, list] of this.handlers.entries()) {
      const filtered = list.filter((h) => h.pluginId !== pluginId);
      if (filtered.length === 0) {
        this.handlers.delete(event);
      } else {
        this.handlers.set(event, filtered);
      }
    }
  }

  /**
   * 检查事件是否有 handler
   */
  has(event: string): boolean {
    const list = this.handlers.get(event);
    return list !== undefined && list.length > 0;
  }

  /**
   * 获取已注册的事件列表
   */
  list(): { pluginId: string; event: string }[] {
    const result: { pluginId: string; event: string }[] = [];
    for (const [, list] of this.handlers.entries()) {
      for (const h of list) {
        result.push({ pluginId: h.pluginId, event: h.event });
      }
    }
    return result;
  }

  // ── 事件分发 ──

  /**
   * 分发事件到所有注册的 handler
   * 串行执行，handler 返回 false 可中断传播
   */
  async emit(
    event: string,
    payload?: any,
    options?: EventEmitOptions,
  ): Promise<EventEmitResult> {
    const list = this.handlers.get(event);
    if (!list || list.length === 0) {
      return { event, handled: 0, interrupted: false, results: [] };
    }

    const breakOnFalse = options?.breakOnFalse ?? true;
    const results: any[] = [];
    let interrupted = false;

    for (const entry of list) {
      try {
        const result = await entry.handler(payload);
        results.push(result);

        // handler 返回 false 时中断传播
        if (breakOnFalse && result === false) {
          interrupted = true;
          break;
        }
      } catch (err) {
        console.warn(
          `[EventDispatcher] Handler error: ${entry.pluginId} -> ${event}:`,
          err,
        );
        results.push(null);
      }
    }

    return {
      event,
      handled: list.length,
      interrupted,
      results,
    };
  }
}

/** 全局单例 */
export const eventDispatcher = new EventDispatcher();
