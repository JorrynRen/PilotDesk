import { ArrowRight } from 'lucide-react';

import {
  useAccountStore,
  type UpgradeContext,
  type UpgradePlanKey,
} from '../../stores/accountStore';

interface UpgradePromptProps {
  capability: string;
  plan: UpgradePlanKey;
  title: string;
  description: string;
  source: string;
  className?: string;
  compact?: boolean;
  /**
   * 即使个人账号已具备该能力也展示。
   *
   * 用于“个人权益已解锁，但当前组织仍未解锁”的场景；组织页需要独立展示组织级缺口。
   */
  forceShow?: boolean;
  /** 覆盖默认的“查看团队版 / 查看专业版”按钮文案 */
  actionLabel?: string;
  /** 组织权益需要前往组织页时，可覆盖默认的官网升级页入口 */
  onUpgrade?: () => void;
}

/**
 * 能力受阻时的统一升级提示。
 *
 * 只展示“当前场景多出什么能力 + 推荐档位 + 下一步”，不在全局常驻推销；
 * 已解锁该能力的账号不渲染。
 */
export function UpgradePrompt({
  capability,
  plan,
  title,
  description,
  source,
  className = '',
  compact = false,
  forceShow = false,
  actionLabel,
  onUpgrade,
}: UpgradePromptProps) {
  const account = useAccountStore((state) => state.account);
  const openUpgrade = useAccountStore((state) => state.openUpgrade);

  if (!forceShow && account?.capabilities.includes(capability)) return null;

  const planName = plan === 'team' ? '团队版' : '专业版';
  const handleUpgrade = () => {
    if (onUpgrade) {
      onUpgrade();
      return;
    }
    const context: UpgradeContext = { plan, capability, source };
    void openUpgrade(context);
  };

  return (
    <div
      className={`pd-upgrade-prompt ${className}`}
      data-compact={compact ? 'true' : 'false'}
    >
      <div className="pd-upgrade-prompt-copy">
        <div className="pd-upgrade-prompt-title-row">
          <span className="pd-upgrade-prompt-plan">{planName}</span>
          <span className="pd-upgrade-prompt-title">{title}</span>
        </div>
        <p className="pd-upgrade-prompt-description">{description}</p>
      </div>
      <button
        type="button"
        className="pd-btn pd-upgrade-prompt-action"
        onClick={handleUpgrade}
      >
        {actionLabel ?? `查看${planName}`}
        <ArrowRight size={12} />
      </button>
    </div>
  );
}
