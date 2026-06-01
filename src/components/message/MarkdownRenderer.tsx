import { useState, useCallback } from 'react';
import { Copy, Check } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';

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
      className="absolute top-2 right-2 p-1 rounded transition-colors"
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

export function MarkdownRenderer({ content }: MarkdownRendererProps) {
  return (
    <div className="pilotdesk-markdown" style={{ color: 'var(--text-primary)' }}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={{
          code({ className, children, ...props }) {
            const isInline = !className;
            if (isInline) {
              return (
                <code
                  className="px-1 py-0.5 rounded"
                  style={{
                    backgroundColor: 'var(--bg-secondary)',
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
            return (
              <code
                className={`${className || ''}`}
                style={{ backgroundColor: 'transparent', fontSize: '12px' }}
              >
                {children}
              </code>
            );
          },
          pre({ children, ...props }) {
            let codeText = '';
            const child = Array.isArray(children) ? children[0] : children;
            if (child && typeof child === 'object' && 'props' in child) {
              const childProps = child.props as { children?: React.ReactNode };
              codeText = typeof childProps?.children === 'string'
                ? childProps.children.replace(/\n$/, '')
                : '';
            }
            return (
              <div className="group relative">
                <CopyButton code={codeText} />
                <pre
                  className="rounded-lg p-3 overflow-x-auto"
                  style={{
                    backgroundColor: 'var(--bg-secondary)',
                    border: '1px solid var(--border)',
                    fontSize: '12px',
                    lineHeight: '1.6',
                    fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
                  }}
                  {...props}
                >
                  {children}
                </pre>
              </div>
            );
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
              <p style={{ margin: '4px 0', lineHeight: '1.7' }} {...props}>
                {children}
              </p>
            );
          },
          ul({ children, ...props }) {
            return (
              <ul style={{ margin: '4px 0', paddingLeft: '20px', lineHeight: '1.7' }} {...props}>
                {children}
              </ul>
            );
          },
          ol({ children, ...props }) {
            return (
              <ol style={{ margin: '4px 0', paddingLeft: '20px', lineHeight: '1.7' }} {...props}>
                {children}
              </ol>
            );
          },
          li({ children, ...props }) {
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
        {content}
      </ReactMarkdown>
    </div>
  );
}
