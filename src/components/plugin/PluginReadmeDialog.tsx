import { useEffect, useState, useRef } from 'react';
import { X, FileText, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface Props {
  /** 本地插件 ID 或远程 baseUrl */
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
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    (async () => {
      try {
        // 前端超时兜底（10 秒）
        const timeoutPromise = new Promise<never>((_, reject) => {
          timerRef.current = setTimeout(() => reject(new Error('请求超时（10秒）')), 10000);
        });

        const readPromise = invoke<string>('read_plugin_readme', {
          pluginId: basePath,
          isRemote: !!isRemote,
          remoteUrl: isRemote ? basePath : null,
        });

        const text = await Promise.race([readPromise, timeoutPromise]);
        setContent(text);
      } catch (err) {
        const msg = String(err);
        if (msg.includes('NOT_FOUND') || msg.includes('HTTP 404') || msg.includes('404')) {
          setNotFound(true);
        } else if (msg.includes('超时') || msg.includes('timeout') || msg.includes('TimedOut')) {
          setError('请求超时，作者未提供 README 文档或网络不可用');
        } else if (msg.includes('连接失败') || msg.includes('connect')) {
          setError('网络连接失败，无法获取 README 文档');
        } else {
          setError('读取 README 失败: ' + msg);
        }
      } finally {
        if (timerRef.current) {
          clearTimeout(timerRef.current);
          timerRef.current = null;
        }
        setLoading(false);
      }
    })();

    return () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
    };
  }, [basePath, isRemote]);

  function renderMarkdown(text: string): string {
    // 1. 代码块（必须最先处理，避免内部内容被后续规则污染）
    let html = text.replace(/```(\w*)[\s\S]*?```/g, (match) => {
      const lang = match.match(/```(\w*)/)?.[1] || '';
      let code = match.replace(/```\w*\n?/, '').replace(/```$/, '');
      // 去掉代码块内部的所有空行（逐行过滤，不留任何空行）
      code = code.split('\n').filter(line => line.trim() !== '').join('\n');
      return '<pre><code' + (lang ? ' class="language-' + lang + '"' : '') + '>' + escapeHtml(code.trim()) + '</code></pre>';
    });

    // 2. 行内代码
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');

    // 3. 粗体
    html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');

    // 4. 链接
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');

    // 5. 标题
    html = html.replace(/^### (.+)$/gm, '<h4>$1</h4>');
    html = html.replace(/^## (.+)$/gm, '<h3>$1</h3>');
    html = html.replace(/^# (.+)$/gm, '<h2>$1</h2>');

    // 6. 无序列表
    html = html.replace(/^[-*] (.+)$/gm, '<li>$1</li>');

    // 7. 段落：将连续的非空行包裹在 <p> 中
    const lines = html.split('\n');
    const result: string[] = [];
    let inParagraph = false;
    for (const line of lines) {
      const trimmed = line.trim();
      if (trimmed === '') {
        if (inParagraph) { result.push('</p>'); inParagraph = false; }
        continue;
      }
      // 已经是 HTML 标签的行，直接保留
      if (trimmed.startsWith('<h') || trimmed.startsWith('<li') || trimmed.startsWith('<pre') || trimmed.startsWith('</pre') || trimmed.startsWith('<ul') || trimmed.startsWith('</ul') || trimmed.startsWith('<ol') || trimmed.startsWith('</ol')) {
        if (inParagraph) { result.push('</p>'); inParagraph = false; }
        result.push(line);
        continue;
      }
      if (trimmed.startsWith('<') && trimmed.endsWith('>')) {
        if (inParagraph) { result.push('</p>'); inParagraph = false; }
        result.push(line);
        continue;
      }
      // 普通文本行，包裹在 <p> 中
      if (!inParagraph) { result.push('<p>'); inParagraph = true; }
      else { result.push(' '); }
      result.push(line);
    }
    if (inParagraph) result.push('</p>');

    return result.join('\n');
  }

  function escapeHtml(str: string): string {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
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
              <p>作者未提供 README 文档</p>
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
