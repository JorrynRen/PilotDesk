/**
 * 知识库领域类型（前端）
 *
 * 与后端一一对应（`src-tauri/src/api_agent/knowledge.rs`）：
 *   knowledge_bases   → KnowledgeBase      （一行一个库，只放定义：名称/描述/专属字段）
 *   kb_entry_links    → KnowledgeEntry.meta（多对多；专属属性值挂在关联上）
 *   key_memories      → KnowledgeEntry      （category='knowledge'，通用属性复用记忆表）
 *   kb_files          → KnowledgeFile       （原文共享一份，不按库拷贝）
 *   kb_candidates     → KnowledgeCandidate  （待确认队列）
 *
 * 后端视图里 JSON 是字符串（fieldsJson / metaJson）、tags 是逗号分隔字符串，
 * 这里统一在 `toXxx()` 里转成前端好用的形状，组件只面对转换后的类型。
 */

/** 专属属性字段类型 */
export type KnowledgeFieldType = 'text' | 'number' | 'boolean' | 'select' | 'date';

/** 专属属性字段定义：由用户为每个知识库自定义 */
export interface KnowledgeFieldDef {
  /** 存储键（短英文，落在关联行的 kb_meta JSON 里） */
  key: string;
  /** 展示名 */
  label: string;
  type: KnowledgeFieldType;
  /** select 的候选项 */
  options?: string[];
  required?: boolean;
}

/** 知识库定义：用户只维护「名称 / 描述 / 专属字段」三样 */
export interface KnowledgeBase {
  id: string;
  name: string;
  description: string;
  fields: KnowledgeFieldDef[];
  entryCount: number;
  pinnedCount: number;
  fileCount: number;
  pendingCount: number;
  createdAt: number;
  updatedAt: number;
}

/**
 * 知识来源（= 内容怎么进来的，共 6 种。工作是自动的，其余 5 种由用户发起）
 *   work    工作中沉淀：会话里模型显式写入 / 后台抽取的候选，用户不用主动操作
 *   snippet 片段投喂：粘贴一段文本
 *   file    文件知识：拖入或选择文件
 *   url     网页投喂：贴一个网址
 *   cloud   云文档投喂：从云文档平台（如语雀）拉取文档列表，勾选后取正文
 *   ai      AI 生成：给主题/要求，由模型产出知识
 */
export type KnowledgeOrigin = 'work' | 'snippet' | 'file' | 'url' | 'cloud' | 'ai';

export const ORIGIN_LABEL: Record<KnowledgeOrigin, string> = {
  work: '工作中沉淀',
  snippet: '片段投喂',
  file: '文件知识',
  url: '网页投喂',
  cloud: '云文档投喂',
  ai: 'AI 生成',
};

/**
 * 来源配色（单一来源，图谱与列表共用）。
 * 必须是 `Record<KnowledgeOrigin, string>`：新增来源时靠类型把漏配色的地方全部报出来 ——
 * 图谱曾漏了 `ai`，`fill={undefined}` 会退化成 SVG 默认黑块，类型能挡住这类漂移。
 */
export const ORIGIN_COLOR: Record<KnowledgeOrigin, string> = {
  work: '#8B5CF6',
  snippet: '#3B82F6',
  file: '#10B981',
  url: '#F59E0B',
  cloud: '#06B6D4',
  ai: '#EC4899',
};

/** 六种来源的说明（给用户看的一张表：谁触发、内容从哪来） */
export const ORIGIN_DESC: Record<KnowledgeOrigin, { trigger: string; note: string }> = {
  work: { trigger: '自动', note: '会话里沉淀下来的结论，由模型写入或整理成候选，无需你操作' },
  snippet: { trigger: '你发起', note: '粘贴一段草稿：由 AI 整理成规范知识（分节、去重复），原始片段保留在候选里可对照' },
  file: { trigger: '你发起', note: '拖入或选择文件，原文复制进库；正文由 AI 决定怎么分组与起标题（原文一字不改）' },
  url: { trigger: '你发起', note: '贴一个网址：本地去脚本后由 AI 清洗正文（丢掉导航/页脚）并分组，落盘为 .md' },
  cloud: { trigger: '你发起', note: '从云文档平台（如语雀）拉取文档列表，勾选后取正文入库；只投正文，图片与附件不在范围内' },
  ai: { trigger: '你发起', note: '提一个主题 / 要求，由模型生成知识草稿' },
};

const ORIGINS: string[] = ['work', 'snippet', 'file', 'url', 'cloud', 'ai'];

/** 专属属性值（按字段类型存原生值） */
export type KnowledgeMeta = Record<string, string | number | boolean>;

/**
 * 知识条目：key_memories 的一行 + 本库的专属属性值。
 * 同一 key 可同时属于多个知识库 —— 通用属性（tags/pin/热度）共享，专属属性按库各存一份。
 */
export interface KnowledgeEntry {
  key: string;
  value: string;
  tags: string[];
  pin: boolean;
  accessCount: number;
  createdAt: number;
  updatedAt: number;
  /** 当前知识库的专属属性值 */
  meta: KnowledgeMeta;
  origin: KnowledgeOrigin;
  sourceRef: string;
  /** 文件分块序号（1 起；用于自动生成「分块相邻」边） */
  chunkIndex?: number;
  chunkTotal?: number;
  /** 该条目关联的全部知识库（>1 时前端显示「N 库」） */
  baseIds: string[];
}

/**
 * 分块 key → 给人看的标题。
 *
 * key 的形态是 `<标题>-<sha8>#<序号>`（见后端 `write_planned_chunks`）：`-<sha8>` 是**去重指纹**、
 * `#序号` 是分块次序。指纹是内部标识，直接显示出来就是"标题后面拖着一串乱码"；
 * 序号另有 `chunkIndex` 字段，界面上单独显示成「第 n/m 块」更清楚。
 * **只在显示时剥掉**：key 是主键，所有读写操作一律仍用原 key。
 */
export function entryTitle(key: string): string {
  return key.replace(/-[0-9a-f]{8}#\d+$/, '');
}

/**
 * 原文相对路径 → 给人看的文件名（`files/d803b634-名字.md` → `名字.md`）。
 * 与 `entryTitle` 同一个道理：`<sha8>-` 前缀是去重用的，`kb_files.name` 里存的是干净名字，
 * 而条目只带 `sourceRef`，所以在显示层剥掉。
 */
export function sourceFileName(ref: string): string {
  const base = ref.split(/[\\/]/).pop() ?? ref;
  return base.replace(/^[0-9a-f]{8}-/, '');
}

/**
 * 文件的**专属属性**——由该文件所有分块的属性聚合而来（`kb_files` 上没有属性列，
 * 属性只挂在条目关联行上，所以"这个文件属于哪个部门"只能由分块聚合得到）。
 *
 * 给的是**全部去重取值**（不是"一个值"），文件树靠它表达三种情况：
 *   0 个取值 = 分块都没填 → 这个文件**没有子目录**（留在根目录）；
 *   1 个取值 = 进那一个目录；
 *   N 个取值 = **同时出现在 N 个目录**（虚拟重复：是同一份文件，不是 N 份 ——
 * 勾选 / 导出 / 从库中移除一律按文件 id 走，不会重复处理）。
 */
export interface KnowledgeFileAttr {
  /** 原文相对路径（与 `KnowledgeFile.path` 同一口径） */
  sourceRef: string;
  /** 字段 → 该文件全部去重取值（已按展示口径转成文本，布尔是「是/否」；已排序） */
  values: Record<string, string[]>;
  /** 参与聚合的分块数（0 = 没有分块，必然没有属性） */
  chunkCount: number;
}

/** 文件知识：原文只存一份，多库共享 */
export interface KnowledgeFile {
  id: string;
  name: string;
  /** 相对知识库文件根的路径（网络来源时为空） */
  path: string;
  /** 绝对路径（「打开原文」直接用；由后端用知识库根拼好） */
  absPath: string;
  url: string;
  mime: string;
  size: number;
  /** unsupported = 原文已登记，但该格式暂不支持抽取正文（内容不进检索） */
  extractStatus: 'pending' | 'extracting' | 'done' | 'unsupported' | 'failed';
  chunkCount: number;
  /** 分块是否已过 AI 整理（打标 + 填专属属性） */
  enriched: boolean;
  /** 整理失败的原因（成功时为空） */
  enrichError: string;
  /** 上次**成功**整理的时间（0 = 从没整理过）：AI 主导维护下，用户至少要能知道属性何时被刷过 */
  enrichedAt: number;
  baseIds: string[];
  createdAt: number;
}

/**
 * 图谱边类型：全部自动派生。
 *
 * 注意这里**没有**「同源」和「分块相邻」：图谱的节点粒度是**文件**（同一个文件的全部
 * 分块折叠成一个节点），所以"同源"是节点的自明属性，不必连边。
 * 早期版本以分块为节点、把同源组连成全连通团，一个 400 块的文件就会产生近 8 万条边
 * （O(n²)），既画不出信息量也会把前端拖死 —— 那正是改成文件节点的原因。
 */
export type KnowledgeEdgeKind = 'same_field' | 'shared_tag' | 'attachment' | 'supersedes';

export const EDGE_LABEL: Record<KnowledgeEdgeKind, string> = {
  same_field: '同专属属性',
  shared_tag: '共标签',
  attachment: '附件关系',
  supersedes: '版本替代',
};

export const EDGE_COLOR: Record<KnowledgeEdgeKind, string> = {
  same_field: '#10B981',
  shared_tag: '#F59E0B',
  attachment: '#3B82F6',
  supersedes: '#8B5CF6',
};

/** 结构边「分块归属」= 文件 → 它自己的分块。与四类派生关系并列，但**可以开关** */
export const CONTAINS_LABEL = '分块归属';

/**
 * 结构边的颜色：**玫红**。
 *
 * 选它只看一件事 —— 与四类语义边在色相上拉得开。现有四色是绿 160° / 琥珀 38° /
 * 蓝 217° / 紫 258°，最大的空档是 258°→38° 那一段（140°），玫红正落在里面
 * （距紫 92°、距琥珀 48°）。反例：青色/青绿虽然"看起来没被占用"，但离绿与蓝都只有 28° 上下，
 * 而图例色条只有 2px 高、画布上的线只有 1px —— 那个距离在实际尺寸下分不出来。
 *
 * 曾用过 `--text-tertiary` 与石板灰 `#64748B`，两次都被反馈"还是灰的"：
 * 结构边最多有几百条、又确实表达不了"发现了什么关系"，但它仍然需要**是一个颜色** ——
 * 否则图例里就没有它的位置，用户找不到开关。
 */
export const CONTAINS_COLOR = '#F43F5E';

/** 图谱节点：文件（由该文件的全部条目折叠而成）、独立条目，或展开出来的片段 */
export interface KnowledgeGraphNode {
  /** `file:<relPath>` / `entry:<key>` / `chunk:<entryKey>` */
  id: string;
  label: string;
  kind: 'file' | 'entry' | 'chunk';
  origin: KnowledgeOrigin;
  /** 文件节点 = 原文相对路径；独立条目 / 片段节点 = '' */
  sourceRef: string;
  /**
   * 该文件的**登记**分块数（`kb_files.chunk_count` 是权威值），独立条目节点为 1。
   *
   * 刻意与 `entries.length` 分开：条目列表按库一次最多 500 条，`entries` 很可能只是
   * 这个文件的一个子集。用 `entries.length` 当"分块数"会在库变大后悄悄变小。
   */
  chunkCount: number;
  /** 文件节点附带的原文记录（详情面板用）；原文行已不在库里时为空 */
  file?: KnowledgeFile;
  /**
   * 片段节点（层 1）所属文件节点的 id；核心节点为空。
   * 分层定位据此把片段摆在父节点外侧的弧上。
   */
  parentId?: string;
  /** 折叠进来的条目：文件节点 = 该文件的全部（已加载）分块；独立条目 / 片段 = 自身 */
  entries: KnowledgeEntry[];
  /** 聚合标签（文件内各块标签的并集） */
  tags: string[];
  /** 聚合专属属性：块间取值一致时给该值，`null` 表示各块不一致（多值） */
  meta: Record<string, string | null>;
  pin: boolean;
}

/** 图谱边：一对节点合并为一条线，kinds 记录全部命中类型 */
export interface KnowledgeRelation {
  from: string;
  to: string;
  kinds: KnowledgeEdgeKind[];
  weight: number;
}

/** 待确认候选：app 自动整理的结果，采纳后才写入知识库 */
export interface KnowledgeCandidate {
  id: number;
  baseId: string;
  key: string;
  value: string;
  meta: KnowledgeMeta;
  /** 整理/生成给出的检索标签（采纳后写进条目的通用属性） */
  tags: string[];
  origin: KnowledgeOrigin;
  sourceRef: string;
  /** 为什么建议入库（哪次会话 / 哪个文件 / 哪条链接） */
  reason: string;
  createdAt: number;
}

/* ────────────────────────── 后端视图 → 前端类型 ────────────────────────── */

export interface KnowledgeBaseView {
  id: string;
  name: string;
  description: string;
  fieldsJson: string;
  entryCount: number;
  pinnedCount: number;
  fileCount: number;
  pendingCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface KnowledgeEntryView {
  key: string;
  value: string;
  category: string;
  tags: string;
  pin: boolean;
  accessCount: number;
  createdAt: number;
  updatedAt: number;
  metaJson: string;
  origin: string;
  sourceRef: string;
  chunkIndex: number | null;
  chunkTotal: number | null;
  baseIds: string[];
}

export interface KnowledgeFileView {
  id: string;
  name: string;
  relPath: string;
  absPath: string;
  url: string;
  mime: string;
  size: number;
  extractStatus: string;
  chunkCount: number;
  enriched: boolean;
  enrichError: string;
  enrichedAt: number;
  baseIds: string[];
  createdAt: number;
}

export interface KnowledgeCandidateView {
  id: number;
  baseId: string;
  key: string;
  value: string;
  metaJson: string;
  tags: string;
  origin: string;
  sourceRef: string;
  reason: string;
  createdAt: number;
}

/** 来源为空/未知时按「片段投喂」兜底（手动建条目不填来源） */
function toOrigin(raw: string): KnowledgeOrigin {
  return ORIGINS.includes(raw) ? (raw as KnowledgeOrigin) : 'snippet';
}

function parseObject(raw: string): KnowledgeMeta {
  try {
    const v = JSON.parse(raw);
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as KnowledgeMeta) : {};
  } catch {
    return {};
  }
}

export function toBase(v: KnowledgeBaseView): KnowledgeBase {
  let fields: KnowledgeFieldDef[] = [];
  try {
    const parsed = JSON.parse(v.fieldsJson);
    if (Array.isArray(parsed)) fields = parsed as KnowledgeFieldDef[];
  } catch {
    fields = [];
  }
  return { ...v, fields };
}

export function toEntry(v: KnowledgeEntryView): KnowledgeEntry {
  return {
    key: v.key,
    value: v.value,
    tags: v.tags ? v.tags.split(',').filter(Boolean) : [],
    pin: v.pin,
    accessCount: v.accessCount,
    createdAt: v.createdAt,
    updatedAt: v.updatedAt,
    meta: parseObject(v.metaJson),
    origin: toOrigin(v.origin),
    sourceRef: v.sourceRef,
    chunkIndex: v.chunkIndex ?? undefined,
    chunkTotal: v.chunkTotal ?? undefined,
    baseIds: v.baseIds,
  };
}

export function toFile(v: KnowledgeFileView): KnowledgeFile {
  return {
    id: v.id,
    name: v.name,
    path: v.relPath,
    absPath: v.absPath,
    url: v.url,
    mime: v.mime,
    size: v.size,
    extractStatus: (['pending', 'extracting', 'done', 'unsupported', 'failed'].includes(v.extractStatus)
      ? v.extractStatus
      : 'pending') as KnowledgeFile['extractStatus'],
    chunkCount: v.chunkCount,
    enriched: v.enriched,
    enrichError: v.enrichError,
    enrichedAt: v.enrichedAt ?? 0,
    baseIds: v.baseIds,
    createdAt: v.createdAt,
  };
}

/** 投喂结果（文件 / 网页 / 云文档共用，对应后端 IngestOutcome） */
export interface IngestOutcome {
  fileId: string;
  fileName: string;
  relPath: string;
  chunks: number;
  /** 原文已在库里（同 sha256）：只补了关联，没有重复落盘 */
  reused: boolean;
  status: string;
  truncated: boolean;
  /** AI 分组未生效的原因（生效时为空串）—— 投喂主路径就是 AI 分块，失败必须让用户看见 */
  aiNote: string;
}

/* ────────────────────────── 云文档源（第一期：语雀） ──────────────────────────
 * 一个"账号" = 一个平台 + 一套凭证；可以配多个，投喂时按 id 指定用哪一个。 */

/** 云文档平台标识。加飞书时在这里加一项，并在投喂面板的来源下拉里补上 */
export type CloudSourceId = 'yuque';

export const CLOUD_SOURCE_LABEL: Record<CloudSourceId, string> = {
  yuque: '语雀',
};

/** 平台标识 → 展示名（认不出的标识原样显示，避免"配了却看不见"） */
export function cloudSourceLabel(source: string): string {
  return CLOUD_SOURCE_LABEL[source as CloudSourceId] ?? source;
}

/**
 * 一个已配置的云文档账号（**不含 Token**，后端从不下发明文）。
 *
 * 账号是复数：可以同时配多个 —— 同一平台的两个账号（个人 / 团队）、将来的多个平台。
 * 投喂时按 `id` 指定用哪一个，所以界面上必须让用户看得见、选得到。
 */
export interface CloudAccount {
  id: string;
  /** 平台标识（`yuque`），展示时配 `cloudSourceLabel` 用 */
  source: string;
  /** 账号名（用户填的，或添加时用平台账号名兜底） */
  label: string;
}

/** 云文档平台上的一个知识库 */
export interface CloudRepo {
  /** 命名空间（`login/slug`）：列文档、取正文都用它 */
  namespace: string;
  name: string;
  description: string;
  /** 所属团队（个人知识库为空） */
  groupName: string;
  docCount: number;
  updatedAt: string;
}

/** 知识库里的一篇文档（只有元信息，正文在投喂时逐篇取） */
export interface CloudDoc {
  id: string;
  title: string;
  wordCount: number;
  updatedAt: string;
}

/**
 * 文件之间的关系（公文类：正文—附件；制度类：新版—旧版）。
 * 方向约定与后端一致：attachment 的 from 是正文 / to 是附件，supersedes 的 from 是新版 / to 是旧版。
 * **只标注、不替换**：新旧文件与条目都保留、都参与检索（查历史版本是真实诉求）。
 */
export type KnowledgeFileRelationKind = 'attachment' | 'supersedes';

export const FILE_RELATION_LABEL: Record<KnowledgeFileRelationKind, string> = {
  attachment: '附件',
  supersedes: '版本替代',
};

export interface KnowledgeFileRelation {
  fromFileId: string;
  toFileId: string;
  kind: KnowledgeFileRelationKind;
}

export interface KnowledgeFileRelationView {
  fromFileId: string;
  toFileId: string;
  kind: string;
}

export function toFileRelation(v: KnowledgeFileRelationView): KnowledgeFileRelation {
  return {
    fromFileId: v.fromFileId,
    toFileId: v.toFileId,
    kind: (v.kind === 'supersedes' ? 'supersedes' : 'attachment') as KnowledgeFileRelationKind,
  };
}

export function toCandidate(v: KnowledgeCandidateView): KnowledgeCandidate {
  return {
    id: v.id,
    baseId: v.baseId,
    key: v.key,
    value: v.value,
    meta: parseObject(v.metaJson),
    tags: v.tags ? v.tags.split(',').filter(Boolean) : [],
    origin: toOrigin(v.origin),
    sourceRef: v.sourceRef,
    reason: v.reason,
    createdAt: v.createdAt,
  };
}
