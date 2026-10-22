/**
 * 团队知识库图谱：把主机返回的知识条目派生成图谱核心数据（GraphModel）。
 *
 * 与个人库的区别：团队条目没有"文件分块"这一层，节点口径是
 * **知识条目 + 文件（交付物 / 来源文件）**；边只有「共标签」与「附件」。
 * 渲染层复用个人库的 `KnowledgeGraph`（通过其 `source` 入口传入本文件的结果）。
 */

import type { KbEntryView } from '../../types/enclave';
import type {
  KnowledgeEntry,
  KnowledgeGraphNode,
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
    meta: {},
    origin: mapOrigin(e.origin),
    sourceRef: '',
    baseIds: [],
  };
}

export function buildTeamGraph(entries: KbEntryView[]): GraphModel {
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
      meta: {},
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

  return { nodes: kept, relations: [...map.values()], totalNodes };
}
