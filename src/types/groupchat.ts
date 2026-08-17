// 群聊（多 Agent 群聊）类型定义，与后端 groupchat/models.rs 字段对齐（camelCase）。

export type RoomStatus = 'idle' | 'running' | 'paused' | 'finished' | 'aborted';

export interface GroupChatRoom {
  id: string;
  title: string;
  topic: string;
  status: RoomStatus;
  strategy: string;
  maxRounds: number;
  maxParallel: number;
  directorId?: string;
  currentTaskId?: string;
  createdAt: number;
  updatedAt: number;
  goalNotes?: string;
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
  /** 附加结构化数据（如用户确认请求，JSON 对象字符串），普通消息为 "{}" */
  extra: string;
  timestamp: number;
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

export type TaskStatus = 'discussing' | 'pending' | 'running' | 'success' | 'failed' | 'skipped';

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
}

// 事件 payload（对应后端 groupchat-event 单通道）
export type GroupChatEvent =
  | { roomId: string; type: 'started' }
  | { roomId: string; type: 'room_status'; status: RoomStatus }
  | { roomId: string; type: 'floor_granted'; speaker: string; round: number }
  | { roomId: string; type: 'token_stream'; speaker: string; delta: string }
  | { roomId: string; type: 'message'; message: GroupChatMessage }
  | { roomId: string; type: 'stance_updated'; participantId: string; stance: string }
  | { roomId: string; type: 'role_updated'; participantId: string; role: string }
  | { roomId: string; type: 'task_updated'; task: GroupChatTask }
  | { roomId: string; type: 'finished' };
