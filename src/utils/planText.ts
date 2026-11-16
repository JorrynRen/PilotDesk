/**
 * 会员相关文案：到期时间的展示口径（弹层、设置页共用一份，避免两处漂移）。
 */

export const CLOUD_SYNC_WORKFLOW_ITEMS = 'sync.workflow-items';
export const CLOUD_SYNC_INSPIRATION_ITEMS = 'sync.inspiration-items';

/** 个人云同步配额摘要：`工作流 100 条、灵感 100 条`；缺省项不展示，-1 显示为不限 */
export function cloudSyncQuotaSummary(
  quotas: Readonly<Record<string, number>> | null | undefined,
): string {
  const valueText = (value: number): string => (value < 0 ? '不限' : `${value} 条`);
  return [
    [CLOUD_SYNC_WORKFLOW_ITEMS, '工作流'],
    [CLOUD_SYNC_INSPIRATION_ITEMS, '灵感'],
  ].flatMap(([key, label]) => {
    const value = quotas?.[key];
    return value === undefined ? [] : [`${label} ${valueText(value)}`];
  }).join('、');
}

/** 权益清单中的个人云同步名称；有配额时附带额度，避免把额度误认为版本数或保存天数 */
export function cloudSyncEntitlementLabel(
  quotas: Readonly<Record<string, number>> | null | undefined,
): string {
  const summary = cloudSyncQuotaSummary(quotas);
  return summary ? `个人云同步（${summary}）` : '个人云同步';
}

/** ISO → `2026/11/4（剩 31 天）`；已过期 → `2026/10/1（已过期 3 天）`；非法值原样返回 */
export function expiryText(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const days = Math.ceil((date.getTime() - Date.now()) / 86400000);
  const label = date.toLocaleDateString('zh-CN');
  if (days <= 0) {
    const overdue = Math.floor((Date.now() - date.getTime()) / 86400000);
    return `${label}（${overdue >= 1 ? `已过期 ${overdue} 天` : '已过期'}）`;
  }
  return `${label}（剩 ${days} 天）`;
}
