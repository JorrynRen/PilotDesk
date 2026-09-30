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
 * 用法：node server/scripts/generate-workflow-index.mjs
 */

import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url)); // <repo>/server/scripts
const ROOT = path.resolve(HERE, '..'); // <repo>/server
const WORKFLOW_DIR = path.join(ROOT, 'market', 'workflow');
const OUT_FILE = path.join(WORKFLOW_DIR, 'workflow-index.json');

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
    const st = await fs.stat(absDir);

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
      updatedAt: st.mtime.toISOString(),
    });
  }

  templates.sort((a, b) => a.name.localeCompare(b.name, 'zh'));

  const index = {
    schemaVersion: '1.0',
    updatedAt: new Date().toISOString(),
    templates,
  };
  await fs.writeFile(OUT_FILE, JSON.stringify(index, null, 2) + '\n', 'utf8');

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
