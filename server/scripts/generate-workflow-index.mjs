#!/usr/bin/env node
/**
 * 生成工作流模板索引 `server/market/workflow/workflow-index.json`
 *
 * 为什么需要索引：模板目录是 `workflow/<模板名>/`，而静态托管**没有目录列表**，
 * 客户端没法"问服务器有哪些模板"。所以必须有一个索引文件把清单固化下来 ——
 * 这与插件市场的 `plugins-index.json` 是同一个道理（那个由 GitHub Actions 生成）。
 *
 * 只读不写业务数据：本脚本不碰模板 JSON 内容，只提取浏览/搜索需要的元信息。
 *
 * 索引条目 = 两类字段的合并：
 * - **派生字段**：直接从模板 JSON 提取（名称/版本/描述/阶段数/节点数/触发方式/节点类型），
 *   模板一改、重跑即更新，不需要人工维护。
 * - **人工字段**：分类/标签/作者/最低版本/安装要求，以及「精选」清单，
 *   来自同目录的 `catalog.json`。索引每次重算都会把这些合并进来，
 *   所以 **catalog.json 是唯一需要手工编辑的地方**，索引随时可以重生、不丢信息。
 *
 * catalog.json 结构（扁平 map，键 = 模板目录名；保留键 `featured` 除外）：
 * {
 *   "featured": ["自动短篇小说写作工作流"],   // 精选清单：市场默认 tab 按它排序展示
 *   "某模板目录名": {
 *     "category": "creative",               // 白名单 automation/agent/data/devops/creative，非法或缺失 → other
 *     "tags": ["小说", "写作"],              // 最多 8 个、每个最多 32 字，超限只截断+告警
 *     "author": "PilotDesk 官方",
 *     "minAppVersion": "0.4.0",             // 安装最低版本要求（空 = 不限制）
 *     "requirements": ["需要配置可用的 LLM 连接"] // 安装前置要求（自由文案，详情里逐条展示）
 *   }
 * }
 * 条目里没有模板、或 featured 指向了不存在的模板：都会告警，但不阻断生成。
 *
 * 刻意不放进索引的字段（设计取舍，勿"补回来"）：
 * - 下载量/收藏数：静态市场没有统计源，派生不出来，也没有服务器可查。
 * - 难度/预计耗时：人工评的主观值，没有稳定口径，不如不展示。
 * - 文件大小：对"装不装"的决策没有帮助，省掉一次 stat。
 * 这些字段待自建服务器后再由服务端数据库提供，索引保持"只描述模板本身"。
 *
 * ⚠️ 索引里的模板条目**刻意不带"这条什么时候更新的"时间戳**（只有顶层一个 updatedAt）。
 * 曾经用 `git log -1 --format=%cI -- <path>` 取"最后一次改动该目录的提交时间"，
 * 但那个值在提交时刻必然算不对：索引必须与模板文件在**同一次提交**里，于是本地生成时
 * 读到的是**上一个**提交的时间，CI 事后重算必然不同 → 每改一次模板就多一个纯时间戳提交。
 * 这是结构性不可稳定，不是实现没写好。条目级"更新时间"该由 git 历史回答，
 * 不要往派生物里塞；真要展示"更新于"，在 UI 侧按需查 git，或让作者在模板里自己写。
 *
 * 用法：node server/scripts/generate-workflow-index.mjs
 */

import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url)); // <repo>/server/scripts
const ROOT = path.resolve(HERE, '..'); // <repo>/server
const WORKFLOW_DIR = path.join(ROOT, 'market', 'workflow');
const OUT_FILE = path.join(WORKFLOW_DIR, 'workflow-index.json');
const CATALOG_FILE = path.join(WORKFLOW_DIR, 'catalog.json');

/** 主定义文件的命名约定：`[主]xxx.json`（其余 `[子]xxx.json` 是它引用的子工作流） */
const MAIN_PREFIX = '[主]';

/** catalog 顶层保留键：精选清单（值 = 模板目录名数组） */
const FEATURED_KEY = 'featured';

/** 分类白名单。非法值一律归入 other —— 前端筛选按这组枚举渲染，脏值会造出"幽灵分类" */
const CATEGORIES = ['automation', 'agent', 'data', 'devops', 'creative'];
const DEFAULT_CATEGORY = 'other';

/** 标签上限（与灵感市场的 normalizeTags 同款规则：只告警、不放弃整条） */
const MAX_TAGS = 8;
const MAX_TAG_LEN = 32;

/**
 * 写入索引，并让顶层 `updatedAt` 只在**内容真的变了**时才更新。
 *
 * 直接用 `new Date()` 的问题：只要 workflow 被触发，生成结果就与已提交的不同，
 * `git diff` 恒非空 → 每次触发都追一个只含时间戳的提交，历史全是噪音，
 * 真改动反而被淹没。改成"内容变了才盖新时间戳、否则原样沿用"后，产出对相同输入
 * 逐字节稳定，只有真改动才会产生提交。
 *
 * 语义也随之更准：它是"索引内容最后一次变化的时间"，而非"最后一次跑脚本的时间"。
 * 比较时排除 `updatedAt` 本身，其余字段逐字比对 —— 条目里没有时间戳（理由见文件头），
 * 所以"其余字段"全部是内容派生的、可复现的。
 */
async function writeIndexStable(outFile, index) {
  let prev = null;
  try {
    prev = JSON.parse(await fs.readFile(outFile, 'utf8'));
  } catch {
    /* 首次生成 / 旧文件损坏：当作全新 */
  }

  const strip = ({ updatedAt, ...rest }) => rest;
  const unchanged = prev !== null && JSON.stringify(strip(prev)) === JSON.stringify(strip(index));

  const finalIndex = { ...index, updatedAt: unchanged ? prev.updatedAt : index.updatedAt };
  await fs.writeFile(outFile, JSON.stringify(finalIndex, null, 2) + '\n', 'utf8');
  return unchanged;
}

/** 节点总数：跨全部阶段累加（模板是 stages[].nodes[] 的结构） */
function countNodes(def) {
  if (!Array.isArray(def?.stages)) return 0;
  return def.stages.reduce((n, s) => n + (Array.isArray(s?.nodes) ? s.nodes.length : 0), 0);
}

/**
 * 节点类型列表（去重、保持首现顺序）。
 *
 * 详情弹窗用它渲染"这个模板由哪些节点组成"的胶囊，让用户**不进编辑器**就能判断模板是否
 * 用得上（有没有 subflow、有没有 plugin）；也省掉前端为看结构去下载模板 JSON。
 */
function collectNodeTypes(def) {
  if (!Array.isArray(def?.stages)) return [];
  const types = [];
  for (const stage of def.stages) {
    if (!Array.isArray(stage?.nodes)) continue;
    for (const node of stage.nodes) {
      const t = typeof node?.type === 'string' ? node.type.trim() : '';
      if (t && !types.includes(t)) types.push(t);
    }
  }
  return types;
}

/**
 * 归一化标签：只留非空字符串，去首尾空白、超长截断、去重、限制条数。
 *
 * 与 id/name 的处理刻意不同：**标签问题只告警、不跳过整条模板** ——
 * 为一个写错的标签把模板拦在市场外，代价远大于收益。被丢弃/裁剪的每一处都逐条报出，
 * 不静默改数据。
 */
function normalizeTags(raw, label, warnings) {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw)) {
    warnings.push(`${label}（tags 不是数组，已忽略）`);
    return [];
  }
  const out = [];
  for (const item of raw) {
    if (typeof item !== 'string') {
      warnings.push(`${label}（tags 里有非字符串项，已忽略）`);
      continue;
    }
    let t = item.trim();
    if (!t) {
      warnings.push(`${label}（tags 里有空白项，已忽略）`);
      continue;
    }
    if (t.length > MAX_TAG_LEN) {
      warnings.push(`${label}（标签「${t}」超过 ${MAX_TAG_LEN} 字，已截断）`);
      t = t.slice(0, MAX_TAG_LEN);
    }
    if (out.includes(t)) {
      warnings.push(`${label}（标签「${t}」重复，已去重）`);
      continue;
    }
    out.push(t);
  }
  if (out.length > MAX_TAGS) {
    warnings.push(`${label}（标签 ${out.length} 个超过上限 ${MAX_TAGS}，只取前 ${MAX_TAGS} 个）`);
    return out.slice(0, MAX_TAGS);
  }
  return out;
}

/** 归一化安装前置要求：只留非空字符串（文案由作者自由写，不做条数限制） */
function normalizeRequirements(raw, label, warnings) {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw)) {
    warnings.push(`${label}（requirements 不是数组，已忽略）`);
    return [];
  }
  const out = [];
  for (const item of raw) {
    if (typeof item !== 'string' || !item.trim()) {
      warnings.push(`${label}（requirements 里有非字符串或空白项，已忽略）`);
      continue;
    }
    out.push(item.trim());
  }
  return out;
}

/**
 * 读取 catalog.json：模板目录名 → 人工维护的展示元数据 + 精选清单。
 *
 * - 文件缺失：只告警、返回空（新仓库还没建 catalog 也能生成索引）
 * - 文件存在但不是合法 JSON / 顶层不是对象：直接报错退出 —— 这是手工维护的文件，
 *   写坏了必须立刻停下，不能默默生成一份"没有任何分类"的索引上线
 */
async function readCatalog(warnings) {
  const rel = path.relative(ROOT, CATALOG_FILE);
  let raw;
  try {
    raw = await fs.readFile(CATALOG_FILE, 'utf8');
  } catch {
    warnings.push(`未找到 ${rel}（全部模板将使用默认元数据）`);
    return { entries: {}, featured: [] };
  }

  let data;
  try {
    data = JSON.parse(raw);
  } catch (e) {
    console.error(`${rel} 不是合法 JSON：${e.message}`);
    process.exit(1);
  }
  if (data === null || typeof data !== 'object' || Array.isArray(data)) {
    console.error(`${rel} 的顶层必须是对象（键 = 模板目录名）`);
    process.exit(1);
  }

  const { [FEATURED_KEY]: featuredRaw, ...entries } = data;
  const featured = [];
  if (featuredRaw !== undefined) {
    if (!Array.isArray(featuredRaw)) {
      warnings.push(`${rel}（${FEATURED_KEY} 不是数组，已忽略）`);
    } else {
      for (const id of featuredRaw) {
        if (typeof id !== 'string' || !id.trim()) {
          warnings.push(`${rel}（${FEATURED_KEY} 里有非字符串或空白项，已忽略）`);
          continue;
        }
        featured.push(id.trim());
      }
    }
  }
  return { entries, featured };
}

async function main() {
  const warnings = [];
  const { entries: catalog, featured } = await readCatalog(warnings);
  const featuredSet = new Set(featured);

  let dirs;
  try {
    dirs = (await fs.readdir(WORKFLOW_DIR, { withFileTypes: true }))
      .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
      .map((e) => e.name);
  } catch {
    console.error(`找不到模板目录：${WORKFLOW_DIR}`);
    process.exit(1);
  }

  const templates = [];
  const skipped = [];

  for (const dir of dirs) {
    const absDir = path.join(WORKFLOW_DIR, dir);
    const files = (await fs.readdir(absDir)).filter((f) => f.toLowerCase().endsWith('.json'));
    const mainFile = files.find((f) => f.startsWith(MAIN_PREFIX));

    // 没有主定义就没法作为模板装载：**明确报出来**，不静默跳过
    if (!mainFile) {
      skipped.push(`${dir}（缺少以「${MAIN_PREFIX}」开头的主定义文件）`);
      continue;
    }

    let def;
    try {
      def = JSON.parse(await fs.readFile(path.join(absDir, mainFile), 'utf8'));
    } catch (e) {
      skipped.push(`${dir}（主定义不是合法 JSON：${e.message}）`);
      continue;
    }

    // ── 人工字段：来自 catalog.json。没有条目 = 全部默认值，保持索引条目形状稳定 ──
    const meta = catalog[dir];
    let m = {};
    if (meta !== undefined) {
      if (meta === null || typeof meta !== 'object' || Array.isArray(meta)) {
        warnings.push(`${dir}（catalog 条目不是对象，已按默认元数据处理）`);
      } else {
        m = meta;
      }
    }

    // 分类：白名单之外（含条目缺字段）一律归入 other —— 枚举稳定比"原样保留"重要。
    // 完全没进 catalog 的模板只走默认值，不告警（覆盖顺序由作者安排，不强制全量登记）
    let category = typeof m.category === 'string' ? m.category.trim() : '';
    if (!CATEGORIES.includes(category)) {
      if (category) {
        warnings.push(`${dir}（category「${category}」不在白名单内，已归入 ${DEFAULT_CATEGORY}）`);
      } else if (meta !== undefined) {
        warnings.push(`${dir}（catalog 条目缺少 category，已归入 ${DEFAULT_CATEGORY}）`);
      }
      category = DEFAULT_CATEGORY;
    }

    const tags = normalizeTags(m.tags, dir, warnings);

    let author = typeof m.author === 'string' ? m.author.trim() : '';
    if (m.author !== undefined && typeof m.author !== 'string') {
      warnings.push(`${dir}（author 不是字符串，已忽略）`);
      author = '';
    }

    let minAppVersion = typeof m.minAppVersion === 'string' ? m.minAppVersion.trim() : '';
    if (m.minAppVersion !== undefined && typeof m.minAppVersion !== 'string') {
      warnings.push(`${dir}（minAppVersion 不是字符串，已忽略）`);
      minAppVersion = '';
    }

    const requirements = normalizeRequirements(m.requirements, dir, warnings);

    const trigger = def?.trigger?.triggerType;

    templates.push({
      id: dir,
      name: typeof def.name === 'string' && def.name ? def.name : dir,
      version: typeof def.version === 'string' ? def.version : '',
      description: typeof def.description === 'string' ? def.description : '',
      // 已 URL 编码的路径：模板名带中文与括号，让调用方各自编码迟早出错，这里一次编好
      path: `workflow/${encodeURIComponent(dir)}/${encodeURIComponent(mainFile)}`,
      dir,
      mainFile,
      subFiles: files.filter((f) => f !== mainFile),
      stageCount: Array.isArray(def.stages) ? def.stages.length : 0,
      nodeCount: countNodes(def),
      // 触发方式取值 manual/cron/event（前端负责映射成中文）；缺失写空串，形状稳定
      triggerType: typeof trigger === 'string' ? trigger.trim() : '',
      nodeTypes: collectNodeTypes(def),
      // 精选只出现在市场侧清单里：条目上仅打标，不打标的条目不带这个字段
      ...(featuredSet.has(dir) ? { featured: true } : {}),
      category,
      tags,
      author,
      minAppVersion,
      requirements,
    });
  }

  // catalog 指向了不存在的模板目录 / featured 指向了装不了的模板名：都明确报出来，
  // 多半是模板改目录名后忘了同步 catalog（孤儿条目本身无害，但必须让人看到）
  const dirNames = new Set(dirs);
  for (const key of Object.keys(catalog)) {
    if (!dirNames.has(key)) {
      warnings.push(`${path.relative(ROOT, CATALOG_FILE)}（条目「${key}」没有对应的模板目录）`);
    }
  }
  const templateIds = new Set(templates.map((t) => t.id));
  for (const id of featured) {
    if (!templateIds.has(id)) {
      warnings.push(`${path.relative(ROOT, CATALOG_FILE)}（精选里的「${id}」不是可装载的模板）`);
    }
  }

  templates.sort((a, b) => a.name.localeCompare(b.name, 'zh'));

  const index = {
    schemaVersion: '1.0',
    updatedAt: new Date().toISOString(),
    templates,
  };
  await writeIndexStable(OUT_FILE, index);

  console.log(`已生成 ${path.relative(ROOT, OUT_FILE)}：${templates.length} 个模板`);
  for (const t of templates) {
    console.log(
      `  · ${t.name} v${t.version} — ${t.stageCount} 阶段 / ${t.nodeCount} 节点 / ${t.category}` +
        `${t.featured ? ' / 精选' : ''}`,
    );
  }
  if (skipped.length > 0) {
    console.warn(`\n跳过 ${skipped.length} 个目录：`);
    for (const s of skipped) console.warn(`  ! ${s}`);
  }
  if (warnings.length > 0) {
    console.warn(`\n${warnings.length} 条元数据提示：`);
    for (const w of warnings) console.warn(`  ! ${w}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});