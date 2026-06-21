/**
 * PluginAPI.agent — Agent 会话 API 实现
 *
 * 复用 PilotDesk 已有的 Agent 会话系统。
 * 插件拥有独立的会话上下文。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import { invoke } from '@tauri-apps/api/core';

/** 会话信息 */
export interface SessionInfo {
  session_id: string;
  agent_type: string;
  created_at: string;
}

/** Agent 信息 */
export interface AgentInfo {
  agent_type: string;
  name: string;
  version: string;
}

/** 消息 */
export interface Message {
  role: string;
  content: string;
  timestamp: string;
}

/** Agent 响应 */
export interface AgentResponse {
  content: string;
  session_id: string;
}

/** 会话选项 */
export interface SessionOptions {
  system_prompt?: string;
}

/** 流式回调 */
export interface StreamCallbacks {
  onChunk?: (chunk: string) => void;
  onDone?: (fullContent: string) => void;
  onError?: (error: string) => void;
}

/** Agent API */
export class PluginAgentAPI {
  private pluginId: string;

  constructor(pluginId: string) {
    this.pluginId = pluginId;
  }

  async createSession(agentType: string, options?: SessionOptions): Promise<SessionInfo> {
    return invoke<SessionInfo>('plugin_agent_create_session', {
      pluginId: this.pluginId,
      agentType,
      options: options || null,
    });
  }

  async sendMessage(sessionId: string, content: string): Promise<AgentResponse> {
    return invoke<AgentResponse>('plugin_agent_send_message', {
      pluginId: this.pluginId,
      sessionId,
      content,
    });
  }

  async getHistory(sessionId: string): Promise<Message[]> {
    return invoke<Message[]>('plugin_agent_get_history', {
      pluginId: this.pluginId,
      sessionId,
    });
  }

  async listSessions(): Promise<SessionInfo[]> {
    return invoke<SessionInfo[]>('plugin_agent_list_sessions', {
      pluginId: this.pluginId,
    });
  }

  async deleteSession(sessionId: string): Promise<void> {
    return invoke<void>('plugin_agent_delete_session', {
      pluginId: this.pluginId,
      sessionId,
    });
  }

  async listAgents(): Promise<AgentInfo[]> {
    return invoke<AgentInfo[]>('plugin_agent_list_agents', {
      pluginId: this.pluginId,
    });
  }
}
