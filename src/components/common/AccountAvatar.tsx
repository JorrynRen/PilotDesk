/**
 * 账号头像（确定性生成，无图片存储）。
 *
 * 用 email（无则昵称）的哈希推导固定色相，渲染「双色渐变底 + 首字母」：
 * 同一账号在任何设备、任何主题下都是同一张头像，不需要上传、落盘或请求平台。
 * 后续接入真实头像时，只需在本组件内优先渲染图片并在失败时回退到这里。
 */

interface Props {
  nickname?: string | null;
  email?: string | null;
  /** 边长（px），默认 32。 */
  size?: number;
  /** 圆角（px），默认 8。 */
  radius?: number;
  className?: string;
}

/** FNV-1a 32 位：同一输入稳定得到同一色相。 */
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

export function AccountAvatar({ nickname, email, size = 32, radius = 8, className = '' }: Props) {
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
      aria-hidden
    >
      {avatarInitial(nickname, email)}
    </span>
  );
}
