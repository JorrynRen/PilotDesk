import type { ToastType } from './toast';

/**
 * 单个工作流成员的导入动作（与 Rust `ImportMemberAction` 对齐）。
 *
 * `created` 新建 / `updated` 覆盖 / `skipped` 跳过（本地已有同 ID）。
 */
export type ImportMemberAction = 'created' | 'updated' | 'skipped';

/** 单个成员的导入结果明细（与 Rust `ImportMemberOutcome` 对齐） */
export interface ImportMemberOutcome {
  id: string;
  name: string;
  action: ImportMemberAction;
  /** 导入后被多少个其它工作流引用（无引用为 0） */
  referencingCount: number;
}

/** 提示文案里逐条列出的名称上限，超出折叠为「等 N 个」 */
const MAX_LISTED_NAMES = 5;

/** 拼接名称清单：超过上限时折叠为「等 N 个」 */
function joinNames(names: string[]): string {
  if (names.length <= MAX_LISTED_NAMES) return names.join('、');
  return `${names.slice(0, MAX_LISTED_NAMES).join('、')} 等 ${names.length} 个`;
}

/** 成员显示名（缺失时回退占位，避免提示里出现空串） */
function displayName(name: string): string {
  return name?.trim() ? name : '（未命名）';
}

/**
 * 由导入明细生成提示文案；无成员（空包）时返回 `null`（不打扰用户）。
 *
 * 只要发生「未新建」（覆盖 / 跳过）都必须给出可见提示；被引用的子流被更新时追加说明。
 * 文案为**单行**（Toast 不保留换行），用「；」分隔各段。
 */
export function buildImportMemberMessage(
  members?: ImportMemberOutcome[] | null,
): { message: string; type: ToastType } | null {
  if (!members || members.length === 0) return null;

  const created = members.filter((m) => m.action === 'created');
  const updated = members.filter((m) => m.action === 'updated');
  const skipped = members.filter((m) => m.action === 'skipped');
  // 三者都为 0（空包）时不弹
  if (created.length === 0 && updated.length === 0 && skipped.length === 0) return null;

  const parts: string[] = [];
  if (created.length) parts.push(`新建 ${created.length} 个`);
  if (updated.length) {
    parts.push(`覆盖 ${updated.length} 个（原内容已备份为本地版本，可在「版本」中回滚）`);
  }
  if (skipped.length) parts.push(`跳过 ${skipped.length} 个（内容相同，未改动）`);

  const segments: string[] = [`已导入 ${members.length} 个工作流：${parts.join(' · ')}`];
  if (updated.length) {
    segments.push(`覆盖：${joinNames(updated.map((m) => displayName(m.name)))}`);
  }
  if (skipped.length) {
    segments.push(`跳过：${joinNames(skipped.map((m) => displayName(m.name)))}`);
  }
  for (const m of updated.filter((m) => m.referencingCount > 0)) {
    segments.push(
      `子工作流「${displayName(m.name)}」已被 ${m.referencingCount} 个工作流引用，本次已更新它`,
    );
  }

  return {
    message: segments.join('；'),
    // 覆盖了本地内容 → 用警示色引起注意；仅跳过 → 信息色；全部新建 → 成功色
    type: updated.length > 0 ? 'warning' : skipped.length > 0 ? 'info' : 'success',
  };
}
