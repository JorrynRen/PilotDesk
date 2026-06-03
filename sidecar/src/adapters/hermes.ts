import { spawn, ChildProcess } from 'child_process';
import { AgentAdapter, buildPrompt } from './base';
import type { ChatRequest, SkillInfo } from '../types';

// Strip ANSI escape sequences from text
function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\x1b\[[0-9;]*m/g, '').replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '');
}

// Filter out decorative/UI lines from Hermes output
function isContentLine(line: string): boolean {
  const trimmed = line.trim();
  // Filter empty lines, box-drawing chars, session info, resume info
  if (!trimmed) return false;
  if (/^[\─┄┈│┌┐└┘├┤┬┴┼━┃╋╭╮╰╯─│┄┈]+$/.test(trimmed)) return false;
  if (trimmed === 'Initializing agent...') return false;
  if (trimmed.startsWith('Resume this session')) return false;
  if (trimmed.startsWith('hermes --resume')) return false;
  if (trimmed.startsWith('Session:')) return false;
  if (trimmed.startsWith('Duration:')) return false;
  if (trimmed.startsWith('Messages:')) return false;
  if (trimmed.startsWith('Query:')) return false;
  return true;
}

export class HermesAdapter implements AgentAdapter {
  agentType = 'hermes' as const;
  private processes = new Map<string, ChildProcess>();
  private aborted = new Set<string>();

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

    // Build the command string for shell execution
    // Use -q for single query + -Q for quiet mode (suppress banner/spinner/tool previews)
    // Escape the message for shell: wrap in double quotes, escape internal double quotes
    const escapedMsg = fullMessage.replace(/"/g, '\\"');
    const isWin = process.platform === 'win32';
    const cmd = isWin
      ? `hermes.bat chat -q "${escapedMsg}" -Q`
      : `hermes chat -q "${escapedMsg}" -Q`;

    // Set PYTHONHOME='' on Windows to bypass WPS .pth conflicts with hermes_cli
    const hermesEnv = isWin
      ? { ...process.env, PYTHONHOME: '' }
      : { ...process.env };

    const child = spawn(cmd, [], {
      cwd: cwd || process.cwd(),
      stdio: ['pipe', 'pipe', 'pipe'],
      shell: true,
      env: hermesEnv,
    });

    this.processes.set(sessionId, child);

    // No stdin needed - message passed via -q flag
    child.stdin?.end();

    // Stream stdout chunks
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
        console.error(`[Hermes stderr] ${text}`);
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

      // Process stream line by line
      let buffer = '';
      while (!finished || queue.length > 0) {
        if (queue.length > 0) {
          buffer += queue.shift()!;
          while (buffer.includes('\n')) {
            const idx = buffer.indexOf('\n');
            const line = buffer.slice(0, idx);
            buffer = buffer.slice(idx + 1);
            const clean = stripAnsi(line);
            if (isContentLine(clean)) {
              yield clean + '\n';
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
        const clean = stripAnsi(buffer.trim());
        if (isContentLine(clean)) {
          yield clean + '\n';
        }
      }

      const exitCode = await closePromise;
      if (exitCode !== 0 && !this.aborted.has(sessionId)) {
        const stderrText = stderrChunks.join('').trim();
        let detail = stderrText ? stderrText.slice(0, 300) : '';
        // Try to extract meaningful error from stderr
        let friendlyMsg = `Hermes 进程异常退出 (exit code ${exitCode})`;
        if (detail) {
          // Check for common API errors in stderr
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
