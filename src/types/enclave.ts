/**
 * 飞地（Enclave）前端类型：与 Rust serde 输出对齐（`rename_all = "camelCase"`）。
 *
 * 权威分工（见设计文档《飞地（Enclave）产品设计与落地方案》）：
 * 组织/成员/角色以平台为准（这里只读快照）；任务/汇报/产出/文件以主机为准。
 */

/** 飞地模块状态（`enclave_status`）。 */
export interface EnclaveStatus {
  /** 飞地数据库是否已打开并就绪 */
  ready: boolean;
  /** `enclave.db` 的绝对路径 */
  dbPath: string;
  /** 打开时确定的结构版本 */
  schemaVersion: number;
}

/** 组织快照视图（`enclave_org_list` / `enclave_org_detail`）：来自平台，主机侧只读。 */
export interface OrgSnapshotView {
  orgId: string;
  name: string;
  /** 平台 owner（即主机身份来源）；平台补齐 ownerUserId 前为空串 */
  ownerUserId: string;
  planKey: string;
  planExpiresAt: string | null;
  /** 席位总数 */
  seats: number;
  /** 团队成员数 */
  memberCount: number;
  status: string;
  /** 本快照同步时间（unix 秒） */
  syncedAt: number;
}

/** 成员快照视图：`seatStatus = 'removed'` 表示已离席（保留历史，不物理删）。 */
export interface MemberView {
  userId: string;
  /** owner / admin / member */
  role: string;
  displayName: string;
  email: string;
  /** active / removed */
  seatStatus: string;
  syncedAt: number;
}

/** 组织详情视图：组织信息 + 成员列表（按 owner → admin → member 排序）。 */
export interface OrgDetailView {
  org: OrgSnapshotView;
  members: MemberView[];
}

/** 审计流水视图（后续里程碑接入策略 / 应急 / 转移变更）。 */
export interface AuditView {
  id: string;
  orgId: string;
  actorId: string;
  action: string;
  /** 结构化明细 JSON 字符串 */
  detail: string;
  createdAt: number;
}

/** 一轮治理快照刷新结果（`enclave_snapshot_refresh`）。 */
export interface SnapshotOutcome {
  orgCount: number;
  memberCount: number;
  syncedAt: number;
  /**
   * 成员列表拉取失败的原因（仅首个；字段缺省表示全部成功）。
   *
   * 平台成员列表接口属待对齐项：该接口缺失时组织快照照常写入，成员表留空并在此给出提示。
   */
  memberError?: string;
}
