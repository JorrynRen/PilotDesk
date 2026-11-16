/** 项目工作归属。项目不是独立工作模式，只把会话、圆桌和工作流组织到一起。 */

export interface ProjectDirectory {
  id: string;
  projectId: string;
  path: string;
  isPrimary: boolean;
  sortOrder: number;
  createdAt: number;
  updatedAt: number;
}

export interface Project {
  id: string;
  name: string;
  description: string;
  icon: string;
  archivedAt?: number | null;
  createdAt: number;
  updatedAt: number;
  lastActivityAt: number;
  directories: ProjectDirectory[];
  sessionCount: number;
  roomCount: number;
  workflowCount: number;
}

export interface ProjectDirectoryInput {
  path: string;
  isPrimary: boolean;
}

export interface ProjectInput {
  name: string;
  description: string;
  icon: string;
  directories: ProjectDirectoryInput[];
}

export type ProjectWorkKind = 'session' | 'groupchat' | 'workflow';

/** 项目详情中的统一工作摘要。 */
export interface ProjectWorkItem {
  kind: ProjectWorkKind;
  id: string;
  title: string;
  subtitle: string;
  status: string;
  updatedAt: number;
}

export type ProjectCreateKind = 'session' | 'groupchat' | 'workflow';

export interface ProjectCreateContext {
  kind: ProjectCreateKind;
  projectId: string;
  projectName: string;
  /** 创建弹窗首次打开时的展示值；实际运行仍动态读取项目最新主目录。 */
  primaryDirectory?: string;
}
