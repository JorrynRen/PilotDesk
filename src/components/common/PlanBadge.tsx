/**
 * 等级徽章：免费 / 专业 / 团队一眼可辨。
 *
 * 注意：状态栏背景本身就是 `var(--bg-secondary)`，徽章若也用主题背景变量会**完全看不见**
 * ——所以这里用**固定实色 + 描边 + 白字**，深浅色模式下对比度都足够，不依赖主题变量。
 * 已过期转琥珀；离线（平台不可达、按缓存显示）追加「离线」。
 */
interface Props {
  planKey: string;
  planName: string;
  expired?: boolean;
  stale?: boolean;
}

interface BadgeStyle {
  bg: string;
  fg: string;
  border: string;
}

const PLAN_STYLES: Record<string, BadgeStyle> = {
  free: { bg: '#64748B', fg: '#FFFFFF', border: '#475569' },
  pro: { bg: '#2563EB', fg: '#FFFFFF', border: '#1D4ED8' },
  team: { bg: '#7C3AED', fg: '#FFFFFF', border: '#6D28D9' },
};
const FALLBACK_STYLE = PLAN_STYLES.free;
const EXPIRED_STYLE: BadgeStyle = { bg: '#D97706', fg: '#FFFFFF', border: '#B45309' };

export function PlanBadge({ planKey, planName, expired, stale }: Props) {
  const style = expired ? EXPIRED_STYLE : (PLAN_STYLES[planKey] ?? FALLBACK_STYLE);
  return (
    <span
      className="px-2 py-0.5 rounded-full text-[10px] font-semibold whitespace-nowrap leading-4"
      style={{ backgroundColor: style.bg, color: style.fg, border: `1px solid ${style.border}` }}
    >
      {planName}
      {expired ? ' · 已过期' : ''}
      {stale ? ' · 离线' : ''}
    </span>
  );
}
