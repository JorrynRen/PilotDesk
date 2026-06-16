import { invoke } from '@tauri-apps/api/core';

/**
 * 通用 Tauri invoke 调用包装。
 * 消除重复的 try-catch 和类型标注。
 */

export async function listItems<T>(cmd: string, params?: Record<string, unknown>): Promise<T[]> {
  return invoke<T[]>(cmd, params ?? {});
}

export async function getItem<T>(cmd: string, params: Record<string, unknown>): Promise<T | null> {
  try {
    return await invoke<T>(cmd, params);
  } catch {
    return null;
  }
}

export async function saveItem<T>(cmd: string, payload: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, { payload });
}

export async function deleteItem(cmd: string, id: string): Promise<void> {
  await invoke(cmd, { id });
}

export async function invokeAction<T = void>(cmd: string, params?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, params ?? {});
}
