/**
 * PluginAPI.fs — 文件系统 API 实现
 *
 * 通过 Tauri invoke 调用 Rust 侧 fs 命令。
 * 沙箱启用时所有操作被拒绝。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import { invoke } from '@tauri-apps/api/core';

/** 文件条目 */
export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

/** 文件系统 API */
export class PluginFSAPI {
  private pluginId: string;

  constructor(pluginId: string) {
    this.pluginId = pluginId;
  }

  async readText(path: string): Promise<string> {
    return invoke<string>('plugin_fs_read_text', { pluginId: this.pluginId, path });
  }

  async writeText(path: string, content: string): Promise<void> {
    return invoke<void>('plugin_fs_write_text', { pluginId: this.pluginId, path, content });
  }

  async delete(path: string): Promise<void> {
    return invoke<void>('plugin_fs_delete', { pluginId: this.pluginId, path });
  }

  async exists(path: string): Promise<boolean> {
    return invoke<boolean>('plugin_fs_exists', { pluginId: this.pluginId, path });
  }

  async readDir(path: string): Promise<FileEntry[]> {
    return invoke<FileEntry[]>('plugin_fs_read_dir', { pluginId: this.pluginId, path });
  }
}
