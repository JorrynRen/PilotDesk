import { memo, useState, useCallback } from 'react';
import { Copy, Check, Eye, Code2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useImagePreviewStore } from '../../stores/imagePreviewStore';
import { preprocessMarkdown } from './markdownText';

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
                      showToast(errorMessage(err), 'error');
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
