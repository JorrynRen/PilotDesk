/**
 * 团队知识库图谱：把主机返回的知识条目派生成图谱核心数据（GraphModel）。
 *
 * 与个人库的区别：团队条目没有"文件分块"这一层，节点口径是
 * **知识条目 + 文件（交付物 / 来源文件）**；边有「共标签」「附件」「同专属属性」。
 * 渲染层复用个人库的 `KnowledgeGraph`（通过其 `source` 入口传入本文件的结果）。
 */

import type { KbEntryView } from '../../types/enclave';
import type {
  KnowledgeEntry,
  KnowledgeGraphNode,
  KnowledgeMeta,
  KnowledgeOrigin,
  KnowledgeRelation,
} from '../../types/knowledge';
import type { GraphModel } from '../../stores/knowledgeStore';
import { MAX_SCENE_NODES } from '../knowledge/graphModel';

const ENTRY_PREFIX = 'entry:';
const FILE_PREFIX = 'file:';
/** 单个标签的节点数超过这个值就不连边（人人都有它，连出来只是一团废线） */
const MAX_TAG_GROUP = 8;
/**
 * 单个属性值的条目数上限：超过就不连「同专属属性」边。
 * 属性比标签更能说明关系（同客户 / 同项目），但"状态=进行中"这类值也会覆盖大半张图。
 */
const MAX_FIELD_GROUP = 8;
/** 每个节点最多保留的语义边数：超出后按权重丢弃弱边（总边数上界 ≈ 节点数 × 这个值） */
const MAX_EDGES_PER_NODE = 10;
/** 全局边预算：再大的库也不让边数无限增长 */
const MAX_TOTAL_EDGES = 30_000;

/**
 * 边数防护：结构边只受全局预算约束，语义边（共标签 / 同专属属性）再受每节点上限。
 *
 * 组上限（8）只保证"单组"不爆炸；标签 / 属性多的库仍会线性涨到十万级。
 * 这里把语义边压到 O(节点数 × MAX_EDGES_PER_NODE)，先丢弱关系；
 * 附件这类结构边表达"知识有哪些文件"，不参与每节点上限，只受全局预算兜底。
 */
function capRelations(relations: KnowledgeRelation[]): KnowledgeRelation[] {
  const sorted = [...relations].sort((a, b) => b.weight - a.weight);
  const isStructural = (r: KnowledgeRelation) =>
    r.kinds.includes('attachment') || r.kinds.includes('supersedes');
  const out: KnowledgeRelation[] = [];

  // 1) 结构边优先占用预算
  for (const r of sorted) {
    if (out.length >= MAX_TOTAL_EDGES) break;
    if (isStructural(r)) out.push(r);
  }
  // 2) 语义边：每个节点最多 K 条
  const degree = new Map<string, number>();
  for (const r of sorted) {
    if (out.length >= MAX_TOTAL_EDGES) break;
    if (isStructural(r)) continue;
    const da = degree.get(r.from) ?? 0;
    const db = degree.get(r.to) ?? 0;
    if (da >= MAX_EDGES_PER_NODE || db >= MAX_EDGES_PER_NODE) continue;
    out.push(r);
    degree.set(r.from, da + 1);
    degree.set(r.to, db + 1);
  }
  return out;
}

/**
 * 团队来源 → 个人库的来源枚举。
 *
 * 仅用于节点配色与详情里的来源文案，不参与任何业务判定：
 *   task_output → work（「工作中沉淀」）
 *   file / url / ai 原样
 *   manual（手工入库）→ snippet
 */
function mapOrigin(origin: string): KnowledgeOrigin {
  switch (origin) {
    case 'task_output':
      return 'work';
    case 'file':
      return 'file';
    case 'url':
      return 'url';
    case 'ai':
      return 'ai';
    default:
      return 'snippet';
  }
}

/**
 * 历史数据兼容：早期「手工入库」把 origin 写成 `'manual'`（只可能出现在 snippet / file / url
 * 三种待整理项里），它不属于 5 种来源，会凭空多出第六种；图谱里还会落到默认分支被当成
 * 「片段投喂」上色（文件/网址知识看起来像片段）。
 *
 * 读时按内容还原真实来源：网址快照 → url；有原件 → file；其余 → snippet。
 * 新写入已直接写真实来源（见 rpc.rs 的 kb.ingest_manual）。
 */
export function normalizeOrigin(e: KbEntryView): string {
  if (e.origin !== 'manual') return e.origin;
  if (e.sourceRef.startsWith('http')) return 'url';
  return e.files.length > 0 ? 'file' : 'snippet';
}

/** 标签串 → 去重数组（与页面里的 splitTags 同口径） */
function splitTags(raw: string): string[] {
  return [...new Set(
    raw.split(/[,，;；\s]+/).map((t) => t.trim()).filter(Boolean),
  )];
}

/** 团队条目 → 个人库图谱认识的条目形状（只保住图谱用得到的字段） */
export function toKnowledgeEntry(e: KbEntryView): KnowledgeEntry {
  return {
    key: e.memoryId,
    value: e.content,
    tags: splitTags(e.tags),
    pin: false,
    accessCount: 0,
    createdAt: e.addedAt,
    updatedAt: e.updatedAt,
    meta: toMeta(e.metadata),
    origin: mapOrigin(normalizeOrigin(e)),
    sourceRef: '',
    baseIds: [],
  };
}

/** 专属属性只取标量：metadata 里的溯源信息（reportId 等）不按属性渲染 */
function toMeta(raw: Record<string, unknown>): KnowledgeMeta {
  const out: KnowledgeMeta = {};
  for (const [k, v] of Object.entries(raw)) {
    if (typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') out[k] = v;
  }
  return out;
}

/** 节点级属性：图谱详情按字符串渲染 */
function toNodeMeta(raw: Record<string, unknown>): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const [k, v] of Object.entries(raw)) {
    if (typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') {
      out[k] = String(v);
    }
  }
  return out;
}

export function buildTeamGraph(
  entries: KbEntryView[],
  /** 当前库的专属属性键：只有这些键参与「同专属属性」连边 */
  fieldKeys: string[] = [],
): GraphModel {
  const nodes: KnowledgeGraphNode[] = [];
  /** 文件 → 关联的条目（同一份原件可能被多个条目引用） */
  const files = new Map<string, { name: string; entries: KnowledgeEntry[] }>();

  for (const raw of entries) {
    const entry = toKnowledgeEntry(raw);
    nodes.push({
      id: `${ENTRY_PREFIX}${raw.memoryId}`,
      label: raw.title || '(无标题)',
      kind: 'entry',
      origin: entry.origin,
      sourceRef: '',
      chunkCount: 1,
      entries: [entry],
      tags: entry.tags,
      meta: toNodeMeta(raw.metadata),
      pin: false,
    });
    for (const f of raw.files) {
      const rec = files.get(f.fileId) ?? { name: f.fileName, entries: [] };
      rec.entries.push(entry);
      files.set(f.fileId, rec);
    }
  }

  for (const [fileId, rec] of files) {
    nodes.push({
      id: `${FILE_PREFIX}${fileId}`,
      label: rec.name,
      kind: 'file',
      origin: rec.entries[0]?.origin ?? 'file',
      // 团队文件没有"分块展开"这层，sourceRef 留空，渲染层就不会去个人库拉分块
      sourceRef: '',
      chunkCount: rec.entries.length,
      entries: rec.entries,
      tags: [...new Set(rec.entries.flatMap((e) => e.tags))],
      meta: {},
      pin: false,
    });
  }

  // 规模封顶：**与个人库同一个显存安全阀**（不是体验闸门）；图例会显示「已显示 N / 共 M」
  const totalNodes = nodes.length;
  const kept = totalNodes > MAX_SCENE_NODES ? nodes.slice(0, MAX_SCENE_NODES) : nodes;
  const keptIds = new Set(kept.map((n) => n.id));

  const map = new Map<string, KnowledgeRelation>();
  const add = (a: string, b: string, kind: KnowledgeRelation['kinds'][number], weight: number) => {
    if (a === b || !keptIds.has(a) || !keptIds.has(b)) return;
    const [from, to] = a < b ? [a, b] : [b, a];
    const key = `${from}\u0000${to}`;
    const cur = map.get(key);
    if (cur) {
      if (!cur.kinds.includes(kind)) cur.kinds.push(kind);
      cur.weight += weight;
    } else {
      map.set(key, { from, to, kinds: [kind], weight });
    }
  };

  // 附件关系：条目 ↔ 文件
  for (const [fileId, rec] of files) {
    const fileNode = `${FILE_PREFIX}${fileId}`;
    for (const e of rec.entries) add(`${ENTRY_PREFIX}${e.key}`, fileNode, 'attachment', 1.5);
  }

  // 共标签（共享 ≥2 个才算）：倒排索引 + 组内配对，组过大的标签跳过
  const tagGroups = new Map<string, KnowledgeGraphNode[]>();
  for (const n of kept) {
    for (const t of n.tags) {
      const g = tagGroups.get(t) ?? [];
      g.push(n);
      tagGroups.set(t, g);
    }
  }
  const sharedCount = new Map<string, number>();
  for (const g of tagGroups.values()) {
    if (g.length < 2 || g.length > MAX_TAG_GROUP) continue;
    for (let i = 0; i < g.length; i += 1) {
      for (let j = i + 1; j < g.length; j += 1) {
        const a = g[i].id;
        const b = g[j].id;
        const key = a < b ? `${a}\u0000${b}` : `${b}\u0000${a}`;
        sharedCount.set(key, (sharedCount.get(key) ?? 0) + 1);
      }
    }
  }
  for (const [key, shared] of sharedCount) {
    if (shared < 2) continue;
    const sep = key.indexOf('\u0000');
    add(key.slice(0, sep), key.slice(sep + 1), 'shared_tag', shared * 0.5);
  }

  // 同专属属性：同一字段同值即连（一个字段就够，不像标签要求 ≥2）；
  // 值过宽的组（如"状态=进行中"）跳过，否则整张图会被一条线串成链
  if (fieldKeys.length > 0) {
    const fieldCount = new Map<string, number>();
    for (const key of fieldKeys) {
      const groups = new Map<string, KnowledgeGraphNode[]>();
      for (const n of kept) {
        if (n.kind !== 'entry') continue;
        const v = n.meta[key];
        if (v === null || v === undefined || String(v).trim() === '') continue;
        const value = String(v);
        const g = groups.get(value) ?? [];
        g.push(n);
        groups.set(value, g);
      }
      for (const g of groups.values()) {
        if (g.length < 2 || g.length > MAX_FIELD_GROUP) continue;
        for (let i = 0; i < g.length; i += 1) {
          for (let j = i + 1; j < g.length; j += 1) {
            const a = g[i].id;
            const b = g[j].id;
            const pair = a < b ? `${a}\u0000${b}` : `${b}\u0000${a}`;
            fieldCount.set(pair, (fieldCount.get(pair) ?? 0) + 1);
          }
        }
      }
    }
    for (const [pair, shared] of fieldCount) {
      const sep = pair.indexOf('\u0000');
      add(pair.slice(0, sep), pair.slice(sep + 1), 'same_field', shared * 0.8);
    }
  }

  const relations = [...map.values()];
  return {
    nodes: kept,
    relations: capRelations(relations),
    totalNodes,
    totalRelations: relations.length,
  };
}
