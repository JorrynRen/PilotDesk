/**
 * 账号头像渲染（与平台内置头像目录对齐，见 docs/avatar-api.md）。
 *
 * 平台只下发头像键（`avatarId`），像素全部在本地生成：
 * - 命中预设 → 渐变底 + 白色几何图形；
 * - `null` / 未知键 / 老平台 → 确定性渐变 + 首字母。
 * 不涉及任何图片上传、下载或本地文件。
 */

interface Props {
  nickname?: string | null;
  email?: string | null;
  /** 平台下发的内置头像键；空值回退首字母头像 */
  avatarId?: string | null;
  /** 边长（px），默认 32。 */
  size?: number;
  /** 圆角（px），默认 8。 */
  radius?: number;
  className?: string;
}

type AvatarMotif = 'sun' | 'wave' | 'peak' | 'moon' | 'diamond' | 'crystal' | 'orbit' | 'bloom';

interface AvatarPreset {
  id: string;
  label: string;
  from: string;
  to: string;
  motif: AvatarMotif;
}

/** 顺序与平台目录一致：暖色 → 冷色 */
export const AVATAR_PRESETS: readonly AvatarPreset[] = [
  { id: 'dawn', label: '晨光', from: '#FB923C', to: '#BE123C', motif: 'sun' },
  { id: 'ember', label: '余烬', from: '#F59E0B', to: '#9A3412', motif: 'diamond' },
  { id: 'bloom', label: '花影', from: '#F472B6', to: '#9D174D', motif: 'bloom' },
  { id: 'pine', label: '松岭', from: '#34D399', to: '#065F46', motif: 'peak' },
  { id: 'quartz', label: '石英', from: '#22D3EE', to: '#155E75', motif: 'crystal' },
  { id: 'tide', label: '潮汐', from: '#38BDF8', to: '#1E3A8A', motif: 'wave' },
  { id: 'orbit', label: '星轨', from: '#818CF8', to: '#312E81', motif: 'orbit' },
  { id: 'dusk', label: '暮色', from: '#A78BFA', to: '#4C1D95', motif: 'moon' },
];

/** FNV-1a 32 位：同一输入稳定得到同一色相 */
function hashString(value: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < value.length; i++) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** 头像首字母：昵称优先，其次邮箱，均无则用问号占位。 */
export function avatarInitial(nickname?: string | null, email?: string | null): string {
  const source = (nickname || email || '').trim();
  return source ? source.charAt(0).toUpperCase() : '?';
}

/** 48×48 画布上的白色几何图形 */
function Motif({ motif }: { motif: AvatarMotif }) {
  switch (motif) {
    case 'sun':
      return (
        <>
          <circle cx={24} cy={21} r={7} />
          <path d="M9 34h30" fill="none" stroke="#fff" strokeWidth={3.5} strokeLinecap="round" />
        </>
      );
    case 'wave':
      return (
        <>
          {[16, 24, 32].map((y) => (
            <path
              key={y}
              d={`M6 ${y}c4-3.5 8-3.5 12 0s8 3.5 12 0 8-3.5 12 0`}
              fill="none"
              stroke="#fff"
              strokeWidth={3}
              strokeLinecap="round"
            />
          ))}
        </>
      );
    case 'peak':
      return <path d="M7 36l10-15 7 9 4-5 9 11z" />;
    case 'moon':
      return <path d="M28 8a13 13 0 1 0 0 26 10.5 10.5 0 0 1 0-26z" />;
    case 'diamond':
      return <path d="M24 9l14 15-14 15L10 24z" />;
    case 'crystal':
      return (
        <>
          <path d="M24 8l12 9-4.5 21h-15L12 17z" />
          <path d="M24 8v30" fill="none" stroke="#fff" strokeOpacity={0.55} strokeWidth={1.6} />
        </>
      );
    case 'orbit':
      return (
        <>
          <circle cx={24} cy={24} r={11} fill="none" stroke="#fff" strokeWidth={2.6} strokeOpacity={0.9} />
          <circle cx={34.2} cy={17.4} r={3.8} />
        </>
      );
    case 'bloom':
      return (
        <>
          <circle cx={24} cy={12.5} r={6.4} />
          <circle cx={35.5} cy={24} r={6.4} />
          <circle cx={24} cy={35.5} r={6.4} />
          <circle cx={12.5} cy={24} r={6.4} />
          <circle cx={24} cy={24} r={4.6} />
        </>
      );
  }
}

export function AccountAvatar({
  nickname,
  email,
  avatarId,
  size = 32,
  radius = 8,
  className = '',
}: Props) {
  const preset = AVATAR_PRESETS.find((p) => p.id === avatarId);

  if (preset) {
    const gradientId = `pd-avatar-${preset.id}`;
    return (
      <span
        className={`shrink-0 overflow-hidden select-none ${className}`}
        style={{ width: size, height: size, borderRadius: radius }}
      >
        <svg width={size} height={size} viewBox="0 0 48 48" aria-hidden>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
              <stop offset="0" stopColor={preset.from} />
              <stop offset="1" stopColor={preset.to} />
            </linearGradient>
          </defs>
          <rect width={48} height={48} fill={`url(#${gradientId})`} />
          <g fill="#fff" fillOpacity={0.92}>
            <Motif motif={preset.motif} />
          </g>
        </svg>
      </span>
    );
  }

  // 优先用邮箱作为种子：昵称可随时修改，邮箱在同一账号内稳定
  const seed = (email || nickname || '').trim().toLowerCase() || '?';
  const hue = hashString(seed) % 360;
  const hueShift = (hue + 36) % 360;

  return (
    <span
      className={`flex items-center justify-center shrink-0 font-semibold text-white select-none ${className}`}
      style={{
        width: size,
        height: size,
        borderRadius: radius,
        fontSize: Math.max(10, Math.round(size * 0.4)),
        lineHeight: 1,
        background: `linear-gradient(135deg, hsl(${hue} 58% 44%), hsl(${hueShift} 62% 32%))`,
      }}
    >
      {avatarInitial(nickname, email)}
    </span>
  );
}
