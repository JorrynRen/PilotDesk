import { spawn, ChildProcess } from 'child_process';
import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest, SkillInfo } from '../types';

export class HermesAdapter implements AgentAdapter {
  agentType = 'hermes' as const;
  private processes = new Map<string, ChildProcess>();
  private aborted = new Set<string>();
  private pendingResolve: (() => void) | null = null;

  async createSession(sessionId: string, cwd?: string): Promise<void> {
    console.log(`[Hermes] Session created: ${sessionId}, cwd: ${cwd || 'default'}`);
  }

  async closeSession(sessionId: string): Promise<void> {
    this.stopGeneration(sessionId);
    console.log(`[Hermes] Session closed: ${sessionId}`);
  }

  async *sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown> {
    const { sessionId, message, mode, cwd } = request;
    const fullMessage = buildPrompt(message, mode);
    this.aborted.delete(sessionId);

    const child = spawn('hermes', ['chat', '--stream'], {
      cwd: cwd || process.cwd(),
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    this.processes.set(sessionId, child);

    let buffer = '';
    let done = false;

    // Send message via stdin
    child.stdin?.write(fullMessage + '\n');
    child.stdin?.end();

    const queue: string[] = [];
    let resolveWait: (() => void) | null = null;

    child.stdout?.on('data', (chunk: Buffer) => {
      if (this.aborted.has(sessionId)) return;
      buffer += chunk.toString();
      while (buffer.includes('\n')) {
        const idx = buffer.indexOf('\n');
        const line = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 1);
        if (line.trim()) {
          queue.push(line + '\n');
          if (resolveWait) {
            resolveWait();
            resolveWait = null;
          }
        }
      }
    });

    child.stderr?.on('data', (chunk: Buffer) => {
      console.error(`[Hermes stderr] ${chunk.toString()}`);
    });

    const closePromise = new Promise<void>((resolve, reject) => {
      child.on('close', (code) => {
        done = true;
        if (buffer.trim() && !this.aborted.has(sessionId)) {
          queue.push(buffer);
          if (resolveWait) {
            resolveWait();
            resolveWait = null;
          }
        }
        if (code !== 0 && !this.aborted.has(sessionId)) {
          reject(new Error(`Hermes process exited with code ${code}`));
        } else {
          resolve();
        }
        this.processes.delete(sessionId);
      });

      child.on('error', (err) => {
        done = true;
        reject(err);
        this.processes.delete(sessionId);
      });
    });

    try {
      // Yield queued chunks as they arrive
      while (!done || queue.length > 0) {
        if (queue.length > 0) {
          yield queue.shift()!;
        } else if (!done) {
          await new Promise<void>((resolve) => {
            resolveWait = resolve;
          });
        }
      }
      await closePromise;
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
    console.log(`[Hermes] Generation stopped: ${sessionId}`);
  }

  async listSkills(): Promise<SkillInfo[]> {
    // Hermes skills are stored in ~/.hermes/skills/
    const skills: SkillInfo[] = [];
    try {
      const fs = await import('fs/promises');
      const path = await import('path');
      const os = await import('os');
      const skillsDir = path.join(os.homedir(), '.hermes', 'skills');
      const entries = await fs.readdir(skillsDir, { withFileTypes: true });
      for (const entry of entries) {
        if (entry.isDirectory()) {
          // Read skill manifest if exists
          try {
            const manifestPath = path.join(skillsDir, entry.name, 'manifest.json');
            const raw = await fs.readFile(manifestPath, 'utf-8');
            const manifest = JSON.parse(raw);
            skills.push({
              name: entry.name,
              description: manifest.description || entry.name,
              category: manifest.category || '自定义',
            });
          } catch {
            skills.push({
              name: entry.name,
              description: entry.name,
              category: '自定义',
            });
          }
        }
      }
    } catch {
      // Skills dir not found, return empty
    }

    // Fallback built-in skills if no skills found
    if (skills.length === 0) {
      skills.push(
        { name: 'code-review', description: '代码审查与优化建议', category: '内置' },
        { name: 'translate', description: '多语言翻译', category: '内置' },
        { name: 'summarize', description: '文本摘要与总结', category: '内置' },
        { name: 'debug', description: '代码调试与错误诊断', category: '内置' },
        { name: 'refactor', description: '代码重构', category: '内置' },
        { name: 'test-gen', description: '单元测试生成', category: '内置' },
        { name: 'doc-gen', description: '文档生成', category: '内置' },
      );
    }

    return skills;
  }
}
