#!/usr/bin/env node
/**
 * PilotDesk 本地资源服务（**仅用于开发测试**，不要拿它上线）
 *
 * 为什么需要它：桌面端的在线商店与工作流模板市场，数据源都是
 * `<base>/server/market/...`（见 `src-tauri/src/utils/market.rs` 的 SERVER_SOURCES）。
 * 线上是 jsdelivr / GitHub raw；本地开发要迭代模板与官网时必须有一个同构的 HTTP 源，
 * 否则客户端只能读前端写死的假数据。
 *
 * 它做的事很少，刻意如此：把仓库的 `server/` 原样挂到 `/server` 下。
 * 于是 `server/market/workflow/workflow-index.json` 在
 * `http://127.0.0.1:1421/server/market/workflow/workflow-index.json` 就能取到，
 * 与线上源的路径**完全一致** —— 客户端换源只需换 base，不用改任何路径常量。
 *
 * 用法：
 *   node server/dev-server.mjs                 # 默认 127.0.0.1:1421
 *   node server/dev-server.mjs --port 1500
 *   node server/dev-server.mjs --web ../web/dist   # 顺带把官网构建产物挂在 /
 *
 * 设计取舍：
 *   - **零依赖**：只用 node:http / node:fs，不引入 serve / http-server 这类包。
 *     这是个开发辅助脚本，不该给项目加依赖，也不该要求额外的运行时。
 *   - **只监听回环地址**：本地测试用，不暴露到局域网，避免机器上出现一个无鉴权的文件服务。
 *   - **带 CORS**：官网页面（另一个端口）与浏览器里直接 fetch 调试都需要它。
 *   - **支持目录浏览**：静态托管没有目录列表，光看文件系统不容易确认"服务到底挂上了没"，
 *     所以目录请求直接回一份 JSON 清单。
 */

import { createServer } from 'node:http';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url)); // <repo>/server
const REPO = path.resolve(HERE, '..');

/** `--key value` 形式的简单取值 */
function argValue(key) {
  const i = process.argv.indexOf(key);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : '';
}

const PORT = Number(argValue('--port') || process.env.PORT || 1421);
const HOST = '127.0.0.1';
/** 可选的官网构建产物目录：给了就挂在根路径上，便于"一个端口调试官网 + 资源" */
const WEB_DIR = argValue('--web') ? path.resolve(REPO, argValue('--web')) : '';

const MIME = {
  '.json': 'application/json; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
};
const mimeOf = (p) => MIME[path.extname(p).toLowerCase()] || 'application/octet-stream';

function sendJson(res, status, data) {
  const body = JSON.stringify(data, null, 2);
  res.writeHead(status, { 'Content-Type': MIME['.json'], 'Content-Length': Buffer.byteLength(body) });
  res.end(body);
}

/**
 * 解析出该请求对应的磁盘路径。
 *
 * 两道防护：① decode 后再 resolve，杜绝 `%2e%2e%2f` 这类编码绕过；
 * ② 结果必须仍在允许的根目录内 —— 这是本地文件服务唯一真正危险的地方。
 */
function resolveFsPath(urlPath) {
  let rel = urlPath;
  let root;
  if (rel === '/' || rel === '') {
    if (WEB_DIR) {
      root = WEB_DIR;
      rel = '/';
    } else {
      root = HERE; // 没给官网目录时，根路径列出 market 资源树
      rel = '/';
    }
  } else if (rel.startsWith('/server/')) {
    root = HERE;
    rel = rel.slice('/server'.length);
  } else if (WEB_DIR) {
    root = WEB_DIR;
  } else {
    return null;
  }

  const abs = path.resolve(root, '.' + rel);
  if (abs !== root && !abs.startsWith(root + path.sep)) return null; // 越界
  return abs;
}

/** 目录浏览：列出条目，标注是目录还是文件 */
async function listDir(abs, urlPath) {
  const names = await fs.readdir(abs, { withFileTypes: true });
  const entries = await Promise.all(
    names
      // 只读用途，隐藏文件（.gitkeep 之类）不列出来
      .filter((e) => !e.name.startsWith('.'))
      .map(async (e) => {
        const child = path.join(abs, e.name);
        const st = await fs.stat(child).catch(() => null);
        return {
          name: e.name,
          type: e.isDirectory() ? 'dir' : 'file',
          size: e.isDirectory() ? null : st?.size ?? null,
          url: `${urlPath.replace(/\/$/, '')}/${encodeURIComponent(e.name)}`,
        };
      }),
  );
  entries.sort((a, b) => (a.type === b.type ? a.name.localeCompare(b.name) : a.type === 'dir' ? -1 : 1));
  return { path: urlPath, entries };
}

const server = createServer(async (req, res) => {
  // 本地调试工具，放开跨源：官网开发服务器（另一个端口）与浏览器直接 fetch 都要用
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'GET,HEAD,OPTIONS');
  res.setHeader('Cache-Control', 'no-store'); // 开发期不缓存，改了立刻生效
  if (req.method === 'OPTIONS') {
    res.writeHead(204);
    return res.end();
  }
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    return sendJson(res, 405, { error: '只支持 GET' });
  }

  let urlPath;
  try {
    urlPath = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  } catch {
    return sendJson(res, 400, { error: 'URL 编码非法' });
  }

  const abs = resolveFsPath(urlPath);
  if (!abs) return sendJson(res, 404, { error: `没有这个路径：${urlPath}`, 提示: '资源在 /server/... 下' });

  try {
    const st = await fs.stat(abs);
    if (st.isDirectory()) {
      // 目录有 index.html 就给它（官网用），否则回 JSON 清单
      const indexHtml = path.join(abs, 'index.html');
      const hasIndex = await fs.stat(indexHtml).then((s) => s.isFile()).catch(() => false);
      if (hasIndex) {
        const body = await fs.readFile(indexHtml);
        res.writeHead(200, { 'Content-Type': MIME['.html'], 'Content-Length': body.length });
        return res.end(body);
      }
      return sendJson(res, 200, await listDir(abs, urlPath));
    }

    const body = await fs.readFile(abs);
    res.writeHead(200, { 'Content-Type': mimeOf(abs), 'Content-Length': body.length });
    res.end(req.method === 'HEAD' ? undefined : body);
  } catch (e) {
    if (e.code === 'ENOENT' || e.code === 'ENOTDIR') {
      return sendJson(res, 404, { error: `文件不存在：${urlPath}` });
    }
    return sendJson(res, 500, { error: String(e.message || e) });
  }
});

// 端口被占用是最常见的启动失败：给一句能直接照做的提示，而不是甩一段 Node 栈
server.on('error', (e) => {
  if (e.code === 'EADDRINUSE') {
    console.error(`\n  端口 ${PORT} 已被占用。`);
    console.error(`  多半是上一次的服务还开着；也可以换个端口：`);
    console.error(`    node server/dev-server.mjs --port 1500\n`);
    process.exit(1);
  }
  throw e;
});

server.listen(PORT, HOST, () => {
  const base = `http://${HOST}:${PORT}`;
  console.log(`\n  PilotDesk 本地资源服务（开发测试用）`);
  console.log(`  监听  ${base}`);
  console.log(`  根目录 ${HERE}`);
  if (WEB_DIR) console.log(`  官网   ${WEB_DIR} → ${base}/`);
  console.log(`\n  常用地址：`);
  console.log(`    工作流模板索引  ${base}/server/market/workflow/workflow-index.json`);
  console.log(`    插件索引        ${base}/server/market/plugins/plugins-index.json`);
  console.log(`    灵感市场索引    ${base}/server/market/inspirations/index.json`);
  console.log(`    Agent 配置      ${base}/server/market/agents-config/agents-config.json`);
  console.log(`    资源树          ${base}/server/market\n`);
});
