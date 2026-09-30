/**
 * graphModel — 图谱的「聚合 + 三维布局」层（纯函数，不碰 React / Three.js）
 *
 * **没有"档位"，也没有折叠**（2026-09-28 定）。曾经有过两套折叠设计，都删了：
 *   ① "缩略时按目录/专属属性折成簇" —— 折叠轴根本不可靠：库可能一个专属属性都没有、也可能有若干个；
 *      属性类型不定（数字属性几乎人人不同 ⇒ 折出几千个"簇"，两值布尔 ⇒ 只折出 2 个点）；
 *      标签是多值叠加更不能折（超图：同一文件从多个簇冒出来，折叠时它该回哪个簇没有答案）。
 *   ② "按相机距离分档" —— 缩放驱动换档会让图自己跳变，且每跨一次档都要重建整份 GPU 缓冲。
 * 现在只有一件事决定图上有什么：**哪些文件的分块被展开了**（`expanded`）。
 * 展开与否由**规模**决定（见 KnowledgeGraph 的自动展开），不设任何数量阈值 ——
 * 大部分库都是小规模的，默认就该看得见分块；只有达到一定规模才退回文件级。
 *
 * 规模靠三件事顶住，跟"档位"无关：
 *   ① 节点用 `Points`、边用 `LineSegments`，各一次 draw call；
 *   ② 按需重绘（不脏不提交帧）；
 *   ③ 大库不自动展开分块。
 * `MAX_SCENE_NODES` 是**显存安全阀**（异常数据别把缓冲顶爆），不是体验闸门；触发时显式说明。
 *
 * 布局：目录簇心 → 簇内小球；分块挂在各自文件的旁边。O(n)、零迭代、无随机数 ⇒ 同数据永远同一张图。
 */

import { deriveGraph, chunkNodeId, type GraphModel } from '../../stores/knowledgeStore';
import {
  entryTitle,
  type KnowledgeBase,
  type KnowledgeEdgeKind,
  type KnowledgeEntry,
  type KnowledgeFile,
  type KnowledgeFileRelation,
  type KnowledgeOrigin,
} from '../../types/knowledge';

/** 世界坐标半径：只影响数值量级，渲染层按像素换算，改它不改变观感 */
export const WORLD_R = 100;

/**
 * 显存安全阀：单帧进入渲染的节点上限。
 * **它不是规模闸门**（那是"要不要自动展开分块"的职责），只防一份异常数据把 GPU 缓冲顶爆。
 * 触发时必须在状态栏显式说明。
 */
export const MAX_SCENE_NODES = 60_000;

/** 目录簇心的世界半径倍率（簇内小球半径随成员数按体积增长） */
const LOCAL_R_FACTOR = 0.05;
const LOCAL_R_MIN = 0.02;
const LOCAL_R_MAX = 0.2;

/** 黄金角（≈137.5°）：球内均匀分布的经典做法，天然避免聚簇与规则网格 */
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

/** 投喂内容的固定落盘目录：按路径分组时要跳过它，否则所有文件都会落进同一组 */
const PATH_ROOT = 'files';

/** 「分块 → 别的文件」的跨文件共标签连线：每个分块最多保留几条（与分块数同阶 ⇒ 不爆炸） */
const MAX_CHUNK_LINKS = 3;
/**
 * 只有当标签在**文件层**足够稀有（出现它的文件数 ≤ 这个值）时才拿它配对。
 *
 * 为什么必须加这个闸门：另一侧只能用**文件的标签并集**（几十个，很吵），
 * 直接拿它配对，任何分块都会和几乎所有文件"共享标签"。
 * 稀有标签才是话题，这与 `deriveGraph` 里 `MAX_TAG_GROUP = 8` 是同一个取舍。
 */
const MAX_LINK_TAG_GROUP = 8;

/** 一个可渲染节点 */
export interface SceneNode {
  id: string;
  label: string;
  origin: KnowledgeOrigin;
  /** 世界半径（渲染层按投影换成像素） */
  radius: number;
  /** 供状态栏/详情显示：文件节点 = 登记分块数，其余 1 */
  count: number;
  /** 业务引用：文件 = `file:<relPath>`，分块 = 条目 key */
  refId?: string;
}

export interface SceneEdge {
  /** 索引指向 `nodes`，不是 id —— 十万条边走 Map 查 id 会成为最大的开销 */
  from: number;
  to: number;
  kind: KnowledgeEdgeKind | 'contains';
  weight: number;
}

export interface GraphScene {
  nodes: SceneNode[];
  edges: SceneEdge[];
  /** 3 × nodes.length，直接上传给 GPU */
  positions: Float32Array;
  /**
   * 内容的外接球半径（世界单位）。
   * 渲染层用它算"默认缩放该多大" —— 十几个节点的库不该被摆成画布中心一小撮。
   */
  radius: number;
  shown: number;
  /** 核心（文件/独立条目）节点总数，含被安全阀截掉的 */
  total: number;
  /** 语义边总数（未按开关过滤） */
  rawEdgeCount: number;
}

/* ────────────────────────── 布局 ────────────────────────── */

/**
 * 球内均匀分布的第 i 个点。
 *
 * 为什么要 `cbrt`：体积元 ∝ r²dr，取 `r = R·u^(1/3)` 才让点在**球体内**均匀 ——
 * 直接用 `u` 会让所有点糊在球心附近，看着像一坨。
 */
function ballPoint(i: number, n: number, radius: number, out: [number, number, number]): void {
  const y = 1 - (2 * i + 1) / Math.max(1, n);
  const rho = Math.sqrt(Math.max(0, 1 - y * y));
  const theta = i * GOLDEN_ANGLE;
  const r = radius * Math.cbrt((i + 0.5) / Math.max(1, n));
  out[0] = Math.cos(theta) * rho * r;
  out[1] = y * r;
  out[2] = Math.sin(theta) * rho * r;
}

/**
 * 一组 `ballPoint` 的质心。
 *
 * **必须减掉它**：`ballPoint` 只在 n 大时才以原点为质心 —— n=1 时那个点落在 `0.79R` 上，
 * n=2 时质心落在 `0.77R` 上。不减掉，整个目录就会整体偏离簇心。
 * 实测过的后果：所有文件都在 `files/` 下的库（只有**一个**目录），整张图被推到世界坐标
 * `(79, 0, 0)`，点节点后相机推近，图直接飞出画布。
 */
function centroidOf(n: number, radius: number): [number, number, number] {
  const t: [number, number, number] = [0, 0, 0];
  let cx = 0;
  let cy = 0;
  let cz = 0;
  for (let i = 0; i < n; i++) {
    ballPoint(i, n, radius, t);
    cx += t[0];
    cy += t[1];
    cz += t[2];
  }
  return n > 0 ? [cx / n, cy / n, cz / n] : [0, 0, 0];
}

/** 只在布局里用的分组键：文件所在的目录（去掉固定前缀 `files/`） */
function dirKeyOf(sourceRef: string): string {
  if (!sourceRef) return '';
  const seg = sourceRef.split(/[\\/]/).filter(Boolean);
  const dirs = seg.slice(0, -1);
  if (dirs[0] === PATH_ROOT) dirs.shift();
  return dirs.join('/');
}

/** 目录簇心：均匀铺在球内，并整体平移到以原点为质心 */
function groupCenters(count: number): Float32Array {
  const out = new Float32Array(count * 3);
  const tmp: [number, number, number] = [0, 0, 0];
  for (let i = 0; i < count; i++) {
    ballPoint(i, count, WORLD_R, tmp);
    out[i * 3] = tmp[0];
    out[i * 3 + 1] = tmp[1];
    out[i * 3 + 2] = tmp[2];
  }
  const bias = centroidOf(count, WORLD_R);
  for (let i = 0; i < count; i++) {
    out[i * 3] -= bias[0];
    out[i * 3 + 1] -= bias[1];
    out[i * 3 + 2] -= bias[2];
  }
  return out;
}

/** 簇内小球半径：随成员数按体积增长，并夹在上下限内 */
function localRadius(memberCount: number): number {
  const r = LOCAL_R_FACTOR * Math.cbrt(Math.max(1, memberCount));
  return Math.min(LOCAL_R_MAX, Math.max(LOCAL_R_MIN, r)) * WORLD_R;
}

/** 分块数 → 节点半径：信息量越大的点越显眼，但幅度压得很小 */
function nodeRadiusOf(count: number, base: number): number {
  return base * (1 + Math.min(1.6, Math.log2(1 + count) * 0.28));
}

/* ────────────────────────── 跨文件连线 ────────────────────────── */

/**
 * 「分块 → 别的文件」的共标签连线（常驻）。
 *
 * 判据：**别的文件携带了本分块也有的、且足够稀有的标签**（出现该标签的文件数 ≤ `MAX_LINK_TAG_GROUP`）。
 * 强度 = 命中的稀有标签数；每个分块只留最强的 `MAX_CHUNK_LINKS` 个目标 ⇒ 总边数与分块数同阶。
 *
 * **不能退回"共享 ≥2 个标签"**：分块自己的标签只有 3~6 个具体词，要求跨文件凑齐两个一条都过不了
 * （旧版在文件↔文件上能用 ≥2，是因为文件的标签是各分块的并集、几十个）。
 */
function chunkFileLinks(
  chunks: { index: number; tags: string[]; refFileId: string }[],
  files: { index: number; id: string; tags: string[] }[],
): { from: number; to: number; weight: number }[] {
  if (chunks.length === 0 || files.length === 0) return [];

  const byTag = new Map<string, { index: number; id: string }[]>();
  for (const f of files) {
    for (const t of f.tags) {
      const g = byTag.get(t);
      if (g) g.push({ index: f.index, id: f.id });
      else byTag.set(t, [{ index: f.index, id: f.id }]);
    }
  }
  for (const [t, g] of byTag) if (g.length > MAX_LINK_TAG_GROUP) byTag.delete(t);

  const out: { from: number; to: number; weight: number }[] = [];
  for (const c of chunks) {
    const hits = new Map<number, number>();
    for (const t of c.tags) {
      const g = byTag.get(t);
      if (!g) continue;
      for (const f of g) {
        if (f.id === c.refFileId) continue; // 同文件的兄弟分块：零信息量
        hits.set(f.index, (hits.get(f.index) ?? 0) + 1);
      }
    }
    const top = [...hits.entries()]
      .sort((x, y) => y[1] - x[1] || x[0] - y[0])
      .slice(0, MAX_CHUNK_LINKS);
    for (const [to, weight] of top) out.push({ from: c.index, to, weight });
  }
  return out;
}

/* ────────────────────────── 组场景 ────────────────────────── */

export interface BuildSceneArgs {
  /** `deriveGraph` 的输出（文件节点与语义边） */
  core: GraphModel;
  /**
   * 已展开分块的文件：`文件节点 id` → 它的分块（已过筛选）。
   * 键的集合就是"哪些文件展开了" —— 与 store 的 `graphChunks` 同一口径。
   */
  expanded: Map<string, KnowledgeEntry[]>;
  /** 当前打开的边类型 */
  enabledKinds: (KnowledgeEdgeKind | 'contains')[];
}

export function buildScene(args: BuildSceneArgs): GraphScene {
  const { core, expanded, enabledKinds } = args;
  const edgesOn = (k: SceneEdge['kind']) => enabledKinds.includes(k);

  const kept = core.nodes.slice(0, MAX_SCENE_NODES);
  const truncated = Math.max(0, core.nodes.length - kept.length);

  // 目录分组 → 簇心（只影响位置）
  const groupOf = new Map<string, number>();
  const groupKeys: string[] = [];
  for (const n of kept) {
    const k = dirKeyOf(n.sourceRef);
    if (!groupOf.has(k)) {
      groupOf.set(k, groupKeys.length);
      groupKeys.push(k);
    }
  }
  const memberCounts = new Array<number>(groupKeys.length).fill(0);
  for (const n of kept) memberCounts[groupOf.get(dirKeyOf(n.sourceRef)) as number] += 1;
  const centers = groupCenters(groupKeys.length);
  const seen = new Array<number>(groupKeys.length).fill(0);
  // 组内偏移的质心（同 centroidOf 的理由）：小组不居中，减掉才让成员真正围在簇心四周
  const memberBias = groupKeys.map((_, gi) =>
    centroidOf(memberCounts[gi], localRadius(memberCounts[gi])));

  // 第一遍只定"有哪些节点、谁挂多少分块"：分块的位置依赖各自文件的位置，得先知道总数才好分配缓冲
  const nodes: SceneNode[] = [];
  const indexOfId = new Map<string, number>();
  const fileSeq: number[] = [];
  const chunkHosts: { host: number; list: KnowledgeEntry[] }[] = [];
  let totalChunks = 0;
  for (const n of kept) {
    const gi = groupOf.get(dirKeyOf(n.sourceRef)) as number;
    fileSeq.push(seen[gi]++);
    const i = nodes.length;
    indexOfId.set(n.id, i);
    nodes.push({
      id: n.id,
      label: n.label,
      origin: n.origin,
      // 文件节点的半径体现"登记的分块数"（不是本次加载到的条数）
      radius: n.kind === 'file' ? nodeRadiusOf(n.chunkCount, 1.8) : 1.5,
      count: n.chunkCount,
      refId: n.id,
    });
    const list = expanded.get(n.id);
    if (list && list.length > 0) {
      totalChunks += list.length;
      chunkHosts.push({ host: i, list });
    }
  }

  const positions = new Float32Array((nodes.length + totalChunks) * 3);
  const tmp: [number, number, number] = [0, 0, 0];

  // 文件：落在自己目录的小球里
  kept.forEach((n, k) => {
    const gi = groupOf.get(dirKeyOf(n.sourceRef)) as number;
    ballPoint(fileSeq[k], memberCounts[gi], localRadius(memberCounts[gi]), tmp);
    positions[k * 3] = centers[gi * 3] + tmp[0] - memberBias[gi][0];
    positions[k * 3 + 1] = centers[gi * 3 + 1] + tmp[1] - memberBias[gi][1];
    positions[k * 3 + 2] = centers[gi * 3 + 2] + tmp[2] - memberBias[gi][2];
  });

  const edges: SceneEdge[] = [];
  let raw = 0;
  for (const r of core.relations) {
    const a = indexOfId.get(r.from);
    const b = indexOfId.get(r.to);
    if (a === undefined || b === undefined) continue;
    raw += 1;
    if (!edgesOn(r.kinds[0])) continue;
    edges.push({ from: a, to: b, kind: r.kinds[0], weight: r.weight });
  }

  // 分块：各自挂在所属文件旁边的一个小球里
  const chunkIdx: { index: number; tags: string[]; refFileId: string }[] = [];
  for (const { host, list } of chunkHosts) {
    const hx = positions[host * 3];
    const hy = positions[host * 3 + 1];
    const hz = positions[host * 3 + 2];
    const r = localRadius(list.length) * 0.9;
    const bias = centroidOf(list.length, r);
    list.forEach((e, k) => {
      ballPoint(k, list.length, r, tmp);
      const i = nodes.length;
      nodes.push({
        id: chunkNodeId(e.key),
        label: entryTitle(e.key),
        origin: e.origin,
        radius: 1.3,
        count: 1,
        refId: e.key,
      });
      positions[i * 3] = hx + tmp[0] - bias[0];
      positions[i * 3 + 1] = hy + tmp[1] - bias[1];
      positions[i * 3 + 2] = hz + tmp[2] - bias[2];
      chunkIdx.push({ index: i, tags: e.tags, refFileId: nodes[host].id });
      if (edgesOn('contains')) edges.push({ from: host, to: i, kind: 'contains', weight: 1 });
    });
  }

  // 跨文件连线：分块 → 别的文件（每个分块最多 3 条，见 chunkFileLinks）
  if (edgesOn('shared_tag') && chunkIdx.length > 0) {
    const fileSide = kept
      .map((n, i) => ({ index: i, id: n.id, tags: n.tags }))
      .filter((f) => f.tags.length > 0);
    for (const l of chunkFileLinks(chunkIdx, fileSide)) {
      edges.push({ from: l.from, to: l.to, kind: 'shared_tag', weight: l.weight });
    }
  }

  // 内容的外接球半径：渲染层按它决定"默认缩放多大"（小库不该是画布中心一小撮）
  let radius = 0;
  for (let i = 0; i < nodes.length; i++) {
    const d = Math.hypot(positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]);
    if (d > radius) radius = d;
  }

  return {
    nodes,
    edges,
    positions: positions.subarray(0, nodes.length * 3),
    radius: Math.max(1, radius),
    shown: nodes.length,
    total: core.totalNodes + truncated,
    rawEdgeCount: raw,
  };
}

/** 组件侧一行拿到核心图（把 store 的数据 + 筛选口径 + 安全阀收在这里） */
export function buildCore(
  entries: KnowledgeEntry[],
  base: KnowledgeBase,
  files: KnowledgeFile[],
  fileRelations: KnowledgeFileRelation[],
  filtersActive: boolean,
): GraphModel {
  return deriveGraph(entries, base, files, fileRelations, filtersActive, MAX_SCENE_NODES);
}
