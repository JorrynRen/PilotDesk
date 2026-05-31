import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';

interface MarkdownRendererProps {
  content: string;
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
                {...props}
              >
                {children}
              </code>
            );
          },
          pre({ children, ...props }) {
            return (
              <pre
                className="rounded-md p-3 text-xs overflow-x-auto"
                style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                {...props}
              >
                {children}
              </pre>
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
