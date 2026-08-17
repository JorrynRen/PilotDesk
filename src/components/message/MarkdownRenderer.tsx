import { memo, useState, useCallback } from 'react';
import { Copy, Check, Eye, Code2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import { showToast } from '../../utils/toast';

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
      className="pd-btn absolute top-2 right-2 p-1 rounded transition-colors"
      style={{
        backgroundColor: copied ? 'var(--accent)' : 'var(--border)',
        color: copied ? '#fff' : 'var(--text-secondary)',
        opacity: 0,
      }}
      title="复制代码"
      onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.opacity = '1'; }}
      onMouseLeave={(e) => { if (!copied) (e.currentTarget as HTMLElement).style.opacity = '0'; }}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

/** 代码块容器：为 HTML/SVG 代码块提供「预览」切换，作为轻量 Artifacts 输出 */
function CodeBlock({ language, codeText, children }: { language: string; codeText: string; children: React.ReactNode }) {
  const [showPreview, setShowPreview] = useState(false);
  const isHtml = language === 'html' || language === 'htm' || language === 'html5' || language === 'xml';
  const isSvg = language === 'svg';
  const previewable = isHtml || isSvg;

  return (
    <div className="group relative">
      {previewable && (
        <button
          onClick={() => setShowPreview((v) => !v)}
          className="pd-btn absolute top-2 right-10 p-1 rounded transition-colors flex items-center gap-0.5"
          style={{
            backgroundColor: showPreview ? 'var(--accent)' : 'var(--border)',
            color: showPreview ? '#fff' : 'var(--text-secondary)',
            opacity: 0,
          }}
          title={showPreview ? '查看代码' : '预览渲染结果'}
          onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.opacity = '1'; }}
          onMouseLeave={(e) => { if (!showPreview) (e.currentTarget as HTMLElement).style.opacity = '0'; }}
        >
          {showPreview ? <Code2 size={12} /> : <Eye size={12} />}
        </button>
      )}
      <CopyButton code={codeText} />
      {previewable && showPreview && (
        <div
          className="rounded-lg overflow-hidden mb-2"
          style={{ border: '1px solid var(--border)', backgroundColor: '#fff' }}
        >
          {isSvg ? (
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
      )}
      <pre
        className="rounded-lg p-3 overflow-x-auto"
        style={{
          backgroundColor: 'var(--bg-tertiary)',
          border: '1px solid var(--border)',
          fontSize: '12px',
          lineHeight: '1.6',
          fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
        }}
      >
        {children}
      </pre>
    </div>
  );
}

// 识别常见文件路径（Windows 盘符绝对路径 / ./ ../ 相对路径），转成可点击的自定义协议链接。
// 路径字符仅允许 ASCII 字母数字与常见路径分隔符（\ / . _ -），不包含空白、括号、标点与中文，
// 从而避免把标点、中文正文误吞进路径（如 "E:\tem。"、"E:\tem）"、"E:\tem目录"）。
// 盘符后加 (?![\\/]) 前瞻排除 URL scheme（如 http:// https://），避免把 URL 误识别为 "p://..." 本地路径。
const PATH_LINK_RE = /(`?)((?:[A-Za-z]:[\\/](?![\\/])|\.{1,2}[\\/])[A-Za-z0-9\\/._\-]+)\1/g;

function linkifyPaths(content: string): string {
  return content.replace(PATH_LINK_RE, (full, tick, path) => {
    // 去掉尾部标点，避免把句号/逗号吞进路径
    const clean = path.replace(/[.,;:!?，。；：！？]+$/, '');
    if (!clean || clean.length < 2) return full;
    const label = tick ? `\`${clean}\`` : clean;
    return `[${label}](pilotdesk-path://${encodeURIComponent(clean)})`;
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
 * - 前缀排除 `(` / `[` / `<`，使已存在于 Markdown 链接语法（`[text](url)` / `<url>`）中的 URL 不被重复处理
 * - 带捕获组（group 1 = URL），供 split 直接切分
 */
const BARE_URL_RE = /(?<![(\[<])(https?:\/\/[^\s<>{}[\]"'(（）)]+)/gi;

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
        style={{ color: 'var(--accent)', textDecoration: 'underline' }}
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
  // 去除 fenced code block 内容首行的前导空白（部分模型输出会在代码块首行带缩进）
  result = result.replace(/(```[^\n]*\n)[ \t]+(?=\S)/g, '$1');
  // 识别文件路径并转成可点击链接
  result = linkifyPaths(result);
  // 裸 URL 转成 Markdown 链接（统一处理全角括号等 gfm autolink 不覆盖的边界）
  return linkifyUrlsMarkdown(result);
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
          if (/^(https?:|mailto:|tel:)/i.test(url)) return url;
          if (url.startsWith('#') || url.startsWith('/') || url.startsWith('./') || url.startsWith('../')) return url;
          return '';
        }}
        components={{
          code({ className, children, ...props }) {
            const isInline = !className;
            if (isInline) {
              return (
                <code
                  className="px-1 py-0.5 rounded"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
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
                style={{ color: 'var(--accent)' }}
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
            return (
              <li style={{ margin: '2px 0', whiteSpace: 'pre-wrap' }} {...props}>
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
