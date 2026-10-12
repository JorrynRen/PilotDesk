#!/usr/bin/env node
/**
 * 生成灵感市场索引 `server/market/inspirations/index.json`
 *
 * 为什么需要索引：静态托管没有目录列表，客户端没法"问服务器有哪些灵感"。
 * 一条灵感一个**目录**（约定 `inspirations/<id>/<id>.json`），索引负责把清单固化下来 ——
 * 与插件市场的 `plugins-index.json`、模板市场的 `workflow-index.json` 同一个道理。
 *
 * 目录结构（与工作流模板、插件同构）的收益有两个：
 *   ① 平台上「停用 / 恢复」可以整目录搬迁（三类内容的处理方式因此完全一致）；
 *   ② 同目录留出了放附件的位置（图标、示例文件等），不必再挤进正文 JSON。
 * 旧的扁平文件 `inspirations/<id>.json` **不再收录**，会明确报出来而不是静默忽略。
 *
 * 索引**只放浏览/搜索需要的元信息**（含正文摘要与标签），正文全文留在源文件里按需拉取。
 * 这样正文只有一份真相，且与另外两个市场同构；代价是搜索覆盖到摘要为止（摘要 200 字，
 * 基本涵盖提示词的场景说明部分），详情多一次请求。
 *
 * 源文件格式（`inspirations/<id>/<id>.json`）：
 *   { id, title, icon, content, tags? }
 * 其中 `tags` 可选，是字符串数组。它的问题只提示、不跳过整条 —— 标签是可选元数据，
 * 为一个写错的标签把整条灵感拦在市场外，代价远大于收益（见 normalizeTags）。
 *
 * ⚠️ 索引里的条目**刻意不带"这条什么时候更新的"时间戳**（只有顶层一个 updatedAt）。
 * 曾经用 `git log -1 --format=%cI -- <path>` 取"最后一次改动该文件的提交时间"，
 * 但那个值在提交时刻必然算不对：索引必须与源文件在**同一次提交**里，于是本地生成时
 * 读到的是**上一个**提交的时间，CI 事后重算必然不同 → 每改一次内容就多一个纯时间戳提交。
 * 这是结构性不可稳定，不是实现没写好。条目级"更新时间"该由 git 历史回答，
 * 不要往派生物里塞；真要展示"更新于"，在 UI 侧按需查 git，或让作者在源文件里自己写。
 *
 * 身份约定：**`id` 是权威键**，且必须**等于其目录名**（布局固定为 `<id>/<id>.json`），
 * 并满足 `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$`（首尾都必须是字母/数字，短横线只能在中间）、
 * 长度 ≤ 64。任何一条不满足 → **直接报错退出**（让 CI 挂掉），
 * 绝不静默跳过；id 撞车同样直接报错。
 *
 * ⚠️ **`id` 一旦发布即不可更改**：它是客户端判断"这条是否已导入过"的依据，
 * 改 id 等于**新增一条**——已导入旧 id 的用户会重复导入一份，且旧条目不会被新内容覆盖。
 * 要改名请新开一个 id（新目录），不要原地改 id。
 *
 * 每条额外输出 **`contentHash`**：对 `{id,title,icon,content,tags}` 做稳定序列化
 * （对象键按字典序、数组保持原序）后取 sha256 前 16 位十六进制。用途：客户端据此判断
 * "市场内容是否变过"——哈希不变即可跳过重新拉取；它与 id 正交（id 不变、内容可变）。
 *
 * 只读不写业务数据：本脚本不修改任何灵感文件，只提取并校验。
 *
 * 用法：node server/scripts/generate-inspiration-index.mjs
 */

import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url)); // <repo>/server/scripts
const ROOT = path.resolve(HERE, '..'); // <repo>/server
const INSPIRATIONS_DIR = path.join(ROOT, 'market', 'inspirations');
const OUT_FILE = path.join(INSPIRATIONS_DIR, 'index.json');
/** 索引自身不是灵感，扫描时要跳过 */
const INDEX_BASENAME = 'index.json';
/** 摘要长度：够覆盖提示词开头的场景说明，又不至于让索引跟着内容一起膨胀 */
const EXCERPT_LEN = 200;
/** 单条灵感最多保留几个标签：市场卡片就那么宽，再多也显示不下 */
const MAX_TAGS = 8;
/** 单个标签的长度上限：标签是"分类"，不是句子 */
const MAX_TAG_LEN = 32;
/** id 允许的字符集：小写字母 / 数字 / 短横线，且以字母或数字开头（同时也是目录名与 URL 片段） */
const ID_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;
/** id 长度上限：够长，又能保证跨平台目录名与 URL 片段都不会出问题 */
const ID_MAX_LEN = 64;

/**
 * 稳定序列化：对象键按**字典序**输出，数组保持原序（递归）。
 * 用于 contentHash —— 保证「同内容 ⇒ 同哈希」，不受键书写顺序影响。
 */
function stableStringify(value) {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${stableStringify(value[k])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

/**
 * 单条市场内容的稳定哈希：对 `{id,title,icon,content,tags}` 稳定序列化后取 sha256 前 16 位十六进制。
 * 客户端据此判断"市场内容是否变过"（哈希不变即可跳过重新拉取）。
 */
function contentHashOf({ id, title, icon, content, tags }) {
  const payload = stableStringify({ id, title, icon, content, tags });
  return createHash('sha256').update(payload, 'utf8').digest('hex').slice(0, 16);
}

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

/** 正文压成单行再截断，供列表展示与搜索使用 */
function excerptOf(content) {
  const flat = content.replace(/\s+/g, ' ').trim();
  return flat.length > EXCERPT_LEN ? flat.slice(0, EXCERPT_LEN) + '…' : flat;
}

/**
 * 归一化标签：只留非空字符串，去首尾空白、超长截断、去重、限制条数。
 *
 * 与 id/title/content 的处理有个刻意的区别：**标签的问题只提示、不跳过整条**。
 * 标签是可选元数据，为一个写错的标签把整条灵感拦在市场外，代价远大于收益。
 * 被丢弃/裁剪的每一处都逐条报出来，不静默改数据。
 */
function normalizeTags(raw, file, warnings) {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw)) {
    warnings.push(`${file}（tags 不是数组，已忽略）`);
    return [];
  }
  const out = [];
  for (const item of raw) {
    if (typeof item !== 'string') {
      warnings.push(`${file}（tags 里有非字符串项，已忽略）`);
      continue;
    }
    let t = item.trim();
    if (!t) {
      warnings.push(`${file}（tags 里有空白项，已忽略）`);
      continue;
    }
    if (t.length > MAX_TAG_LEN) {
      warnings.push(`${file}（标签「${t}」超过 ${MAX_TAG_LEN} 字，已截断）`);
      t = t.slice(0, MAX_TAG_LEN);
    }
    if (out.includes(t)) {
      warnings.push(`${file}（标签「${t}」重复，已去重）`);
      continue;
    }
    out.push(t);
  }
  if (out.length > MAX_TAGS) {
    warnings.push(`${file}（标签 ${out.length} 个超过上限 ${MAX_TAGS}，只取前 ${MAX_TAGS} 个）`);
    return out.slice(0, MAX_TAGS);
  }
  return out;
}

async function main() {
  let entries;
  try {
    entries = await fs.readdir(INSPIRATIONS_DIR, { withFileTypes: true });
  } catch {
    console.error(`找不到灵感目录：${INSPIRATIONS_DIR}`);
    process.exit(1);
  }

  const dirs = entries
    .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
    .map((e) => e.name)
    .sort();
  const stray = entries
    .filter((e) => e.isFile() && e.name.toLowerCase().endsWith('.json') && e.name !== INDEX_BASENAME)
    .map((e) => e.name);

  const inspirations = [];
  const skipped = [];
  const warnings = [];
  const errors = []; // 硬校验失败（id 非法 / 撞车等）：汇总后直接退出，让 CI 挂掉
  const seen = new Map(); // id -> 物理位置，用于查重

  // 旧布局（扁平文件）要明确报出来：否则最容易出现"文件还在、市场里却没有这条"
  for (const name of stray) {
    skipped.push(`${name}（旧的扁平布局已不再收录：请移到 ${name.replace(/\.json$/i, '')}/${name}）`);
  }

  for (const dir of dirs) {
    const absDir = path.join(INSPIRATIONS_DIR, dir);
    // 目录内允许放附件；正文只认一个 JSON：优先 `<目录名>.json`，否则取目录里唯一的那一个
    const jsonNames = (await fs.readdir(absDir, { withFileTypes: true }))
      .filter((e) => e.isFile() && e.name.toLowerCase().endsWith('.json'))
      .map((e) => e.name)
      .sort();

    let file = jsonNames.find((name) => name === `${dir}.json`);
    if (!file) {
      if (jsonNames.length === 0) {
        skipped.push(`${dir}/（目录内没有 .json 正文）`);
        continue;
      }
      if (jsonNames.length > 1) {
        skipped.push(`${dir}/（目录内有 ${jsonNames.length} 个 .json：正文请放在 ${dir}.json）`);
        continue;
      }
      [file] = jsonNames;
      warnings.push(`${dir}/${file}（正文与目录不同名：建议改名为 ${dir}.json，不改也能正常工作）`);
    }

    const location = `${dir}/${file}`;
    const absFile = path.join(absDir, file);

    let item;
    try {
      item = JSON.parse(await fs.readFile(absFile, 'utf8'));
    } catch (e) {
      skipped.push(`${location}（不是合法 JSON：${e.message}）`);
      continue;
    }

    // id 是**权威键**：必须显式给出、**等于目录名**，且满足字符集 / 长度约束。
    // 这里**不猜测、不回落**到目录名；任何一条不满足都直接记为硬错误（而不是静默跳过），
    // 由 main 末尾统一报错退出 —— 静默跳过的代价是"写错 id"以另一条灵感的名义混进市场，
    // 而"改 id"又会让已导入旧 id 的用户重复导入（故 id 一旦发布即不可更改，见文件头）。
    const id = typeof item.id === 'string' ? item.id.trim() : '';
    if (!id) {
      errors.push(`${location}（缺少 id 字段：id 是权威键，必须显式给出）`);
      continue;
    }
    if (id !== dir) {
      errors.push(`${location}（id「${id}」与目录名「${dir}」不一致：id 必须以目录名为准）`);
      continue;
    }
    if (id.length > ID_MAX_LEN) {
      errors.push(`${location}（id「${id}」长度 ${id.length} 超过上限 ${ID_MAX_LEN}）`);
      continue;
    }
    if (!ID_PATTERN.test(id)) {
      errors.push(
        `${location}（id「${id}」不符合 ${ID_PATTERN}：仅允许小写字母 / 数字 / 短横线，且以字母或数字开头与结尾）`,
      );
      continue;
    }

    // id 全局唯一：客户端用它判断"是否已导入"，两条撞同一个 id 会互相覆盖
    if (seen.has(id)) {
      errors.push(`${location}（id「${id}」与 ${seen.get(id)} 重复：id 是权威键，不能撞车）`);
      continue;
    }

    const title = typeof item.title === 'string' ? item.title.trim() : '';
    const content = typeof item.content === 'string' ? item.content : '';
    if (!title) {
      skipped.push(`${location}（title 为空）`);
      continue;
    }
    if (!content.trim()) {
      skipped.push(`${location}（content 为空）`);
      continue;
    }

    seen.set(id, location);
    // icon / tags 先行归一：contentHash 要反映**最终发布值**（与索引里写出的字段一致）
    const icon = typeof item.icon === 'string' && item.icon ? item.icon : '💡';
    // 可选：不写就是无标签（本地库、市场卡片、会话侧栏三处都能正确处理空数组）
    const tags = normalizeTags(item.tags, location, warnings);
    inspirations.push({
      id,
      title,
      icon,
      tags,
      excerpt: excerptOf(content),
      // 客户端据此判断"市场内容是否变过"：对 {id,title,icon,content,tags} 稳定序列化后取 sha256 前 16 位
      contentHash: contentHashOf({ id, title, icon, content, tags }),
      // 两段分别 URL 编码：目录名与文件名都允许中文，让调用方各自编码迟早出错，这里一次编好
      path: `inspirations/${encodeURIComponent(dir)}/${encodeURIComponent(file)}`,
    });
  }

  // 硬校验失败：汇总报出后退出（不写索引），让 CI 明确挂掉
  if (errors.length > 0) {
    console.error(`校验失败 ${errors.length} 处（id 是权威键，必须与目录名一致且合法）：`);
    for (const e of errors) console.error(`  ✗ ${e}`);
    process.exit(1);
  }

  inspirations.sort((a, b) => a.title.localeCompare(b.title, 'zh'));

  const index = {
    schemaVersion: '1.0',
    updatedAt: new Date().toISOString(),
    inspirations,
  };
  await writeIndexStable(OUT_FILE, index);

  console.log(
    `已生成 ${path.relative(ROOT, OUT_FILE)}：${inspirations.length} 条灵感（扫描 ${dirs.length} 个灵感目录）`,
  );
  for (const it of inspirations) {
    console.log(`  · ${it.icon} ${it.title}（摘要 ${it.excerpt.length} 字）`);
  }
  if (warnings.length > 0) {
    console.warn(`\n提示 ${warnings.length} 条（不影响生成）：`);
    for (const w of warnings) console.warn(`  ~ ${w}`);
  }
  if (skipped.length > 0) {
    console.warn(`\n跳过 ${skipped.length} 个文件：`);
    for (const s of skipped) console.warn(`  ! ${s}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
