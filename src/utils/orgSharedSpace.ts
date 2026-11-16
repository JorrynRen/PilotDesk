import {
  CAP_TEAM_SHARED_SPACE,
  CAP_TEAM_USAGE_BOARD,
} from '../stores/accountStore';

/** 组织共享空间使用的组织摘要（与 Rust `org_list_mine` 返回对齐）。 */
export interface OrganizationSummary {
  id: number;
  name: string;
  role: string;
  planKey: string;
  planExpiresAt: string | null;
  capabilities: string[];
  seats: number;
  memberCount: number;
  status: string;
}

/** 组织能力以平台下发为准；缺少明确能力时不授予受控操作。 */
export function orgHasCapability(
  org: OrganizationSummary | null | undefined,
  capability: string,
): boolean {
  return !!org && (org.capabilities ?? []).includes(capability);
}

/** 向组织上报用量只允许落到已开通团队用量看板的组织。 */
export function orgSupportsUsageBoard(
  org: OrganizationSummary | null | undefined,
): boolean {
  return orgHasCapability(org, CAP_TEAM_USAGE_BOARD);
}

/** 组织共享空间只随团队档能力下发；旧平台缺 capabilities 时再按档位兼容。 */
export function orgSupportsSharedSpace(org: OrganizationSummary | null | undefined): boolean {
  if (!org) return false;
  const capabilities = org.capabilities ?? [];
  if (capabilities.includes(CAP_TEAM_SHARED_SPACE)) return true;
  if (capabilities.length > 0) return false;
  const planKey = org.planKey.trim().toLowerCase();
  if (!planKey || planKey === 'free' || planKey === 'pro') return false;
  if (org.planExpiresAt) {
    const expiresAt = Date.parse(org.planExpiresAt);
    if (Number.isFinite(expiresAt) && expiresAt <= Date.now()) return false;
  }
  return true;
}

export function canManageOrganization(
  org: OrganizationSummary | null | undefined,
): boolean {
  return org?.role === 'owner' || org?.role === 'admin';
}
