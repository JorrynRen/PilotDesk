/**
 * Markdown / 纯文本处理工具。
 *
 * 从 MarkdownRenderer.tsx 拆出：这些是不含组件的纯函数，而 `react-refresh/only-export-components`
 * 要求"只导出组件"的文件不得混入非组件导出，故独立成文件供多个组件共用。
 */

// 识别文件路径并转成可点击的自定义协议链接（open_path 打开文件）。
// 约束：
// - 以盘符绝对路径（E:\...）或 ./ ../ 相对路径开头，且**含至少一个目录分隔符**；
// - **必须以文件扩展名结尾**（\.[A-Za-z][A-Za-z0-9]{0,5}，扩展名**字母开头**，避免把
//   `_v1.0.md` 中的 `.0` 误认作扩展名）——避免把"纯目录 + 中文正文/文件名"
//   截断成半截链接（原正则字符集不含中文，会把 `E:\tmp\Agent\中文名.docx` 拆成
//   `[E:\tmp\Agent](...)中文名.docx` 的污染输出）；
// - 路径字符排除空白、括号、引号与全/半角标点（文件名部分**允许 `.`**，如版本号
//   `_v1.0.md`），避免吞进正文标点；
// - 前缀负向断言排除已存在于 Markdown 链接语法（`[x](path)` / `<path>`）中的路径。
const PATH_LINK_RE = /(`?)((?<![([<])(?:[A-Za-z]:[\\/](?![\\/])|\.{1,2}[\\/])[^\s[\](){}<>"'`，。；：！？,;:!?]*[\\/][^\s[\](){}<>"'`，。；：！？,;:!?]*\.[A-Za-z][A-Za-z0-9]{0,5})\1/g;

function linkifyPaths(content: string): string {
  return content.replace(PATH_LINK_RE, (full, tick, path) => {
    // 正则已排除边界标点与空白，无需二次裁剪
    if (!path || path.length < 2) return full;
    const label = tick ? `\`${path}\`` : path;
    return `[${label}](pilotdesk-path://${encodeURIComponent(path)})`;
  });
}

/**
 * 折叠冗余空行：把 `\r\n` / `\r` 归一化为 `\n`，将仅含空白字符的行视为空行，
 * 连续空行折叠为单个空行（`\n\n`），并去除首尾空行。
 */
export function collapseBlankLines(content: string): string {
  const lines = content.replace(/\r\n?/g, '\n').split('\n');
  const out: string[] = [];
  let prevBlank = false;
  for (const line of lines) {
    if (line.trim() === '') {
      if (out.length > 0 && !prevBlank) {
        out.push('');
        prevBlank = true;
      }
    } else {
      out.push(line);
      prevBlank = false;
    }
  }
  while (out.length > 0 && out[out.length - 1] === '') {
    out.pop();
  }
  return out.join('\n');
}

/**
 * 裸 URL（http/https）识别：
 * - 边界排除空白、尖/方/花括号、引号与括号（含全角 `（）`），避免把正文标点（句号、全角右括号等）吞进链接
 * - 排除中文全角标点（，。；：！？、），防止 URL 后紧跟中文正文时被整体吞入链接
 * - 前缀排除 `(` / `[` / `<`，使已存在于 Markdown 链接语法（`[text](url)` / `<url>`）中的 URL 不被重复处理
 * - 带捕获组（group 1 = URL），供 split 直接切分
 */
const BARE_URL_RE = /(?<![([<])(https?:\/\/[^\s<>{}[\]"'(（）)，。；：！？、]+)/gi;

/** 清理链接尾部常见标点（端口号 / 路径字符不受影响） */
function trimUrlTrailingPunct(url: string): string {
  return url.replace(/[.,;:!?，。；：！？]+$/, '');
}

/**
 * 把文本中的裸 URL（http/https）转换为可点击链接，其余文本原样保留。
 * 用于不经过 Markdown 渲染的用户消息直出场景（会话模式 / 群聊的用户消息），
 * 配合父容器的 white-space: pre-wrap 保留原有换行。
 */
export function linkifyUrls(text: string): React.ReactNode {
  const parts = text.split(BARE_URL_RE);
  if (parts.length === 1) return text;
  return parts.map((part, i) => {
    if (!/^https?:\/\//i.test(part)) return part;
    const clean = trimUrlTrailingPunct(part);
    return (
      <a
        key={i}
        href={clean}
        target="_blank"
        rel="noopener noreferrer"
        // 用 currentColor 继承所在气泡文字色（用户消息气泡背景为 --accent，固定 accent 会与背景同色无法辨别），
        // 用下划线保持链接可辨识。
        style={{ color: 'currentColor', textDecoration: 'underline', wordBreak: 'break-all' }}
      >
        {clean}
      </a>
    );
  });
}

/** 把裸 URL 转成 Markdown 链接语法，替代 remark-gfm autolink（后者不处理全角括号等边界） */
function linkifyUrlsMarkdown(content: string): string {
  return content.replace(BARE_URL_RE, (url) => {
    const clean = trimUrlTrailingPunct(url);
    return `[${clean}](${clean})`;
  });
}

/**
 * 删除列表项之间的空行（loose list → tight list）。
 * 项间空行会使列表解析为 loose list（li 内包裹块级 <p>），导致序号与内容分行、
 * 项间空行放大；删除后 li 直接文本，序号与内容同行、项间紧凑。
 * 供 markdown 渲染与流式占位显示共用。
 *
 * 正则结构说明：整个「列表标记 + 标记后空白」必须都写在 lookahead 内，
 * 替换才以 `\n\n` 为匹配主体成立；若把 `[ \t]+` 移到 lookahead 外，
 * 有序标记（`2.` 数字前无空白）将永远匹配失败，有序项间空行无法删除。
 * `(?=[^\s])` 断言标记后还有非空白内容，避免误删「行尾孤立的列表标记」。
 */
export function tightenListGaps(text: string): string {
  // [-*+•∘○★☆] 覆盖 ASCII 与常见 Unicode bullet；\d+[.)] 覆盖有序列表（1. / 1) / 10. 等）
  const re = new RegExp(
    String.raw`\n\n(?=(?:[ \t]*)(?:[-*+•∘○★☆]|\d+[.)])[ \t]+(?=[^\s]))`,
    'g'
  );
  return text.replace(re, '\n');
}

/** 对正文（非代码围栏）部分做列表与链接处理 */
function processPlainText(part: string): string {
  const tightened = tightenListGaps(part);
  // 识别文件路径与裸 URL 并转成链接
  return linkifyUrlsMarkdown(linkifyPaths(tightened));
}

export function preprocessMarkdown(content: string): string {
  // 折叠连续空行（含 \r\n 归一化、空白行折叠、去首尾空行）
  let result = collapseBlankLines(content);
  // 保护独立的 --- 行（thematic break）：用空行包围，避免被误判为 setext 标题下划线
  result = result.replace(/^(\s*)(-{3,})(\s*)$/gm, '\n\n$1$2$3\n\n');
  // 折叠因保护 --- 而新增/叠加的连续空行
  result = collapseBlankLines(result);
  // 代码围栏块（```…```）内保持原文：列表 tight 化与路径/URL 链接化只作用于正文
  return result
    .split(/(```[\s\S]*?```)/g)
    .map((part, i) => (i % 2 === 1 ? part : processPlainText(part)))
    .join('');
}
