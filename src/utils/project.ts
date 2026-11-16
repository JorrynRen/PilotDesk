import type { Project } from '../types/project';
import { normalizePathForComparison } from './path';

/** 项目主目录：优先显式主目录；异常旧数据没有主目录标记时退回第一个目录。 */
export function projectPrimaryDirectory(project?: Project | null): string {
  return project?.directories.find((directory) => directory.isPrimary)?.path
    ?? project?.directories[0]?.path
    ?? '';
}

/** 前端展示用途的路径等价判断：兼容分隔符差异与 Windows 盘符大小写。 */
export function sameProjectPath(left?: string | null, right?: string | null): boolean {
  const leftPath = normalizePathForComparison(left);
  return Boolean(leftPath) && leftPath === normalizePathForComparison(right);
}

/** 关联目录：排除主目录标记项；兼容旧数据未正确标记主目录时按路径去重。 */
export function projectLinkedDirectories(project?: Project | null): Project['directories'] {
  const primary = projectPrimaryDirectory(project);
  return project?.directories.filter(
    (directory) => !directory.isPrimary && !sameProjectPath(directory.path, primary),
  ) ?? [];
}

/** 与后端 `sanitize_dir_name` 保持同一规则的产物目录名。 */
export function workspaceArtifactName(title?: string | null, fallback = 'workspace'): string {
  const cleaned = (title ?? '').replace(/[<>:"/\\|?*]/g, '').trim().replace(/[. ]+$/g, '');
  const truncated = Array.from(cleaned).slice(0, 64).join('');
  if (!truncated) return fallback;
  const base = truncated.split('.')[0]?.toUpperCase() ?? '';
  return /^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/.test(base)
    ? `${truncated}_dir`
    : truncated;
}

/** 展示用产物目录；真实执行路径以后端 `artifactRoot` 为准。 */
export function workspaceArtifactRelativePath(title?: string | null, fallback = 'workspace'): string {
  return `outputs/${workspaceArtifactName(title, fallback)}`;
}
