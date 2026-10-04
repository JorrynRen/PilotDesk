/**
 * 等级徽章：免费 / 专业 / 团队用不同配色 —— 比纯文字更直观。
 * 已过期转琥珀色；离线（平台不可达、按缓存显示）追加「离线」。
 */
interface Props {
  planKey: string;
  planName: string;
  expired?: boolean;
  stale?: boolean;
}

const PLAN_STYLES: Record<string, { bg: string; color: string }> = {
  free: { bg: 'var(--bg-secondary)', color: 'var(--text-secondary)' },
  pro: { bg: 'rgba(59, 130, 246, 0.16)', color: '#3B82F6' },
  team: { bg: 'rgba(139, 92, 246, 0.16)', color: '#8B5CF6' },
};
const EXPIRED_STYLE = { bg: 'rgba(245, 158, 11, 0.16)', color: '#F59E0B' };

export function PlanBadge({ planKey, planName, expired, stale }: Props) {
  const style = expired ? EXPIRED_STYLE : (PLAN_STYLES[planKey] ?? PLAN_STYLES.free);
  return (
    <span
      className="px-1.5 py-0.5 rounded text-[10px] font-medium whitespace-nowrap"
      style={{ backgroundColor: style.bg, color: style.color }}
    >
      {planName}
      {expired ? ' · 已过期' : ''}
      {stale ? ' · 离线' : ''}
    </span>
  );
}
