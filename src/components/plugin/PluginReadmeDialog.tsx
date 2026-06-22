import { useEffect, useState } from 'react';
import { X, FileText, Loader2 } from 'lucide-react';
import { convertFileSrc } from '@tauri-apps/api/core';

interface Props {
  /** 本地插件路径或远程 baseUrl */
  basePath: string;
  /** 插件名称（用于标题） */
  pluginName: string;
  /** 是否为远程 URL */
  isRemote?: boolean;
  onClose: () => void;
}

export function PluginReadmeDialog({ basePath, pluginName, isRemote, onClose }: Props) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const readmeUrl = isRemote
          ? basePath.replace(/\/+$/, '') + '/README.md'
          : basePath.replace(/\\+$/, '') + '/README.md';

        let text: string;
        if (isRemote) {
          const resp = await fetch(readmeUrl);
          if (!resp.ok) throw new Error('HTTP ' + resp.status);
          text = await resp.text();
        } else {
          // 本地文件：通过 Tauri asset 协议读取
          const resp = await fetch(convertFileSrc(readmeUrl));
          if (!resp.ok) throw new Error('HTTP ' + resp.status);
          text = await resp.text();
        }
        setContent(text);
      } catch (err) {
        const msg = String(err);
        if (msg.includes('No such file') || msg.includes('找不到') || msg.includes('NotFound') || msg.includes('HTTP 404') || msg.includes('Failed to fetch')) {
          setNotFound(true);
        } else {
          setError('读取 README 失败: ' + msg);
        }
      } finally {
        setLoading(false);
      }
    })();
  }, [basePath, isRemote]);

  function renderMarkdown(text: string): string {
    return text
      .replace(/\`\`\`(\w*)[\s\S]*?\`\`\`/g, '<pre><code>$1</code></pre>')
      .replace(/\`([^\`]+)\`/g, '<code>$1</code>')
      .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
      .replace(/^### (.+)$/gm, '<h4>$1</h4>')
      .replace(/^## (.+)$/gm, '<h3>$1</h3>')
      .replace(/^# (.+)$/gm, '<h2>$1</h2>')
      .replace(/^[-*] (.+)$/gm, '<li>$1</li>')
      .replace(/\n/g, '<br/>');
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[600px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="flex items-center gap-2 min-w-0">
            <FileText size={16} style={{ color: 'var(--accent)' }} />
            <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>
              {pluginName} - 说明文档
            </span>
          </div>
          <button onClick={onClose} className="pd-btn px-1.5 py-1 rounded text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            <X size={14} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto px-5 py-4">
          {loading && (
            <div className="flex items-center justify-center py-12">
              <Loader2 size={20} className="pd-animate-spin" style={{ color: 'var(--text-tertiary)' }} />
            </div>
          )}
          {notFound && (
            <div className="text-xs py-12 text-center" style={{ color: 'var(--text-tertiary)' }}>
              <FileText size={24} className="mx-auto mb-3" style={{ opacity: 0.4 }} />
              <p>作者未提供 README 介绍文件</p>
            </div>
          )}
          {error && (
            <div className="text-xs py-4 text-center" style={{ color: '#EF4444' }}>{error}</div>
          )}
          {content && (
            <div
              className="readme-content text-xs leading-relaxed"
              style={{ color: 'var(--text-primary)' }}
              dangerouslySetInnerHTML={{ __html: renderMarkdown(content) }}
            />
          )}
        </div>
      </div>
    </div>
  );
}
