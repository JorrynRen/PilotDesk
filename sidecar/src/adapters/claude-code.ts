import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest, SkillInfo } from '../types';

export class ClaudeCodeAdapter implements AgentAdapter {
  agentType = 'claude' as const;
  private abortControllers = new Map<string, AbortController>();

  async createSession(sessionId: string, cwd?: string): Promise<void> {
    console.log(`[Claude] Session created: ${sessionId}, cwd: ${cwd || 'default'}`);
  }

  async closeSession(sessionId: string): Promise<void> {
    const controller = this.abortControllers.get(sessionId);
    if (controller) {
      controller.abort();
      this.abortControllers.delete(sessionId);
    }
    console.log(`[Claude] Session closed: ${sessionId}`);
  }

  async *sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown> {
    const { sessionId, message, mode, cwd } = request;
    const fullMessage = buildPrompt(message, mode);
    const controller = new AbortController();
    this.abortControllers.set(sessionId, controller);

    try {
      const sdk = await import('@anthropic-ai/claude-code/sdk');

      const response = await sdk.query({
        prompt: fullMessage,
        options: {
          cwd: cwd || process.cwd(),
          abortController: controller,
        },
      });

      // Process response stream
      for await (const event of response) {
        if (event.type === 'assistant') {
          // SDKAssistantMessage has a .message property (APIAssistantMessage/BetaMessage)
          const apiMsg = (event as any).message;
          if (apiMsg?.content && Array.isArray(apiMsg.content)) {
            for (const block of apiMsg.content) {
              if (block.type === 'text' && block.text) {
                yield block.text;
              }
            }
          }
        }
      }
    } catch (error: any) {
      if (error.name !== 'AbortError') {
        console.error(`[Claude] Error in session ${sessionId}:`, error.message);
        throw error;
      }
    } finally {
      this.abortControllers.delete(sessionId);
    }
  }

  stopGeneration(sessionId: string): void {
    const controller = this.abortControllers.get(sessionId);
    if (controller) {
      controller.abort();
      this.abortControllers.delete(sessionId);
    }
    console.log(`[Claude] Generation stopped: ${sessionId}`);
  }

  async listSkills(): Promise<SkillInfo[]> {
    // Claude Code skills come from:
    // 1. Built-in slash commands
    // 2. MCP servers configured in ~/.claude/claude_desktop_config.json
    const skills: SkillInfo[] = [
      { name: 'code-review', description: '代码审查与优化建议', category: '内置' },
      { name: 'translate', description: '多语言翻译', category: '内置' },
      { name: 'summarize', description: '文本摘要与总结', category: '内置' },
      { name: 'debug', description: '代码调试与错误诊断', category: '内置' },
      { name: 'refactor', description: '代码重构', category: '内置' },
      { name: 'test-gen', description: '单元测试生成', category: '内置' },
      { name: 'doc-gen', description: '文档生成', category: '内置' },
      { name: 'explain', description: '代码解释与分析', category: '内置' },
      { name: 'architect', description: '系统架构设计与评审', category: '内置' },
    ];

    // Try to read MCP server configs
    try {
      const fs = await import('fs/promises');
      const path = await import('path');
      const os = await import('os');
      const configPath = path.join(os.homedir(), '.claude', 'claude_desktop_config.json');
      const raw = await fs.readFile(configPath, 'utf-8');
      const config = JSON.parse(raw);
      const mcpServers = config.mcpServers || {};
      for (const [name, server] of Object.entries(mcpServers)) {
        const s = server as any;
        skills.push({
          name: `mcp:${name}`,
          description: s.command ? `MCP Server: ${name}` : `MCP Server: ${name}`,
          category: 'MCP',
        });
      }
    } catch {
      // MCP config not found, skip
    }

    return skills;
  }
}
