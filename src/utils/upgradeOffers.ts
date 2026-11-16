import type { UpgradePlanKey } from '../stores/accountStore';

/** 一个可直接展示、可执行的能力升级建议 */
export interface UpgradeOffer {
  capability: string;
  plan: UpgradePlanKey;
  planName: string;
  title: string;
  description: string;
}

export const CLOUD_SYNC_UPGRADE: UpgradeOffer = {
  capability: 'sync.cloud',
  plan: 'pro',
  planName: '专业版',
  title: '让工作流跟着你走',
  description: '跨设备同步完整工作流定义与灵感；不含凭据、插件、技能、执行记录和产物。',
};

export const REMOTE_RUN_UPGRADE: UpgradeOffer = {
  capability: 'workflow.remote-run',
  plan: 'pro',
  planName: '专业版',
  title: '在任何设备发起工作流',
  description: '远程请求只同步工作流标识与状态，执行过程、日志和产物始终留在目标设备。',
};

export const SUPPORT_PRIORITY_UPGRADE: UpgradeOffer = {
  capability: 'support.priority',
  plan: 'pro',
  planName: '专业版',
  title: '获得优先在线服务响应',
  description: '8×5 服务时间内，P1 4 个工作小时响应、P2 1 个工作日内响应；响应时间不等于解决时间。',
};

export const TEAM_MEMBERS_UPGRADE: UpgradeOffer = {
  capability: 'team.members',
  plan: 'team',
  planName: '团队版',
  title: '邀请成员，共同使用组织订阅',
  description: '组织默认仅 1 席；团队版支持邀请成员、角色治理与席位调整。',
};

export const TEAM_SHARE_UPGRADE: UpgradeOffer = {
  capability: 'team.shared-space',
  plan: 'team',
  planName: '团队版',
  title: '把好用的工作流交给团队复用',
  description: '组织共享空间支持共享、导入工作流，并保留版本与更新记录。',
};

export const TEAM_USAGE_UPGRADE: UpgradeOffer = {
  capability: 'team.usage-board',
  plan: 'team',
  planName: '团队版',
  title: '看清团队的云端用量',
  description: '汇总成员上报的调用次数与 token 用量，便于容量规划和成本分摊。',
};

export const TEAM_AUDIT_UPGRADE: UpgradeOffer = {
  capability: 'team.audit',
  plan: 'team',
  planName: '团队版',
  title: '让组织操作可追溯',
  description: '查看成员、席位与订阅操作记录，满足团队治理和审计需要。',
};

export const SSO_UPGRADE: UpgradeOffer = {
  capability: 'sso.login',
  plan: 'team',
  planName: '团队版',
  title: '接入企业身份登录',
  description: '通过 OIDC 企业身份源登录，减少共享密码并统一成员入口。',
};

const UPGRADE_OFFERS: readonly UpgradeOffer[] = [
  CLOUD_SYNC_UPGRADE,
  REMOTE_RUN_UPGRADE,
  SUPPORT_PRIORITY_UPGRADE,
  TEAM_MEMBERS_UPGRADE,
  TEAM_SHARE_UPGRADE,
  TEAM_USAGE_UPGRADE,
  TEAM_AUDIT_UPGRADE,
  SSO_UPGRADE,
];

/**
 * 账号页 / 顶栏弹层展示的“下一步最值得解锁”。
 *
 * 推荐顺序：云同步、远程发起、优先在线服务，最后才是团队治理能力。
 * 已具备全部受限能力时不再打扰。
 */
export function nextUpgradeOffer(capabilities: readonly string[]): UpgradeOffer | null {
  return UPGRADE_OFFERS.find((offer) => !capabilities.includes(offer.capability)) ?? null;
}
