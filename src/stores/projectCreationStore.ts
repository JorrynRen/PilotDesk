import { create } from 'zustand';
import type { ProjectCreateContext, ProjectCreateKind } from '../types/project';

type ProjectCreationListener = (context: ProjectCreateContext) => void;

const listeners: Record<ProjectCreateKind, Set<ProjectCreationListener>> = {
  session: new Set(),
  groupchat: new Set(),
  workflow: new Set(),
};

interface ProjectCreationState {
  pending: ProjectCreateContext | null;
  begin: (context: ProjectCreateContext) => void;
  /** 只允许一次性消费，避免之后普通新建弹窗继续带上项目。 */
  take: (kind: ProjectCreateKind) => ProjectCreateContext | null;
  clear: () => void;
}

export const useProjectCreationStore = create<ProjectCreationState>((set, get) => ({
  pending: null,

  begin: (pending) => {
    set({ pending });

    // 工作台页面跨路由常驻，不能只等下一次挂载时读取 pending。
    // 已有监听者时立即交付一次，未挂载时则保留给首次订阅消费。
    let delivered = false;
    for (const listener of listeners[pending.kind]) {
      delivered = true;
      listener(pending);
    }
    if (delivered) set({ pending: null });
  },
  take: (kind) => {
    const pending = get().pending;
    if (!pending || pending.kind !== kind) return null;
    set({ pending: null });
    return pending;
  },
  clear: () => set({ pending: null }),
}));

export function subscribeProjectCreation(
  kind: ProjectCreateKind,
  listener: ProjectCreationListener,
): () => void {
  listeners[kind].add(listener);

  const pending = useProjectCreationStore.getState().take(kind);
  if (pending) listener(pending);

  return () => {
    listeners[kind].delete(listener);
  };
}
