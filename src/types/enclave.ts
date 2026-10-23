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
  /** 当前登录用户在该组织中的角色（owner / admin / member），前端据此做显隐 */
  myRole: string;
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

/** 本机登录身份快照（平台 /me） */
export interface LocalIdentityView {
  userId: string;
  email: string;
  nickname: string;
  syncedAt: number;
}

/**
 * 组织级能力视图（`enclave_org_caps`）。
 *
 * `actions` 是后端授权器算出的可执行动作，**前端只按它渲染治理按钮**（唯一来源）。
 */
export interface EnclaveOrgCaps {
  orgId: string;
  identity: LocalIdentityView | null;
  /** owner / admin / member；未同步为 null */
  role: string | null;
  /** 可执行动作，如 "policy.write" / "emergency.set" / "org.admin" */
  actions: string[];
  snapshotFresh: boolean;
  syncedAt: number;
  disabled: boolean;
}

/** 数据出境总闸视图（`enclave_policy_get`） */
export interface PolicyView {
  orgId: string;
  allowCloudModel: boolean;
  updatedBy: string;
  updatedAt: number;
  /** 是否已显式设置过；false 表示按默认「不允许」 */
  explicit: boolean;
}

/** 主机应急开关视图（`enclave_emergency_get`） */
export interface EmergencyView {
  orgId: string;
  disabled: boolean;
  updatedBy: string;
  updatedAt: number;
}

/** 主机服务状态视图（`enclave_host_status`） */
export interface HostStatusView {
  /** host = 本机是组织 owner 并对外提供服务；member = 本机为成员，需连接主机 */
  role: 'host' | 'member' | 'unknown';
  running: boolean;
  port: number;
  hostName: string;
  servedOrgIds: string[];
  disabled: boolean;
  lastError: string | null;
  /** 成员端手动填写的主机地址（优先于自动发现） */
  manualAddr: string | null;
  /** 成员端 UDP 自动发现到的主机 */
  discoveredAddr: string | null;
  discoveredHostName: string | null;
  discoveredSeenAt: number | null;
  /** 平台登记下发的主机地址（广播被禁时的连接兜底） */
  registeredAddr: string | null;
  /** 本机作为主机时的证书指纹（sha256 hex） */
  hostFingerprint: string | null;
  /** 成员端已固定的主机证书指纹（sha256 hex） */
  pinnedFingerprint: string | null;
  /** 本机设备标识 */
  deviceId: string;
  /** 当前组织登记的主机设备 id（空 = 尚未登记） */
  hostDeviceId: string | null;
  /** 当前组织登记的主机设备名 */
  hostDeviceName: string | null;
}

/** 同账号换机接管结果（`enclave_host_takeover`） */
export interface HandoverOutcome {
  orgId: string;
  packageHash: string;
  fileCount: number;
}

// ── 沟通层：通知 ────────────────────────────────────────────────

/** 任务评论（`enclave_task_comment_list`）；与过程记录同表，但语义是讨论 */
export interface TaskCommentView {
  id: string;
  taskId: string;
  authorId: string;
  content: string;
  /** 回复的评论 id（顶层评论为 null） */
  replyToId: string | null;
  /** @ 提及的成员 user id */
  mentions: string[];
  createdAt: number;
}

/** 站内通知（`enclave_notification_list`）；写主机库，成员经主机转发读取 */
export interface NotificationView {
  id: string;
  orgId: string;
  userId: string;
  /** task.assigned / report.submitted / report.confirmed / report.returned */
  kind: string;
  title: string;
  body: string;
  /** task / report */
  targetType: string;
  targetId: string;
  readAt: number | null;
  createdAt: number;
}

// ── 主机自动备份 ────────────────────────────────────────────────

export interface BackupStatusView {
  enabled: boolean;
  intervalHours: number;
  keep: number;
  /** 最近一次成功备份（unix 秒，0 = 从未备份） */
  lastAt: number;
  lastPath: string;
  lastSize: number;
  lastError: string;
  dir: string;
}

export interface BackupInfo {
  path: string;
  size: number;
  fileCount: number;
  orgCount: number;
  createdAt: number;
}

/** 本机文件缓存信息（`enclave_cache_info`） */
export interface CacheInfoView {
  /** 缓存目录绝对路径 */
  dir: string;
  /** 当前占用字节数 */
  usedBytes: number;
  /** 上限字节数（默认 2 GB） */
  maxBytes: number;
  /** 缓存文件数 */
  fileCount: number;
}

// ── M8：主机转移 ────────────────────────────────────────────────

/** 转移状态：verified 必须早于 completed（云端生效） */
export type TransferPhase =
  | 'pending'
  | 'accepted'
  | 'migrating'
  | 'verified'
  | 'completed'
  | 'rejected'
  | 'expired'
  | 'failed';

/** 本机侧转移记录（`enclave_transfer_list`） */
export interface TransferView {
  id: string;
  orgId: string;
  /** out = 旧主机转出；in = 新主机接管 */
  direction: 'out' | 'in';
  phase: TransferPhase;
  phaseLabel: string;
  peerDeviceId: string;
  packageHash: string;
  createdAt: number;
  updatedAt: number;
}

export interface TransferExportOutcome {
  transferId: string;
  path: string;
  packageHash: string;
  size: number;
  fileCount: number;
}

export interface TransferImportOutcome {
  transferId: string;
  orgId: string;
  packageHash: string;
  fileCount: number;
}

/** 局域网转移发起结果（`enclave_transfer_lan_offer`） */
export interface LanOfferOutcome {
  transferId: string;
  orgId: string;
  targetUserId: string;
  packageHash: string;
  size: number;
  fileCount: number;
}

// ── M3：任务 ────────────────────────────────────────────────────

/** 任务状态 */
export type TaskStatus = 'pending' | 'in_progress' | 'submitted' | 'done' | 'archived' | 'cancelled';

/** 任务视图（`enclave_task_*`）；`effectiveActions` 是按钮渲染的唯一来源 */
export interface TaskView {
  id: string;
  orgId: string;
  parentTaskId: string | null;
  title: string;
  description: string;
  creatorId: string;
  assigneeId: string | null;
  collaborators: string[];
  status: TaskStatus;
  priority: string;
  dueDate: number | null;
  startedAt: number | null;
  completedAt: number | null;
  /** 前置任务 id（纯 AND 语义） */
  blockedBy: string[];
  visibility: string;
  /** 是否被未完成的前置任务阻塞 */
  blocked: boolean;
  /** 子任务数量：> 0 表示汇总容器（状态由子任务派生） */
  childCount: number;
  effectiveActions: string[];
  createdAt: number;
  updatedAt: number;
}

/** 新建任务入参 */
export interface CreateTaskInput {
  title: string;
  description?: string;
  parentTaskId?: string | null;
  assigneeId?: string | null;
  priority?: string;
  dueDate?: number | null;
  blockedBy?: string[];
}

/** 编辑任务入参（只传要改的字段） */
export interface UpdateTaskInput {
  title?: string;
  description?: string;
  assigneeId?: string | null;
  collaborators?: string[];
  priority?: string;
  dueDate?: number | null;
}

/** 依赖变更预览 */
export interface DepsPreview {
  cycleIds: string[];
  newlyBlockedIds: string[];
  newlyUnblockedIds: string[];
  error: string | null;
}

// ── M4：汇报与文件 ──────────────────────────────────────────────

/** 汇报状态（四态） */
export type ReportStatus = 'draft' | 'submitted' | 'confirmed' | 'returned';

/** 完成汇报（`enclave_report_*`） */
export interface ReportView {
  id: string;
  taskId: string;
  reporterId: string;
  content: string;
  materials: string[];
  deliverables: string[];
  status: ReportStatus;
  confirmedBy: string | null;
  confirmedAt: number | null;
  /** 产出沉淀时间（M5；null = 尚未沉淀） */
  sedimentedAt: number | null;
  sedimentError: string | null;
  createdAt: number;
  updatedAt: number;
}

/** 汇报留言（确认 / 退回记录） */
export interface FeedbackView {
  id: string;
  reportId: string;
  authorId: string;
  action: string;
  content: string;
  createdAt: number;
}

/** 任务附件 */
export interface FileAssetView {
  id: string;
  fileName: string;
  fileType: string;
  fileSize: number;
  fileHash: string;
  uploadedBy: string;
  uploadedAt: number;
  /** 绝对路径 */
  absPath: string;
  /** 挂载到完成汇报时非空 */
  reportId?: string | null;
  /** 挂载到过程记录时非空 */
  progressId?: string | null;
}

/** 本机团队文件根目录（`enclave_files_root_get/set`，全局设置，与具体团队无关） */
export interface TeamFilesRootView {
  /** 用户选择的基础目录 */
  base: string;
  /** 实际文件根：`<基础目录>/enclave/files` */
  root: string;
  custom: boolean;
  available: boolean;
}

// ── M5：组织知识库 ──────────────────────────────────────────────

/** 组织知识库：默认库（系统建、不可删）+ 若干自定义库 */
export interface KbView {
  id: string;
  name: string;
  description: string;
  /** 默认库（系统自动创建、不可删除；任务产出固定沉淀到这里） */
  isImplicit: boolean;
  /** 专属属性定义（JSON 数组字符串，团队库可配置） */
  fieldSchema: string;
  entryCount: number;
  /** 库内交付物文件数（按文件去重） */
  fileCount: number;
  /** 库内最近一次条目更新（0 = 还没有条目） */
  lastEntryAt: number;
  createdAt: number;
  updatedAt: number;
}

/** 团队库加工能力（主机算、成员读缓存） */
export interface TeamIngestCapability {
  canProcess: 'none' | 'local' | 'cloud';
  providerName: string;
  model: string;
  trustDomain: string;
  reason: string;
  /** 待整理条数 */
  pending: number;
}

/** 团队库「待整理」队列的一行 */
export interface EnclaveKbQueueItem {
  id: string;
  orgId: string;
  kbId: string;
  /** snippet / url / file */
  kind: string;
  sourceRef: string;
  payload: string;
  status: 'pending' | 'running' | 'failed';
  attempts: number;
  lastError: string;
  createdAt: number;
  updatedAt: number;
}

export interface TeamIngestOutcome {
  queued: boolean;
  queueId: string;
  entries: number;
}

/** 手工整理第一步的回填内容（提取自原文，未落库） */
export interface TeamIngestPreview {
  title: string;
  content: string;
}

export interface TeamQueueRunOutcome {
  done: number;
  failed: number;
  stopped: boolean;
  remaining: number;
}

/** 团队库「待确认」候选（模型产出，采纳后才进库） */
export interface KbCandidateView {
  id: string;
  orgId: string;
  kbId: string;
  title: string;
  content: string;
  tags: string;
  metadata: string;
  origin: string;
  createdBy: string;
  createdAt: number;
}

/** 团队知识库「文件」页的一行：某条目引用的一个交付物（同一原件可被多条引用） */
export interface KbFileListView {
  fileId: string;
  fileName: string;
  fileType: string;
  fileSize: number;
  fileHash: string;
  /** 所属知识条目 */
  memoryId: string;
  entryTitle: string;
  /** 来源任务 id */
  sourceRef: string;
  /** 来源任务负责人（供「按负责人」分组；空 = 未指派） */
  assigneeId: string;
  /** 来源任务优先级（low / normal / high / urgent） */
  priority: string;
  /** 所属条目的 metadata（含模型写入的专属属性，文件页「按专属属性」分组用） */
  metadata: Record<string, unknown>;
  addedAt: number;
}

/** 无引用文件（条目被移除后留下的孤儿原件） */
export interface KbOrphanFile {
  fileId: string;
  fileName: string;
  fileSize: number;
  createdAt: number;
}

/** 知识条目引用的交付物（同一份任务原件，不重复拷贝） */
export interface KbFileView {
  fileId: string;
  fileName: string;
  fileType: string;
  fileSize: number;
  fileHash: string;
}

/** 组织知识条目 */
export interface KbEntryView {
  memoryId: string;
  /** 所属知识库（「全部库」聚合视图据此标注来源库） */
  kbId: string;
  title: string;
  content: string;
  tags: string;
  /** task_output / manual / ... */
  origin: string;
  /** 来源任务 id */
  sourceRef: string;
  /** 溯源：reportId / taskTitle / assigneeId / completedAt / confirmedBy / confirmedAt / revision / editedBy / editedAt */
  metadata: Record<string, unknown>;
  addedBy: string;
  /** 首次沉淀时间 */
  addedAt: number;
  /** 最近一次覆盖时间 */
  updatedAt: number;
  /** 来源任务已恢复汇报，等待重新确认 */
  stale: boolean;
  /** 该条目自己的交付物 */
  files: KbFileView[];
}

// ── 过程记录 ────────────────────────────────────────────────────

/** 一条进度说明（可多次提交，独立留痕） */
export interface ProgressView {
  id: string;
  taskId: string;
  authorId: string;
  content: string;
  attachments: FileAssetView[];
  createdAt: number;
}

/** 过程记录列表 + 锁定状态（提交完成汇报后锁定） */
export interface ProgressList {
  items: ProgressView[];
  locked: boolean;
  lockReason: string | null;
}
