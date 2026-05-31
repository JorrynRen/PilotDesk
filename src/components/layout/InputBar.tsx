import { useState, useRef, useEffect, useCallback } from 'react';
import { Send, Square, Zap, Brain, GraduationCap } from 'lucide-react';
import type { ChatMode, Session } from '../../types';
import { MODE_LABELS, MODE_COLORS } from '../../types';

interface InputBarProps {
  session: Session | null;
  onSend: (message: string, mode: ChatMode) => void;
  onStop?: () => void;
  isGenerating?: boolean;
}

const MODE_ICONS: Record<ChatMode, typeof Zap> = {
  native: Send,
  fast: Zap,
  think: Brain,
  expert: GraduationCap,
};

export function InputBar({ session, onSend, onStop, isGenerating }: InputBarProps) {
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<ChatMode>('native');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [input, mode]
  );

  const handleSend = useCallback(() => {
    const trimmed = input.trim();
    if (!trimmed || !session) return;
    onSend(trimmed, mode);
    setInput('');
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, mode, session, onSend]);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
      textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 200)}px`;
    }
  }, [input]);

  return (
    <div className="px-4 py-3" style={{ borderTop: '1px solid var(--border)' }}>
      {/* Mode selector */}
      <div className="flex items-center gap-1 mb-2">
        {(Object.keys(MODE_LABELS) as ChatMode[]).map((m) => {
          const Icon = MODE_ICONS[m];
          const isActive = mode === m;
          return (
            <button
              key={m}
              onClick={() => setMode(m)}
              className="flex items-center gap-1 px-2 py-0.5 rounded text-xs transition-colors"
              style={{
                color: isActive ? MODE_COLORS[m] : 'var(--text-secondary)',
                backgroundColor: isActive ? 'var(--border)' : 'transparent',
              }}
            >
              <Icon size={11} />
              {MODE_LABELS[m]}
            </button>
          );
        })}
      </div>

      {/* Input area */}
      <div className="flex items-end gap-2">
        <div
          className="flex-1 rounded-lg px-3 py-2 text-sm overflow-hidden"
          style={{
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
          }}
        >
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={
              session
                ? `发送给 ${session.agentType === 'claude' ? 'Claude Code' : 'Hermes Agent'}... (Enter 发送, Shift+Enter 换行)`
                : '选择一个会话开始对话'
            }
            disabled={!session}
            className="w-full bg-transparent outline-none resize-none text-sm leading-relaxed"
            style={{
              color: 'var(--text-primary)',
              minHeight: '24px',
              maxHeight: '200px',
            }}
            rows={1}
          />
        </div>

        {isGenerating ? (
          <button
            onClick={onStop}
            className="p-2 rounded-lg transition-colors"
            style={{ backgroundColor: '#EF4444', color: '#fff' }}
            title="停止生成"
          >
            <Square size={16} />
          </button>
        ) : (
          <button
            onClick={handleSend}
            disabled={!session || !input.trim()}
            className="p-2 rounded-lg transition-colors disabled:opacity-30"
            style={{
              backgroundColor: 'linear-gradient(135deg, #5B7FFF, #8B5CF6)',
              color: '#fff',
            }}
            title="发送"
          >
            <Send size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
