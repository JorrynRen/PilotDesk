import { memo, useState, useCallback } from 'react';
import { Copy, Check, Eye, Code2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import { showToast } from '../../utils/toast';
import { useImagePreviewStore } from '../../stores/imagePreviewStore';

interface MarkdownRendererProps {
  content: string;
}

function CopyButton({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = code;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [code]);

  return (
    <button
      onClick={handleCopy}
      className="pd-btn p-1 rounded transition-colors"
      style={{
        backgroundColor: copied ? 'var(--accent)' : 'transparent',
        color: copied ? '#fff' : 'var(--text-secondary)',
      }}
      title={copied ? '已复制' : '复制代码'}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

/** 语言别名归一化：py→python、yml→yaml、sh/zsh→bash 等，供徽标着色使用 */
function normalizeLang(lang: string): string {
  const alias: Record<string, string> = {
    py: 'python',
    yml: 'yaml',
    sh: 'bash', zsh: 'bash', shell: 'bash',
    js: 'javascript', jsx: 'javascript',
    ts: 'typescript', tsx: 'typescript',
    cc: 'cpp',
    htm: 'html', html5: 'html',
    rs: 'rust',
    golang: 'go',
    jsonc: 'json',
    docker: 'dockerfile',
    scss: 'css', less: 'css',
  };
  return alias[lang] ?? lang;
}

/** 语言品牌主色（GitHub 徽标风格）：未收录语言回退为中性灰 */
const LANG_COLORS: Record<string, string> = {
  javascript: '#f7df1e',
  typescript: '#3178c6',
  python: '#3776ab',
  rust: '#dea584',
  go: '#00add8',
  java: '#e76f00',
  c: '#00599c',
  cpp: '#00599c',
  html: '#e34f26',
  css: '#1572b6',
  json: '#7bc500',
  bash: '#4eaa25',
  sql: '#e38c00',
  yaml: '#cb171e',
  dockerfile: '#2496ed',
};

/** 代码块容器：为 HTML/SVG/Markdown 代码块提供「预览」切换，作为轻量 Artifacts 输出 */
function CodeBlock({ language, codeText, children }: { language: string; codeText: string; children: React.ReactNode }) {
  const [showPreview, setShowPreview] = useState(false);
  const isHtml = language === 'html' || language === 'htm' || language === 'html5' || language === 'xml';
  const isSvg = language === 'svg';
  const isMarkdown = language === 'markdown' || language === 'md' || language === 'mdx';
  const previewable = isHtml || isSvg || isMarkdown;

  return (
    <div
      className="group relative my-2 overflow-hidden"
      style={{ border: '1px solid var(--border)' }}
    >
      {/* 头部栏：语言标注 + 操作按钮（固定布局，不遮挡代码内容） */}
      <div
        className="flex items-center gap-1.5 px-2 h-7"
        style={{ backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
      >
        {language && (
          (() => {
            const langColor = LANG_COLORS[normalizeLang(language)] ?? '';
            return (
              <span
                className="text-[10px] px-1 py-px rounded select-none"
                style={{
                  color: langColor || 'var(--text-tertiary)',
                  backgroundColor: langColor ? `${langColor}22` : 'var(--bg-tertiary)',
                  border: langColor ? `1px solid ${langColor}44` : 'none',
                }}
              >
                {language}
              </span>
            );
          })()
        )}
        <div className="flex-1" />
        {previewable && (
          <button
            onClick={() => setShowPreview((v) => !v)}
            className="pd-btn p-1 rounded transition-colors flex items-center gap-0.5"
            style={{
              backgroundColor: showPreview ? 'var(--accent)' : 'transparent',
              color: showPreview ? '#fff' : 'var(--text-secondary)',
            }}
            title={showPreview ? '查看代码' : '预览渲染结果'}
          >
            {showPreview ? <Code2 size={12} /> : <Eye size={12} />}
          </button>
        )}
        <CopyButton code={codeText} />
      </div>
      {/* 预览时只显示渲染结果（隐藏源码）；未预览/不可预览时显示源码 */}
      {previewable && showPreview ? (
        <div style={{ backgroundColor: isMarkdown ? 'var(--bg-primary)' : '#fff' }}>
          {isMarkdown ? (
            // Markdown 预览：等价于"去掉围栏标记"，复用主渲染器按正文渲染（样式与消息主体完全一致）
            <div className="px-3 py-2">
              <MarkdownRenderer content={codeText} />
            </div>
          ) : isSvg ? (
            <div
              className="w-full flex items-center justify-center p-2"
              dangerouslySetInnerHTML={{ __html: codeText }}
            />
          ) : (
            <iframe
              title="artifact-preview"
              sandbox="allow-scripts"
              srcDoc={codeText}
              className="w-full block"
              style={{ height: 360, border: 'none' }}
            />
          )}
        </div>
      ) : (
        <div
          className="flex"
          style={{ fontSize: '12px', lineHeight: '1.6', fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace" }}
        >
          {/* 行号列：编辑器风格，不随代码横向滚动 */}
          <div
            className="shrink-0 select-none text-right"
            style={{
              color: 'var(--text-tertiary)',
              backgroundColor: 'var(--bg-tertiary)',
              padding: '12px 8px 12px 12px',
              borderRight: '1px solid var(--border)',
            }}
          >
            {codeText.split('\n').map((_, i) => (
              <div key={i} style={{ fontSize: '12px', lineHeight: '1.6' }}>{i + 1}</div>
            ))}
          </div>
          <pre
            className="p-3 overflow-x-auto m-0 flex-1"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              fontSize: '12px',
              lineHeight: '1.6',
              fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
            }}
          >
            {children}
          </pre>
        </div>
      )}
    </div>
  );
}

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
const PATH_LINK_RE = /(`?)((?<![(\[<])(?:[A-Za-z]:[\\/](?![\\/])|\.{1,2}[\\/])[^\s\[\](){}<>"'`，。；：！？,;:!?]*[\\/][^\s\[\](){}<>"'`，。；：！？,;:!?]*\.[A-Za-z][A-Za-z0-9]{0,5})\1/g;

function linkifyPaths(content: string): string {
  return content.replace(PATH_LINK_RE, (full, tick, path) => {
    // 正则已排除边界标点与空白，无需二次裁剪
    if (!path || path.length < 2) return full;
    const label = tick ? `\`${path}\`` : path;
    return `[${label}](pilotdesk-path://${encodeURIComponent(path)})`;
  });
}

// 从 React children 递归还原纯文本（rehype-highlight 会把代码拆成 <span> 树，需遍历提取）
function extractText(node: React.ReactNode): string {
  if (node == null || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(extractText).join('');
  if (typeof node === 'object' && node !== null && 'props' in node) {
    const props = (node as { props: { children?: React.ReactNode } }).props;
    return extractText(props.children);
  }
  return '';
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
const BARE_URL_RE = /(?<![(\[<])(https?:\/\/[^\s<>{}[\]"'(（）)，。；：！？、]+)/gi;

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

function preprocessMarkdown(content: string): string {
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


export const MarkdownRenderer = memo(function MarkdownRenderer({ content }: MarkdownRendererProps) {
  const processedContent = preprocessMarkdown(content);
  return (
    <div className="pilotdesk-markdown" style={{ color: 'var(--text-primary)' }}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        urlTransform={(url) => {
          // 放行自定义路径协议，并保留安全的外部/相对链接，其余（javascript: 等）置空
          if (url.startsWith('pilotdesk-path:')) return url;
          if (/^(https?:|mailto:|tel:|data:)/i.test(url)) return url;
          if (url.startsWith('#') || url.startsWith('/') || url.startsWith('./') || url.startsWith('../')) return url;
          return '';
        }}
        components={{
          img({ src, alt, ...props }) {
            // 视频：工具返回 `![视频](<url>)`（或视频扩展名 URL）→ 渲染可播放的视频控件
            const isVideo = alt === '视频' || /\.(mp4|webm|mov|m4v)([?#]|$)/i.test(src || '');
            if (isVideo && src) {
              return (
                <video
                  src={src}
                  controls
                  preload="metadata"
                  style={{ maxWidth: '100%', borderRadius: 8, display: 'block' }}
                />
              );
            }
            // Markdown 图片：点击放大（全局 ImagePreview），保留悬浮提示与圆角样式
            return (
              <img
                src={src}
                alt={alt}
                title={alt}
                onClick={(e) => {
                  e.stopPropagation();
                  if (src) useImagePreviewStore.getState().open(src);
                }}
                style={{ maxWidth: '100%', borderRadius: 8, cursor: 'zoom-in', display: 'block' }}
                {...props}
              />
            );
          },
          code({ className, children, ...props }) {
            const isInline = !className;
            if (isInline) {
              return (
                <code
                  style={{
                    color: 'var(--accent)',
                    fontSize: 'inherit',
                    fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
                  }}
                  {...props}
                >
                  {children}
                </code>
              );
            }
            const cls = Array.isArray(className)
              ? (className as unknown as string[]).join(' ')
              : (className || '');
            return (
              <code
                className={cls || undefined}
                style={{ backgroundColor: 'transparent', fontSize: '12px' }}
              >
                {children}
              </code>
            );
          },
          pre({ children }) {
            let codeText = '';
            let language = '';
            const child = Array.isArray(children) ? children[0] : children;
            if (child && typeof child === 'object' && 'props' in child) {
              const childProps = child.props as { children?: React.ReactNode; className?: string | string[] };
              codeText = extractText(childProps?.children).replace(/\n$/, '');
              const cls = Array.isArray(childProps?.className)
                ? childProps.className.join(' ')
                : (childProps?.className || '');
              const m = cls.match(/language-([\w-]+)/);
              language = m ? m[1].toLowerCase() : '';
            }
            return <CodeBlock language={language} codeText={codeText}>{children}</CodeBlock>;
          },
          table({ children, ...props }) {
            return (
              <div style={{ overflowX: 'auto', margin: '8px 0' }}>
                <table
                  style={{
                    borderCollapse: 'collapse',
                    width: '100%',
                    fontSize: '13px',
                    lineHeight: '1.6',
                    border: '1px solid var(--border)',
                  }}
                  {...props}
                >
                  {children}
                </table>
              </div>
            );
          },
          thead({ children, ...props }) {
            return (
              <thead
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  borderBottom: '2px solid var(--border)',
                }}
                {...props}
              >
                {children}
              </thead>
            );
          },
          th({ children, ...props }) {
            return (
              <th
                style={{
                  padding: '6px 12px',
                  textAlign: 'left',
                  fontWeight: 600,
                  fontSize: '12px',
                  borderRight: '1px solid var(--border)',
                  color: 'var(--text-primary)',
                }}
                {...props}
              >
                {children}
              </th>
            );
          },
          td({ children, ...props }) {
            return (
              <td
                style={{
                  padding: '6px 12px',
                  borderRight: '1px solid var(--border)',
                  borderBottom: '1px solid var(--border)',
                  fontSize: '13px',
                }}
                {...props}
              >
                {children}
              </td>
            );
          },
          tr({ children, ...props }) {
            return (
              <tr
                style={{ borderBottom: '1px solid var(--border)' }}
                {...props}
              >
                {children}
              </tr>
            );
          },
          a({ href, children, ...props }) {
            if (href && href.startsWith('pilotdesk-path:')) {
              const raw = href.slice('pilotdesk-path://'.length);
              const path = decodeURIComponent(raw);
              return (
                <a
                  href="#"
                  onClick={(e) => {
                    e.preventDefault();
                    invoke('open_path', { path }).catch((err) => {
                      showToast(String(err), 'error');
                    });
                  }}
                  style={{
                    color: 'var(--accent)',
                    textDecoration: 'underline',
                    cursor: 'pointer',
                    wordBreak: 'break-all',
                    fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
                  }}
                  {...props}
                >
                  {children}
                </a>
              );
            }
            return (
              <a
                href={href}
                target="_blank"
                rel="noopener noreferrer"
                style={{ color: 'var(--accent)', wordBreak: 'break-all' }}
                {...props}
              >
                {children}
              </a>
            );
          },
          p({ children, ...props }) {
            return (
              <p style={{ margin: 0, lineHeight: '1.625', fontSize: '13px' }} {...props}>
                {children}
              </p>
            );
          },
          ul({ children, ...props }) {
            return (
              <ul style={{ margin: '4px 0', paddingLeft: '20px', lineHeight: '1.625', fontSize: '13px' }} {...props}>
                {children}
              </ul>
            );
          },
          ol({ children, ...props }) {
            return (
              <ol style={{ margin: '4px 0', paddingLeft: '20px', lineHeight: '1.625', fontSize: '13px' }} {...props}>
                {children}
              </ol>
            );
          },
          li({ children, ...props }) {
            // 不要设置 white-space: pre-wrap：react-markdown 会在块级子元素（含嵌套列表 li）之间
            // 输出换行文本节点，pre-wrap 会把这些换行渲染为可见空行——这正是嵌套列表项间空行的根源。
            // 外层 .pilotdesk-markdown 为 white-space: normal，会把换行折叠为空格，列表项保持紧凑。
            return (
              <li style={{ margin: '2px 0' }} {...props}>
                {children}
              </li>
            );
          },
          h1({ children, ...props }) {
            return <h1 style={{ fontSize: '18px', fontWeight: 700, margin: '12px 0 6px' }} {...props}>{children}</h1>;
          },
          h2({ children, ...props }) {
            return <h2 style={{ fontSize: '16px', fontWeight: 700, margin: '10px 0 4px' }} {...props}>{children}</h2>;
          },
          h3({ children, ...props }) {
            return <h3 style={{ fontSize: '14px', fontWeight: 600, margin: '8px 0 4px' }} {...props}>{children}</h3>;
          },
          blockquote({ children, ...props }) {
            return (
              <blockquote
                style={{
                  margin: '6px 0',
                  padding: '6px 12px',
                  borderLeft: '3px solid var(--accent)',
                  backgroundColor: 'var(--bg-tertiary)',
                  borderRadius: '0 6px 6px 0',
                  color: 'var(--text-secondary)',
                }}
                {...props}
              >
                {children}
              </blockquote>
            );
          },
          hr({ ...props }) {
            return <hr style={{ border: 'none', borderTop: '1px solid var(--border)', margin: '12px 0' }} {...props} />;
          },
        }}
      >
        {processedContent}
      </ReactMarkdown>
    </div>
  );
});
