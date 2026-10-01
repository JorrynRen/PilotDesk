#!/usr/bin/env node
/**
 * 生成插件市场索引 `server/market/plugins/plugins-index.json`
 *
 * 背景：插件目录是 `plugins/<plugin-id>/<version>/`，而静态托管（jsdelivr / 自建源）
 * **没有目录列表**，客户端无法"问服务器有哪些插件"。所以必须有一个索引文件把清单固化下来，
 * 这与工作流模板市场的 `workflow-index.json` 是同一个道理（那个见 `generate-workflow-index.mjs`）。
 *
 * 索引里**只存相对路径**，不出现任何绝对 URL —— 因此这里无需写死"主源"地址，
 * 也就不存在"改镜像还得改生成器"的漂移问题。绝对地址由客户端在运行时用
 * 「实际可用的源 + 相对 path」拼出来（见 src-tauri/src/utils/market.rs 的 SERVER_SOURCES
 * 与 build_market_url）；换镜像只需改那一处，索引不必重新生成。
 *
 * 本脚本是 `.github/workflows/generate-index.yml` 里那段内联 bash 的等价实现 ——
 * 搬成 Node 脚本是为了能本地跑、能调试，与工作流模板索引生成器保持同一形态。
 * **输出必须与原 bash 逐字节一致**（除 `updatedAt` 外）。
 *
 * 用法：
 *   node server/scripts/generate-plugin-index.mjs [输出路径]
 *   默认写入 server/market/plugins/plugins-index.json；带参数则写到指定路径（便于本地验证）。
 */

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url)); // <repo>/server/scripts
const SERVER_ROOT = path.resolve(HERE, '..'); // <repo>/server
const REPO_ROOT = path.resolve(HERE, '..', '..'); // <repo>
const PLUGINS_DIR = path.join(SERVER_ROOT, 'market', 'plugins');
const DEFAULT_OUT = path.join(PLUGINS_DIR, 'plugins-index.json');

/** 索引在 git 里的仓库相对路径（用于取上一版索引做"版本不可变"校验） */
const INDEX_REPO_PATH = 'server/market/plugins/plugins-index.json';

/**
 * `du -sh` 在 CI（Ubuntu / ext4，块大小 4096）下是按**块**计费的：
 * 每个文件向上取整到 4096 字节的整数倍，目录自身也占一个块。
 * 这里用同样的规则复算，得到跨平台、可复现的等效字节数。
 *
 * 与 `du` 的差异：`du` 读的是文件系统**实际分配**的块（受块大小、稀疏文件等影响），
 * 所以"原样调用 du"在本机和 CI 上可能得到不同结果；本实现固定 4096 口径，任何机器上结果一致。
 */
const DU_BLOCK_SIZE = 4096;

/** 把字节数格式化成 `du -sh` 的 `xxK` / `xxM` 形式 */
function humanSize(bytes) {
  return bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)}M`
    : `${Math.max(1, Math.round(bytes / 1024))}K`;
}

/** 递归统计目录的"磁盘占用"，规则见 DU_BLOCK_SIZE 的注释 */
async function duEquivalentBytes(dir) {
  let total = DU_BLOCK_SIZE; // 目录自身占一个块
  const entries = await fs.readdir(dir, { withFileTypes: true });
  for (const ent of entries) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) {
      total += await duEquivalentBytes(p);
    } else if (ent.isFile()) {
      const { size } = await fs.stat(p);
      total += size === 0 ? 0 : Math.ceil(size / DU_BLOCK_SIZE) * DU_BLOCK_SIZE;
    }
  }
  return total;
}

/**
 * 入口文件的 sha256（十六进制）——安装时的完整性锚点。
 *
 * **必须先做 LF 归一化再算**：CI 在 Linux 上 checkout 得到 LF，而 Windows 本地工作区是 CRLF
 * （仓库 `core.autocrlf=true`），不归一化会让同一份文件在两平台得到不同 sha256，
 * 从而导致所有插件安装时完整性校验失败。索引口径以 LF 为准。
 */
function sha256HexLfNormalized(buf) {
  const out = Buffer.allocUnsafe(buf.length);
  let n = 0;
  for (let i = 0; i < buf.length; i++) {
    if (buf[i] === 0x0d && buf[i + 1] === 0x0a) continue; // 丢弃 CRLF 里的 CR，保留 LF
    out[n++] = buf[i];
  }
  return createHash('sha256').update(out.subarray(0, n)).digest('hex');
}

/** 等价 jq 的 `.<key> // <default>`：null / false 视为空（缺省值生效） */
function jqValue(obj, key, dflt = '') {
  const v = obj?.[key];
  return v === undefined || v === null || v === false ? dflt : String(v);
}

/** 图标探测：bash 里先 png 后 ico，同一后缀下 favicon 优先；都没有则空串 */
function detectIcon(files) {
  for (const ext of ['png', 'ico']) {
    const icon = `icon.${ext}`;
    const favicon = `favicon.${ext}`;
    if (files.has(icon) || files.has(favicon)) return files.has(favicon) ? favicon : icon;
  }
  return '';
}

/** 等价 bash case 的 semver 粗校验：`[0-9]*.[0-9]*.[0-9]*` */
function isSemver(v) {
  return /^[0-9].*\.[0-9].*\.[0-9].*$/.test(v);
}

/**
 * 处理单个 `<plugin-id>/<version>/` 目录，产出索引条目；不满足条件时返回 null 并打印原因。
 * 跳过规则与 bash 的 `continue` 语义一致：宁可不产出，也不写入半个条目。
 */
async function collectEntry(pluginId, dirVersion) {
  const pluginDir = path.join(PLUGINS_DIR, pluginId, dirVersion);
  const manifestPath = path.join(pluginDir, 'manifest.json');

  let manifest;
  try {
    manifest = JSON.parse(await fs.readFile(manifestPath, 'utf8'));
  } catch (e) {
    console.warn(`跳过插件 ${pluginId}：manifest.json 缺失或损坏（${e.message}）`);
    return null;
  }

  // 预校验 manifest 基本合法性（沿用 bash 的规则，跳过恶意/损坏的插件）
  const manifestId = jqValue(manifest, 'id');
  const manifestName = jqValue(manifest, 'name');
  const manifestVerRaw = jqValue(manifest, 'version');

  if (manifestId.includes('/') || manifestId.includes('\\')) {
    console.warn(`跳过插件 ${manifestId}: id 包含路径分隔符`);
    return null;
  }
  if (!isSemver(manifestVerRaw)) {
    console.warn(`跳过插件 ${manifestId}: 版本号非 semver 格式 (${manifestVerRaw})`);
    return null;
  }
  const nameLen = [...manifestName].length;
  if (nameLen > 64) {
    console.warn(`跳过插件 ${manifestId}: name 超过 64 字符 (${nameLen})`);
    return null;
  }

  // 入口文件：默认 index.js（与 bash 一致），禁止路径遍历
  const entryObj = manifest?.entry && typeof manifest.entry === 'object' ? manifest.entry : {};
  const entryMain = jqValue(entryObj, 'main', 'index.js');
  if (entryMain.includes('..')) {
    console.warn(`跳过插件 ${manifestId}: entry.main 包含路径遍历`);
    return null;
  }

  // 计算入口文件 sha256（缺失/读不到就跳过，不做静默放行）
  let entryBytes;
  try {
    entryBytes = await fs.readFile(path.join(pluginDir, entryMain));
  } catch {
    console.warn(`跳过插件 ${manifestId}: 无法计算入口文件 sha256 (${entryMain})`);
    return null;
  }
  const sha256 = sha256HexLfNormalized(entryBytes);

  const files = new Set(await fs.readdir(pluginDir));
  const size = humanSize(await duEquivalentBytes(pluginDir));
  // version 以 manifest 为准，缺失/为空时回退目录名（与 bash 的 manifest_version 逻辑一致）
  const version = jqValue(manifest, 'version') || dirVersion;

  // 字段顺序必须与 bash 完全一致：sha256, id, name, version, description, author,
  // minAppVersion, path, icon, size, readme
  return {
    sha256,
    id: pluginId, // 与 bash 一致：id 取自**目录名**
    name: manifestName,
    version,
    description: jqValue(manifest, 'description'),
    author: jqValue(manifest, 'author'),
    minAppVersion: jqValue(manifest, 'minAppVersion'),
    path: `plugins/${pluginId}/${dirVersion}`, // 相对路径，版本取**目录名**（与 bash 的 rel_path 一致）
    icon: detectIcon(files),
    size,
    readme: files.has('README.md') ? 'README.md' : '',
  };
}

/**
 * 版本不可变校验：插件文件路径里带版本号（`plugins/<id>/<version>/...`），一旦发布，
 * **同一版本号下的内容必须保持不变**。否则 CDN（jsdelivr 等）会出现"索引已刷新、文件仍是旧缓存"：
 * 安装时下载到旧文件，sha256 完整性校验失败。
 *
 * 做法：把新算出的条目与上一版索引（`git show HEAD:...`）对比。
 *   - 同 id + 同 version 但 sha256 变了 → 报错退出（非 0）
 *   - 同 id + 同 version 且 sha256 相同 → 放行
 *   - version 变了 → 一律放行（新版本本来就会产生新路径）
 *   - 取不到上一版索引（首次生成 / git 不可用）→ 跳过校验并提示，不中断
 *
 * 逃生口：设置环境变量 `ALLOW_SAME_VERSION_REWRITE=1` 可跳过拦截，
 * 用于"确实需要原地改内容、且已接受 CDN 缓存风险"的场景。
 */
function checkVersionImmutability(entries) {
  if (process.env.ALLOW_SAME_VERSION_REWRITE === '1') {
    console.warn('已设置 ALLOW_SAME_VERSION_REWRITE=1，跳过"版本不可变"校验（存在 CDN 缓存风险）');
    return;
  }

  let prev;
  try {
    const raw = execFileSync('git', ['show', `HEAD:${INDEX_REPO_PATH}`], {
      cwd: REPO_ROOT,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    });
    prev = JSON.parse(raw);
  } catch {
    console.warn('取不到上一版索引（首次生成或 git 不可用），跳过"版本不可变"校验');
    return;
  }

  const prevSha = new Map((prev.plugins ?? []).map((p) => [`${p.id}@${p.version}`, p.sha256]));
  const offenders = entries.filter((e) => {
    const old = prevSha.get(`${e.id}@${e.version}`);
    return old !== undefined && old !== e.sha256;
  });
  if (offenders.length === 0) return;

  console.error('检测到已发布版本的内容发生变化（sha256 变化），已中止生成：');
  for (const e of offenders) {
    console.error(
      `  插件 ${e.id} 版本 ${e.version} 的内容已变化（sha256 变化），` +
        '请把版本号升一位（如 1.0.0 → 1.0.1）后重试',
    );
  }
  console.error(
    '如确实需要原地改内容并接受 CDN 缓存风险，可设置环境变量 ALLOW_SAME_VERSION_REWRITE=1 跳过该校验。',
  );
  process.exit(1);
}

async function main() {
  const outFile = process.argv[2] ? path.resolve(process.argv[2]) : DEFAULT_OUT;

  // 扫描 server/market/plugins/<plugin-id>/<version>/；跳过隐藏目录（与 bash glob 的 `*` 一致）
  let pluginDirs;
  try {
    pluginDirs = (await fs.readdir(PLUGINS_DIR, { withFileTypes: true }))
      .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
      .map((e) => e.name)
      .sort();
  } catch {
    console.error(`找不到插件目录：${PLUGINS_DIR}`);
    process.exit(1);
  }

  const entries = [];
  for (const pluginId of pluginDirs) {
    let versionDirs;
    try {
      versionDirs = (await fs.readdir(path.join(PLUGINS_DIR, pluginId), { withFileTypes: true }))
        .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
        .map((e) => e.name)
        .sort();
    } catch {
      continue;
    }
    for (const dirVersion of versionDirs) {
      const entry = await collectEntry(pluginId, dirVersion);
      if (entry) entries.push(entry);
    }
  }

  checkVersionImmutability(entries);

  // 拼接格式与 bash 逐字节同构（含括号 / 换行位置）：
  //   头 + '\n' + 条目（条目间为 '\n,\n'）+ '\n' + ']}\n'
  const updatedAt = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
  const header = `{"schemaVersion":"1.0","updatedAt":"${updatedAt}","plugins":[`;
  const body = entries.map((e) => JSON.stringify(e, null, 2)).join('\n,\n');
  const content = entries.length === 0 ? `${header}\n]}\n` : `${header}\n${body}\n]}\n`;

  await fs.writeFile(outFile, content, 'utf8');

  console.log(`已生成 ${path.relative(REPO_ROOT, outFile)}：${entries.length} 个插件`);
  for (const e of entries) {
    console.log(`  · ${e.name} v${e.version} — ${e.size} — ${e.path}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
