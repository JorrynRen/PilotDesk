#!/usr/bin/env node
/**
 * 生成工作流模板索引 `server/market/workflow/workflow-index.json`
 *
 * 为什么需要索引：模板目录是 `workflow/<工作流 UUID>/`（目录名 = 包内工作流 UUID），而静态托管**没有目录列表**，
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
 * catalog.json 结构（扁平 map，键 = 模板目录名 = 工作流 UUID；保留键 `featured` 除外）：
 * {
 *   "featured": ["cd74a4ef-99c0-4481-a6cd-a5f42e8ae7d5"],   // 精选清单：值 = 工作流 UUID（= 目录名），市场默认 tab 按它排序展示
 *   "某工作流 UUID": {
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

/**
 * 工作流模板目录统一为**工作流包**格式：包根 `manifest.json` 是**唯一入口**，
 * 主/子工作流一视同仁平铺在 `workflows/` 子目录下（文件名=工作流原名，重名加序号）。
 * 主文件取 `manifest.json` 里 `mainId` 命中的成员（其语义仅为「导入后默认打开哪一个」），
 * 其余成员即子工作流；引用一律由各 JSON 内的稳定 ID（Subflow 节点的 `params.definitionId`）
 * 表达，文件名不参与解析。目录缺少 `manifest.json` 即视为不可装载，明确报出目录名、不产出错误索引。
 *
 * 市场条目键 = **包内工作流 UUID**（`manifest.mainId`，即主定义在库内的稳定 ID）：
 * 索引条目的 `id`、以及模板目录名都取它，保证 `id == 目录名` 自洽（也避开中文名 / 重名带来的
 * 目录冲突）。`name` 只是工作流名称，**允许重复**，本脚本不做任何唯一性校验（同名不同 UUID
 * 是两条不同条目）。文件路径与清单里的 `file` 仍用原名，用不到 UUID。
 */

/** 工作流包清单文件名（模板目录根下） */
const MANIFEST_FILE = 'manifest.json';

/** 新文件包的格式标识（`manifest.json` 的 `format` 字段） */
const WORKFLOW_PACKAGE_FORMAT = 'pilotdesk.workflow.package';

/** catalog 顶层保留键：精选清单（值 = 工作流 UUID 数组，即模板目录名） */
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
 * 读取 catalog.json：模板目录名（= 工作流 UUID）→ 人工维护的展示元数据 + 精选清单。
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
    console.error(`${rel} 的顶层必须是对象（键 = 工作流 UUID，即模板目录名）`);
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

/**
 * 递归收集目录下所有 `.json` 文件的**相对路径**（相对 `absDir`，POSIX 风格）。
 *
 * 工作流包的工作流文件在 `workflows/` 子目录下，清单里的 `file` 是相对包根的相对路径
 * （如 `workflows/名称.json`），故这里要按整棵树收集、以便与清单声明逐一比对。
 * 以 `.` 开头的目录（如停用区 `.takedown`）跳过。
 */
async function collectJsonFiles(absDir) {
  const out = [];
  async function walk(dir, prefix) {
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const e of entries) {
      if (e.name.startsWith('.')) continue;
      const rel = prefix ? `${prefix}/${e.name}` : e.name;
      if (e.isDirectory()) {
        await walk(path.join(dir, e.name), rel);
      } else if (e.name.toLowerCase().endsWith('.json')) {
        out.push(rel);
      }
    }
  }
  await walk(absDir, '');
  return out;
}

/** 对相对路径逐段编码（保留 `/` 分隔符）：`workflows/名称.json` → `workflows/%E5%90%8D...json`。 */
function encodeRelPath(rel) {
  return rel
    .split('/')
    .filter((s) => s.length > 0)
    .map(encodeURIComponent)
    .join('/');
}

/**
 * 解析工作流包的 `manifest.json`（包格式，见客户端 `commands/workflow.rs`）。
 *
 * 返回两态：
 * - `{ mainId, mainName, mainFile, subFiles }`：合法清单 —— 主文件取 `id === mainId` 的成员
 *   （`mainId` 唯一表达入口），`mainId` 即市场条目键（工作流 UUID），`mainName` 是清单里的名称
 *   （仅作 JSON 缺 name 时的显示兜底）；`subFiles` = 其余成员文件。清单只声明**工作流成员**文件，
 *   故 subFiles 天然不含 `manifest.json`。
 * - `{ error }`：清单不可用（非法 JSON / 非本格式 / 缺主成员 / 声明文件缺失）→ 调用方跳过该目录并报出目录名。
 */
async function readWorkflowPackage(absDir, files) {
  let manifest;
  try {
    manifest = JSON.parse(await fs.readFile(path.join(absDir, MANIFEST_FILE), 'utf8'));
  } catch (e) {
    return { error: `${MANIFEST_FILE} 不是合法 JSON：${e.message}` };
  }
  if (manifest === null || typeof manifest !== 'object' || Array.isArray(manifest)) {
    return { error: `${MANIFEST_FILE} 的顶层必须是对象` };
  }
  if (manifest.format !== WORKFLOW_PACKAGE_FORMAT) {
    return { error: `${MANIFEST_FILE} 的 format 不是 ${WORKFLOW_PACKAGE_FORMAT}` };
  }
  if (!Array.isArray(manifest.workflows)) {
    return { error: `${MANIFEST_FILE} 的 workflows 不是数组` };
  }

  const members = manifest.workflows
    .filter(
      (w) => w !== null && typeof w === 'object' && typeof w.file === 'string' && w.file.trim(),
    )
    .map((w) => ({
      id: typeof w.id === 'string' ? w.id : '',
      name: typeof w.name === 'string' ? w.name : '',
      file: w.file.trim(),
    }));
  // 入口只由 mainId 表达（不再有 isMain 字段）
  const main = members.find((w) => w.id === manifest.mainId);
  if (!main) {
    return { error: `${MANIFEST_FILE} 未声明主工作流（mainId 未匹配任何成员）` };
  }
  const missing = members.map((w) => w.file).filter((f) => !files.includes(f));
  if (missing.length > 0) {
    return { error: `${MANIFEST_FILE} 声明的成员文件缺失：${missing.join('、')}` };
  }
  return {
    // 市场条目键 = 主工作流 UUID（= main.id，二者按定义相等）
    mainId: main.id,
    mainName: main.name,
    mainFile: main.file,
    subFiles: members.map((w) => w.file).filter((f) => f !== main.file),
  };
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
    const files = await collectJsonFiles(absDir);

    // 模板目录统一为工作流包：只按 manifest.json 取主文件 / 子文件。
    // subFiles 只含工作流成员文件、**不含 manifest.json**（清单由 manifestFile 字段单独给出）。
    if (!files.includes(MANIFEST_FILE)) {
      // 缺清单就没法确定主文件与成员：**明确报出目录名**，不静默产出错误索引
      skipped.push(`${dir}（缺少 ${MANIFEST_FILE}）`);
      continue;
    }
    const pkg = await readWorkflowPackage(absDir, files);
    if (pkg.error) {
      skipped.push(`${dir}（${pkg.error}）`);
      continue;
    }
    const { mainId, mainName, mainFile, subFiles } = pkg;
    const manifestFile = MANIFEST_FILE;

    // 目录名约定 = 工作流 UUID（= mainId），保证索引 `id == 目录名`。
    // 不一致时只告警、仍按**实际目录名**拼 path（否则下载地址会指向不存在的目录），
    // 提示作者把目录改名成该 UUID。
    if (dir !== mainId) {
      warnings.push(
        `${dir}（目录名不是工作流 UUID（mainId=${mainId}）：索引 id 取 UUID，而文件路径按目录名拼，` +
          '两者不一致会让人困惑，建议把目录改名为该 UUID）',
      );
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
      // 条目键 = 包内工作流 UUID（manifest.mainId）；目录名同 UUID，故 id == dir（自洽）
      id: mainId,
      name: typeof def.name === 'string' && def.name ? def.name : mainName || dir,
      version: typeof def.version === 'string' ? def.version : '',
      description: typeof def.description === 'string' ? def.description : '',
      // 已 URL 编码的路径：模板名带中文与括号，让调用方各自编码迟早出错，这里一次编好
      // （mainFile 是相对包根的相对路径，如 workflows/名称.json，逐段编码、保留 `/`）
      path: `workflow/${encodeURIComponent(dir)}/${encodeRelPath(mainFile)}`,
      dir,
      mainFile,
      subFiles,
      // 清单文件名（客户端据此把 manifest.json 一并下载）
      manifestFile,
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