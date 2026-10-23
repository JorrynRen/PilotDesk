/**
 * 图谱边数防护：语义边（共标签 / 同专属属性）每节点 Top-K + 全局预算。
 *
 * 为什么单独成文件：`knowledgeStore` 与 `graphModel` 互相引用（前者提供 deriveGraph，
 * 后者提供布局与 MAX_SCENE_NODES），共享逻辑放在任何一侧都会形成环。
 *
 * 为什么需要：组内两两连边本身是 C(k,2)，组上限 8 已把单组压到 28 条；
 * 但"每条知识带十几个标签"时总边数仍会随标签/属性总数线性涨到十万级。
 * 这里把语义边压到 O(节点数 × MAX_EDGES_PER_NODE)，先丢弱关系。
 */

import type { KnowledgeRelation } from '../../types/knowledge';

/** 每个节点最多保留的语义边数（按权重从高到低） */
export const MAX_EDGES_PER_NODE = 10;
/** 全局边预算：再大的库也不让边数无限增长 */
export const MAX_TOTAL_EDGES = 30_000;

/**
 * 结构边（附件 / 版本替代）只受全局预算约束，不参与每节点上限 ——
 * 它们表达"这份知识有哪些文件"，丢了语义就不完整；语义边才是可裁的弱关系。
 */
function isStructural(r: KnowledgeRelation): boolean {
  return r.kinds.includes('attachment') || r.kinds.includes('supersedes');
}

export function capRelations(relations: KnowledgeRelation[]): KnowledgeRelation[] {
  const sorted = [...relations].sort((a, b) => b.weight - a.weight);
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
