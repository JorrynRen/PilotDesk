#!/usr/bin/env node
/**
 * 生成灵感市场索引 `server/market/inspirations/index.json`
 *
 * 为什么需要索引：静态托管没有目录列表，客户端没法"问服务器有哪些灵感"。
 * 一条灵感一个文件（约定放在 `inspirations/<id>.json`），索引负责把清单固化下来 ——
 * 与插件市场的 `plugins-index.json`、模板市场的 `workflow-index.json` 同一个道理。
 *
 * 索引**只放浏览/搜索需要的元信息**（含正文摘要），正文全文留在源文件里按需拉取。
 * 这样正文只有一份真相，且与另外两个市场同构；代价是搜索覆盖到摘要为止（摘要 200 字，
 * 基本涵盖提示词的场景说明部分），详情多一次请求。
 *
 * 身份约定：**`id` 以文件里的字段为准**，文件名只是物理位置。
 * 两者不一致只提示、不拦截 —— 若把它们绑死，"把文件名改通顺"就会变成破坏性操作
 * （改名就得改 id，而改 id 会让已导入这条灵感的用户重复导入）。
 * 因为 id 与文件名解耦，**id 撞车成为可能**，所以这里必须查重。
 *
 * 只读不写业务数据：本脚本不修改任何灵感文件，只提取并校验。
 *
 * 用法：node server/scripts/generate-inspiration-index.mjs
 */

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

/** 正文压成单行再截断，供列表展示与搜索使用 */
function excerptOf(content) {
  const flat = content.replace(/\s+/g, ' ').trim();
  return flat.length > EXCERPT_LEN ? flat.slice(0, EXCERPT_LEN) + '…' : flat;
}

async function main() {
  let files;
  try {
    files = (await fs.readdir(INSPIRATIONS_DIR, { withFileTypes: true }))
      .filter((e) => e.isFile() && e.name.toLowerCase().endsWith('.json'))
      .map((e) => e.name)
      .filter((name) => name !== INDEX_BASENAME);
  } catch {
    console.error(`找不到灵感目录：${INSPIRATIONS_DIR}`);
    process.exit(1);
  }

  const inspirations = [];
  const skipped = [];
  const warnings = [];
  const seen = new Map(); // id -> 文件名，用于查重

  for (const file of files) {
    const absFile = path.join(INSPIRATIONS_DIR, file);
    const idFromFile = file.replace(/\.json$/i, '');

    let item;
    try {
      item = JSON.parse(await fs.readFile(absFile, 'utf8'));
    } catch (e) {
      skipped.push(`${file}（不是合法 JSON：${e.message}）`);
      continue;
    }

    // id 必须显式给出：它同时是去重键（客户端靠它判断"这条是否已导入过"）。
    // 这里**不猜测、不回落**到文件名 —— 静默回落的代价是"忘了写 id"变成一个
    // 看不出来的状态，而写错 id 则会以另一条灵感的名义混进市场。
    const id = typeof item.id === 'string' ? item.id.trim() : '';
    if (!id) {
      skipped.push(`${file}（缺少 id 字段）`);
      continue;
    }

    // id 全局唯一：客户端用它判断"是否已导入"，两条撞同一个 id 会互相覆盖
    if (seen.has(id)) {
      skipped.push(`${file}（id「${id}」与 ${seen.get(id)} 重复：id 是去重键，不能撞车）`);
      continue;
    }

    const title = typeof item.title === 'string' ? item.title.trim() : '';
    const content = typeof item.content === 'string' ? item.content : '';
    if (!title) {
      skipped.push(`${file}（title 为空）`);
      continue;
    }
    if (!content.trim()) {
      skipped.push(`${file}（content 为空）`);
      continue;
    }

    // 提示只针对"真正收录进来"的条目：被跳过的文件再提示它的文件名没意义
    if (id !== idFromFile) {
      warnings.push(`${file}（声明的 id 是「${id}」：建议把文件改名为 ${id}.json，不改也能正常工作）`);
    }

    seen.set(id, file);
    const st = await fs.stat(absFile);
    inspirations.push({
      id,
      title,
      icon: typeof item.icon === 'string' && item.icon ? item.icon : '💡',
      excerpt: excerptOf(content),
      // 已 URL 编码：文件名允许中文，让调用方各自编码迟早出错，这里一次编好
      path: `inspirations/${encodeURIComponent(file)}`,
      updatedAt: st.mtime.toISOString(),
    });
  }

  inspirations.sort((a, b) => a.title.localeCompare(b.title, 'zh'));

  const index = {
    schemaVersion: '1.0',
    updatedAt: new Date().toISOString(),
    inspirations,
  };
  await fs.writeFile(OUT_FILE, JSON.stringify(index, null, 2) + '\n', 'utf8');

  console.log(`已生成 ${path.relative(ROOT, OUT_FILE)}：${inspirations.length} 条灵感`);
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
