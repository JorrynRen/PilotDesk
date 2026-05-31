import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest } from '../types';

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
}
