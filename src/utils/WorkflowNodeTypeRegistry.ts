/**
 * WorkflowNodeTypeRegistry — 前端节点类型注册表
 *
 * 统一管理内置节点类型和插件注册的节点类型。
 * 内置节点在应用启动时注册，插件节点在加载时通过 manifest.node_types 注册。
 *
 * 设计文档: docs/PilotDesk-工作流管理系统设计-v1.0.md
 */

import type { ComponentType } from 'react';
import type { NodeProps } from '@xyflow/react';
import type { WorkflowNodeType, WorkflowNode } from '../types/workflow';
import type { NodeTypeContribution } from '../types/plugin';

/** 注册的节点类型信息 */
export interface RegisteredNodeType {
  typeId: string;
  name: string;
  category: 'builtin' | 'plugin';
  pluginId?: string;
  component: ComponentType<NodeProps>;
  configSchema?: Record<string, { type: string; description?: string; default?: any }>;
  permissions?: string[];
}

/** 节点类型注册表 */
class WorkflowNodeTypeRegistry {
  private entries: Map<string, RegisteredNodeType> = new Map();

  /** 注册节点类型 */
  register(entry: RegisteredNodeType): void {
    this.entries.set(entry.typeId, entry);
  }

  /** 注销节点类型（插件卸载时调用） */
  unregister(typeId: string): void {
    this.entries.delete(typeId);
  }

  /** 获取所有注册的节点类型 */
  getAll(): RegisteredNodeType[] {
    return Array.from(this.entries.values());
  }

  /** 获取内置节点类型 */
  getBuiltin(): RegisteredNodeType[] {
    return this.getAll().filter((e) => e.category === 'builtin');
  }

  /** 获取插件节点类型 */
  getPlugin(): RegisteredNodeType[] {
    return this.getAll().filter((e) => e.category === 'plugin');
  }

  /** 获取指定类型的注册信息 */
  get(typeId: string): RegisteredNodeType | undefined {
    return this.entries.get(typeId);
  }

  /** 获取 react-flow nodeTypes 映射 */
  getNodeComponents(): Record<string, ComponentType<NodeProps>> {
    const components: Record<string, ComponentType<NodeProps>> = {};
    for (const [typeId, entry] of this.entries) {
      components[typeId] = entry.component;
    }
    return components;
  }

  /** 从插件贡献点注册节点类型 */
  registerFromPlugin(
    pluginId: string,
    nodeTypes: NodeTypeContribution[],
    componentFactory: (typeId: string) => ComponentType<NodeProps>,
  ): void {
    for (const nt of nodeTypes) {
      this.register({
        typeId: nt.type_id,
        name: nt.name,
        category: 'plugin',
        pluginId,
        component: componentFactory(nt.type_id),
        configSchema: nt.config_schema?.properties,
        permissions: nt.permissions,
      });
    }
  }

  /** 注销插件所有节点类型 */
  unregisterPlugin(pluginId: string): void {
    for (const [typeId, entry] of this.entries) {
      if (entry.pluginId === pluginId) {
        this.entries.delete(typeId);
      }
    }
  }
}

/** 全局单例 */
export const workflowNodeTypeRegistry = new WorkflowNodeTypeRegistry();
