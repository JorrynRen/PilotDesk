import { spawn, ChildProcess } from 'child_process';
import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest, SkillInfo } from '../types';

export class ClaudeCodeAdapter implements AgentAdapter {
  agentType = 'claude' as const;
  private processes = new Map<string, ChildProcess>();
  private aborted = new Set<string>();

  async createSession(sessionId: string, cwd?: string): Promise<void> {
    console.log(`[Claude] Session created: ${sessionId}, cwd: ${cwd || 'default'}`);
  }

  async closeSession(sessionId: string): Promise<void> {
    this.stopGeneration(sessionId);
    console.log(`[Claude] Session closed: ${sessionId}`);
  }

  async *sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown> {
    const { sessionId, message, mode, cwd } = request;
    const fullMessage = buildPrompt(message, mode);
    this.aborted.delete(sessionId);

    // Use Claude Code CLI with stream-json output format
    // -p: print mode (non-interactive), --verbose required for stream-json
    const escapedMsg = fullMessage.replace(/"/g, '\\"');
    const cmd = `claude -p --verbose --output-format stream-json --dangerously-skip-permissions "${escapedMsg}"`;

    const child = spawn(cmd, [], {
      cwd: cwd || process.cwd(),
      stdio: ['pipe', 'pipe', 'pipe'],
      shell: true,
    });

    this.processes.set(sessionId, child);
    child.stdin?.end();

    try {
      const queue: string[] = [];
      let resolveWait: (() => void) | null = null;
      let finished = false;

      child.stdout?.on('data', (raw: Buffer) => {
        if (this.aborted.has(sessionId)) return;
        const text = raw.toString();
        queue.push(text);
        if (resolveWait) {
          resolveWait();
          resolveWait = null;
        }
      });

      const stderrChunks: string[] = [];
      child.stderr?.on('data', (chunk: Buffer) => {
        const text = chunk.toString();
        // Claude Code outputs startup warnings to stderr, log only if substantive
        if (text.trim() && !text.includes('Claude Code')) {
          console.error(`[Claude stderr] ${text}`);
        }
        stderrChunks.push(text);
      });

      const closePromise = new Promise<number>((resolve) => {
        child.on('close', (code) => {
          finished = true;
          if (resolveWait) {
            resolveWait();
            resolveWait = null;
          }
          this.processes.delete(sessionId);
          resolve(code ?? 0);
        });
        child.on('error', () => {
          finished = true;
          if (resolveWait) {
            resolveWait();
            resolveWait = null;
          }
          this.processes.delete(sessionId);
          resolve(1);
        });
      });

      // Process stream-json events line by line
      let buffer = '';
      while (!finished || queue.length > 0) {
        if (queue.length > 0) {
          buffer += queue.shift()!;
          while (buffer.includes('\n')) {
            const idx = buffer.indexOf('\n');
            const line = buffer.slice(0, idx).trim();
            buffer = buffer.slice(idx + 1);

            if (!line) continue;

            try {
              const event = JSON.parse(line);

              if (event.type === 'assistant') {
                const content = event.message?.content;
                if (Array.isArray(content)) {
                  for (const block of content) {
                    if (block.type === 'text' && block.text) {
                      yield block.text + '\n';
                    }
                  }
                }
              }
              // Ignore other event types (system, init, result, etc.)
            } catch {
              // Not JSON, skip
            }
          }
        } else if (!finished) {
          await new Promise<void>((resolve) => {
            resolveWait = resolve;
          });
        }
      }

      // Process remaining buffer
      if (buffer.trim()) {
        try {
          const event = JSON.parse(buffer.trim());
          if (event.type === 'assistant') {
            const content = event.message?.content;
            if (Array.isArray(content)) {
              for (const block of content) {
                if (block.type === 'text' && block.text) {
                  yield block.text + '\n';
                }
              }
            }
          }
        } catch {
          // Not JSON, skip
        }
      }

      const exitCode = await closePromise;
      if (exitCode !== 0 && !this.aborted.has(sessionId)) {
        const stderrText = stderrChunks.join('').trim();
        let detail = stderrText ? stderrText.slice(0, 300) : '';
        let friendlyMsg = `Claude Code 进程异常退出 (exit code ${exitCode})`;
        if (detail) {
          const d = detail.toLowerCase();
          if (d.includes('insufficient') && (d.includes('balance') || d.includes('quota'))) {
            friendlyMsg = '请求失败：API 账户余额不足。请前往 API 提供商后台充值后重试。';
          } else if (d.includes('403')) {
            friendlyMsg = `请求被拒 (HTTP 403)：${detail.slice(0, 200)}。请检查 API Key 权限、账户余额或模型可用性。`;
          } else if (d.includes('401')) {
            friendlyMsg = '认证失败 (HTTP 401)：API Key 无效或已过期。请检查 API Key 是否正确。';
          } else if (d.includes('model') && (d.includes('not found') || d.includes('not support'))) {
            friendlyMsg = `模型不可用：${detail.slice(0, 200)}。请检查模型名称是否正确，或更换模型后重试。`;
          } else {
            friendlyMsg += `：${detail}`;
          }
        }
        throw new Error(friendlyMsg);
      }
    } catch (error: any) {
      if (!this.aborted.has(sessionId)) {
        throw error;
      }
    }
  }

  stopGeneration(sessionId: string): void {
    this.aborted.add(sessionId);
    const proc = this.processes.get(sessionId);
    if (proc && !proc.killed) {
      proc.kill('SIGTERM');
    }
    this.processes.delete(sessionId);
    console.log(`[Claude] Generation stopped: ${sessionId}`);
  }

  async listSkills(): Promise<SkillInfo[]> {
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
