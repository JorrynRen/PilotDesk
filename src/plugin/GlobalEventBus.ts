/**
 * GlobalEventBus — 全局事件总线
 *
 * 支持跨插件发布/订阅和直接命令调用。
 * 事件命名空间约定：plugin:{pluginId}:{eventName}
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import { commandDispatcher } from './CommandDispatcher';
import type { CommandResult } from '../types/plugin';

// ── 类型定义 ──

/** 全局事件 handler */
type GlobalEventHandler = (payload: any) => void;

/** 已订阅的全局事件信息 */
interface GlobalEventSubscription {
  pluginId: string;
  event: string;
  handler: GlobalEventHandler;
}

// ── 全局事件总线 ──

class GlobalEventBus {
  /** 订阅映射 key: event */
  private subscriptions: Map<string, GlobalEventSubscription[]> = new Map();

  // ── 订阅管理 ──

  /**
   * 订阅全局事件
   * @param pluginId 插件 ID
   * @param event 事件名称
   * @param handler 事件处理函数
   * @returns 取消订阅的函数
   */
  on(pluginId: string, event: string, handler: GlobalEventHandler): () => void {
    const list = this.subscriptions.get(event) || [];
    list.push({ pluginId, event, handler });
    this.subscriptions.set(event, list);

    return () => {
      this.off(pluginId, event);
    };
  }

  /**
   * 取消订阅
   */
  off(pluginId: string, event: string): void {
    const list = this.subscriptions.get(event);
    if (!list) return;

    const filtered = list.filter((s) => s.pluginId !== pluginId);
    if (filtered.length === 0) {
      this.subscriptions.delete(event);
    } else {
      this.subscriptions.set(event, filtered);
    }
  }

  /**
   * 取消指定插件的所有订阅
   */
  offAll(pluginId: string): void {
    for (const [event, list] of this.subscriptions.entries()) {
      const filtered = list.filter((s) => s.pluginId !== pluginId);
      if (filtered.length === 0) {
        this.subscriptions.delete(event);
      } else {
        this.subscriptions.set(event, filtered);
      }
    }
  }

  // ── 事件发布 ──

  /**
   * 发布全局事件
   * 并行调用所有订阅者的 handler
   */
  emit(event: string, payload?: any): void {
    const list = this.subscriptions.get(event);
    if (!list || list.length === 0) return;

    for (const sub of list) {
      try {
        sub.handler(payload);
      } catch (err) {
        console.warn(
          `[GlobalEventBus] Handler error: ${sub.pluginId} -> ${event}:`,
          err,
        );
      }
    }
  }

  // ── 跨插件命令调用 ──

  /**
   * 直接调用其他插件的命令
   * @param pluginId 目标插件 ID
   * @param commandId 命令 ID
   * @param params 命令参数
   */
  async call<T = any>(
    pluginId: string,
    commandId: string,
    params?: any,
  ): Promise<CommandResult> {
    return commandDispatcher.execute<T>(pluginId, commandId, params);
  }
}

/** 全局单例 */
export const globalEventBus = new GlobalEventBus();
