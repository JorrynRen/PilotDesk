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
      // Fallback for non-secure contexts
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
    <div className="prose prose-sm max-w-none" style={{ color: 'var(--text-primary)' }}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={{
          code({ className, children, ...props }) {
            const isInline = !className;
            if (isInline) {
              return (
                <code
                  className="px-1 py-0.5 rounded text-xs"
                  style={{
                    backgroundColor: 'var(--bg-secondary)',
                    color: 'var(--accent)',
                  }}
                  {...props}
                >
                  {children}
                </code>
              );
            }
            return (
              <code
                className={`${className || ''} text-xs`}
                style={{ backgroundColor: 'var(--bg-secondary)' }}
              >
                {children}
              </code>
            );
          },
          pre({ children, ...props }) {
            // Extract code text for copy button
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
                  className="rounded-md p-3 text-xs overflow-x-auto"
                  style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                  {...props}
                >
                  {children}
                </pre>
              </div>
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
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
