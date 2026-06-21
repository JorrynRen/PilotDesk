/**
 * PluginAPI.shell — Shell 命令 API 实现
 *
 * 通过 Tauri invoke 调用 Rust 侧 shell 命令。
 * 沙箱启用时所有操作被拒绝。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import { invoke } from '@tauri-apps/api/core';

/** Shell 执行结果 */
export interface ShellResult {
  stdout: string;
  stderr: string;
  exit_code: number;
}

/** Shell 执行选项 */
export interface ShellExecOptions {
  timeout_ms?: number;
  working_dir?: string;
}

/** Shell API */
export class PluginShellAPI {
  private pluginId: string;

  constructor(pluginId: string) {
    this.pluginId = pluginId;
  }

  async exec(command: string, options?: ShellExecOptions): Promise<ShellResult> {
    return invoke<ShellResult>('plugin_shell_exec', {
      pluginId: this.pluginId,
      command,
      options: options || null,
    });
  }
}
