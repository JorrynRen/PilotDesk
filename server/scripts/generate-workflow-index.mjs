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

/** 主定义文件的命名约定：`[主]xxx.json`（其余 `[子]xxx.json` 是它引用的子工作流） */
const MAIN_PREFIX = '[主]';

const humanSize = (bytes) =>
  bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)}M` : `${Math.max(1, Math.round(bytes / 1024))}K`;

/** 节点总数：跨全部阶段累加（模板是 stages[].nodes[] 的结构） */
function countNodes(def) {
  if (!Array.isArray(def?.stages)) return 0;
  return def.stages.reduce((n, s) => n + (Array.isArray(s?.nodes) ? s.nodes.length : 0), 0);
}

async function main() {
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

    const subFiles = files.filter((f) => f !== mainFile);
    let bytes = 0;
    for (const f of files) {
      bytes += (await fs.stat(path.join(absDir, f))).size;
    }

    templates.push({
      id: dir,
      name: typeof def.name === 'string' && def.name ? def.name : dir,
      version: typeof def.version === 'string' ? def.version : '',
      description: typeof def.description === 'string' ? def.description : '',
      // 已 URL 编码的路径：模板名带中文与括号，让调用方各自编码迟早出错，这里一次编好
      path: `workflow/${encodeURIComponent(dir)}/${encodeURIComponent(mainFile)}`,
      dir,
      mainFile,
      subFiles,
      stageCount: Array.isArray(def.stages) ? def.stages.length : 0,
      nodeCount: countNodes(def),
      size: humanSize(bytes),
    });
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
    console.log(`  · ${t.name} v${t.version} — ${t.stageCount} 阶段 / ${t.nodeCount} 节点 / ${t.size}`);
  }
  if (skipped.length > 0) {
    console.warn(`\n跳过 ${skipped.length} 个目录：`);
    for (const s of skipped) console.warn(`  ! ${s}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
