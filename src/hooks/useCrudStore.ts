import { create } from 'zustand';

/**
 * 通用 CRUD store 工厂函数。
 *
 * 消除 inspirationStore / apiProviderStore 中重复的 fetch→invoke→set 模式。
 *
 * 用法:
 *   const useMyStore = createCrudStore<MyItem>({
 *     name: 'myItems',
 *     listCmd: 'list_my_items',
 *     emptyList: [],
 *   });
 *
 *   自动生成: items[], loading, error, fetchItems, addItem, updateItem, removeItem
 */

interface CrudState<T> {
  items: T[];
  loading: boolean;
  error: string | null;
}

interface CrudActions<T> {
  /** 从后端拉取列表 */
  fetch: (params?: Record<string, unknown>) => Promise<void>;
  /** 添加一项并刷新列表 */
  add: (cmd: string, payload: Record<string, unknown>) => Promise<T | null>;
  /** 更新一项并刷新列表 */
  update: (cmd: string, payload: Record<string, unknown>) => Promise<void>;
  /** 删除一项并刷新列表 */
  remove: (cmd: string, id: string) => Promise<void>;
  /** 重置状态 */
  reset: () => void;
}

export type CrudStore<T> = CrudState<T> & CrudActions<T>;

interface CrudConfig {
  name: string;
  listCmd: string;
}

/**
 * 创建符合 CRUD 模式的 Zustand store。
 *
 * @param config.name 用于日志和调试的名称
 * @param config.listCmd 列表查询的 Tauri command 名称
 */
export function createCrudStore<T extends { id: string }>(config: CrudConfig) {
  const { listCmd } = config;

  const initialState: CrudState<T> = {
    items: [],
    loading: false,
    error: null,
  };

  return create<CrudStore<T>>((set, get) => ({
    ...initialState,

    fetch: async (params?: Record<string, unknown>) => {
      set({ loading: true, error: null });
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const items = await invoke<T[]>(listCmd, params ?? {});
        set({ items, loading: false });
      } catch (err) {
        set({ error: String(err), loading: false });
      }
    },

    add: async (cmd: string, payload: Record<string, unknown>) => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const result = await invoke<T>(cmd, payload);
        await get().fetch();
        return result;
      } catch (err) {
        set({ error: String(err) });
        return null;
      }
    },

    update: async (cmd: string, payload: Record<string, unknown>) => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke(cmd, payload);
        await get().fetch();
      } catch (err) {
        set({ error: String(err) });
      }
    },

    remove: async (cmd: string, id: string) => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke(cmd, { id });
        await get().fetch();
      } catch (err) {
        set({ error: String(err) });
      }
    },

    reset: () => set({ ...initialState }),
  }));
}
