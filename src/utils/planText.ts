/**
 * 会员相关文案：到期时间的展示口径（弹层、设置页共用一份，避免两处漂移）。
 */

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
