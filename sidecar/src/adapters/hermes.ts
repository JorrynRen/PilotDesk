import { spawn, ChildProcess } from 'child_process';
import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest } from '../types';

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
}
