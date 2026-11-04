// 群聊（多 Agent 群聊）类型定义，与后端 groupchat/models.rs 字段对齐（camelCase）。

export type RoomStatus = 'idle' | 'running' | 'paused' | 'finished' | 'aborted';

export interface GroupChatRoom {
  id: string;
  title: string;
  topic: string;
  status: RoomStatus;
  strategy: string;
  /**
   * 讨论轮次**参考预算**（DB `max_rounds`）。**不是硬上限**：引擎不据此中断讨论，
   * 只是给"计划讨论多少轮"一个参考刻度。由主持人在任务编排时按复杂度估算给出，
   * 讨论中也可调整；`0`/负 = 不设预算（前端不显示参考刻度）。
   */
  maxRounds: number;
  maxParallel: number;
  directorId?: string;
  currentTaskId?: string;
  createdAt: number;
  updatedAt: number;
  goalNotes?: string;
  allowAutoCli?: number;
  /** 房间统一产物目录（绝对路径；空=运行时回退 <工作目录>/outputs/<房间标题>/） */
  outputDir?: string;
  /**
   * 以下三个是**派生字段**：后端只在房间列表（`list_rooms`）里读时聚合，房间详情/创建路径不返回。
   *   currentRound  当前**讨论段**（最近一条用户指令之后）的轮次——与运行时口径一致，
   *                 不是全历史最大值（每条新指令会把轮次归零）
   *   taskTotal     子任务总数（执行阶段进度的分母）
   *   taskFinished  已结束的子任务数（含失败/跳过/中止）
   * 取不到（undefined）时前端退化成"只显示状态与更新时间"，不编进度。
   */
  currentRound?: number;
  taskTotal?: number;
  taskFinished?: number;
}

export type ParticipantType = 'api' | 'cli' | 'user' | 'director';

export interface GroupChatParticipant {
  id: string;
  roomId: string;
  participantType: ParticipantType;
  agentConfig: string; // JSON 字符串：api:{provider,model} / cli:{agent_type}
  displayName: string;
  systemRole: string;
  status: string;
}

export interface GroupChatMessage {
  id: string;
  roomId: string;
  round: number;
  seq: number;
  sender: string;
  recipients: string; // JSON 数组
  kind: string;
  replyTo?: string;
  content: string;
  /** 附件元数据（图片/文件，JSON 数组字符串，结构复用 Attachment） */
  attachments: string;
  /** 参与者本轮工具调用链（reasoning/tool_start/tool_result 步骤，JSON 数组字符串，可溯源） */
  toolCalls: string;
  /** 思考链文本（DeepSeek 等 reasoning_content；主持人决策消息与参与者一致，前端无差异展示） */
  reasoningContent?: string;
  /** 附加结构化数据（如用户确认请求，JSON 对象字符串），普通消息为 "{}" */
  extra: string;
  timestamp: number;
}

/** 圆桌阶段消息的结构化条目（extra.stage 载荷的一部分）。 */
export interface GroupChatStageRosterItem {
  participant: string;
  role?: string;
  reason?: string;
}

export interface GroupChatStageRoleItem {
  participant: string;
  role?: string;
  status?: string;
  reason?: string;
}

export interface GroupChatStageTaskItem {
  no: number;
  description: string;
  assignee?: string | null;
  dependsOn?: number[];
}

export interface GroupChatStageRemovedItem {
  no: number;
  reason?: string;
}

export interface GroupChatStageReassignedItem {
  no: number;
  from?: string | null;
  to?: string;
}

export interface GroupChatToolDecisionItem {
  tool: string;
  risk?: string;
  allow?: boolean;
  reason?: string;
}

/**
 * 圆桌阶段消息载荷（消息 extra JSON）：`stage` 决定前端的标题与图标，
 * 其余字段按需携带表格数据；旧消息无此载荷时按正文前缀降级渲染。
 */
export interface GroupChatStageExtra {
  stage: string;
  goal?: string;
  digest?: string[];
  questions?: string[];
  notice?: string;
  items?: GroupChatToolDecisionItem[];
  roster?: GroupChatStageRosterItem[];
  rosterRejected?: string[];
  roles?: GroupChatStageRoleItem[];
  tasks?: GroupChatStageTaskItem[];
  removed?: GroupChatStageRemovedItem[];
  reassigned?: GroupChatStageReassignedItem[];
  speaker?: string;
  speakerReason?: string;
  workContent?: string | null;
  taskId?: string;
  taskNo?: number;
  status?: string;
  description?: string;
  body?: string;
}

/** 结构化确认项（Director 生成，前端按 inputType 渲染对应控件）。 */
export interface GroupChatConfirmationItem {
  id: string;
  label: string;
  inputType: 'select' | 'confirm' | 'text';
  options: string[];
  required: boolean;
  placeholder?: string;
}

/** 主持人向用户发起的确认请求（Director 裁决 ask_user 时产生）。 */
export interface GroupChatConfirmationRequest {
  requestId: string;
  taskId: string;
  /** 确认事项摘要标题（≤22 字，后端 LLM 生成；缺省时前端兜底取 prompt 首行）。 */
  title?: string;
  prompt: string;
  replyMode: 'open' | 'structured';
  items: GroupChatConfirmationItem[];
}

/** 用户对结构化确认请求的单条回复。 */
export interface GroupChatConfirmationResponseInput {
  itemId: string;
  value: string;
}

export interface GroupChatStance {
  roomId: string;
  participantId: string;
  stance: string;
  /** 立场态度：agree / disagree / neutral（后端 LLM 预处理给出，失败时文本分类兜底） */
  attitude: string;
  updatedAt: number;
}

/** 参与者工具调用实时状态（复用会话模式 agent-tool-start / agent-tool-result）。 */
export interface GroupChatToolCall {
  toolId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done';
  result?: string;
  success?: boolean;
}

export type TaskStatus = 'discussing' | 'pending' | 'running' | 'success' | 'failed' | 'skipped' | 'aborted';

export interface GroupChatTask {
  id: string;
  roomId: string;
  taskNo: number;
  description: string;
  assignee?: string;
  dependsOn: string; // JSON 数组
  status: TaskStatus;
  resultSummary?: string;
  error?: string;
  startedAt?: number;
  completedAt?: number;
}

/** 手动新增子任务入参（dependsOn 传任务 id，下标换算由后端完成） */
export interface GroupChatAddTaskInput {
  roomId: string;
  description: string;
  dependsOn: string[];
  assignee?: string | null;
}

/** 依赖变更预览：确认卡据此说明"改完会阻塞/解锁谁、是否成环"（纯计算，不落库） */
export interface GroupChatDepChangePreview {
  /** 变更后陷入依赖环的任务编号（会被判死锁失败，提交时后端会直接拒绝） */
  cycleTaskNos: number[];
  /** 变更后由等待变为就绪的任务编号 */
  unblockedTaskNos: number[];
  /** 变更后由就绪变为等待的任务编号 */
  blockedTaskNos: number[];
}

export interface GroupChatParticipantInput {
  id: string;
  participantType: ParticipantType;
  agentConfig: string;
  displayName: string;
  systemRole: string;
}

export interface CreateGroupChatRoomInput {
  title: string;
  topic: string;
  participants: GroupChatParticipantInput[];
  directorId: string;
  /** 是否允许主持人自动补人时添加 CLI 参与者（1=允许，0=禁止） */
  allowAutoCli: number;
  /** 房间统一产物目录（绝对路径；缺省空=运行时回退 <工作目录>/outputs/<房间标题>/） */
  outputDir?: string;
  /** 同层任务执行并行上限（1-4，缺省 2；仅影响执行阶段，讨论仍按顺序） */
  maxParallel?: number;
}

// 事件 payload（对应后端 groupchat-event 单通道）
export type GroupChatEvent =
  | { roomId: string; type: 'started' }
  | { roomId: string; type: 'room_status'; status: RoomStatus }
  | { roomId: string; type: 'floor_granted'; speaker: string; round: number }
  | { roomId: string; type: 'token_stream'; speaker: string; delta: string }
  | { roomId: string; type: 'message'; message: GroupChatMessage }
  | { roomId: string; type: 'stance_updated'; participantId: string; stance: string; attitude: string }
  | { roomId: string; type: 'role_updated'; participantId: string; role: string }
  | { roomId: string; type: 'task_updated'; task: GroupChatTask }
  | { roomId: string; type: 'participant_updated' }
  | { roomId: string; type: 'output_dir_updated'; outputDir: string }
  | { roomId: string; type: 'finished' };
