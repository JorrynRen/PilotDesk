import { useState, useRef, useEffect, useCallback } from 'react';
import { Send, Square, Zap, Brain, GraduationCap, Lightbulb, Cpu } from 'lucide-react';
import type { ChatMode, Session } from '../../types';
import { MODE_LABELS, MODE_COLORS } from '../../types';
import { InspirationPicker } from '../input/InspirationPicker';
import { SkillPicker } from '../input/SkillPicker';
import { showToast } from '../../utils/toast';

interface InputBarProps {
  session: Session | null;
  onSend: (message: string, mode: ChatMode) => void;
  onStop?: () => void;
  isGenerating?: boolean;
  pendingInput?: string | null;
  onPendingConsumed?: () => void;
}

const MODE_ICONS: Record<ChatMode, typeof Zap> = {
  native: Send,
  fast: Zap,
  think: Brain,
  expert: GraduationCap,
};

export function InputBar({ session, onSend, onStop, isGenerating, pendingInput, onPendingConsumed }: InputBarProps) {
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<ChatMode>('native');
  const [showInspirationPicker, setShowInspirationPicker] = useState(false);
  const [showSkillPicker, setShowSkillPicker] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const pickerAnchorRef = useRef<HTMLDivElement>(null);

  // Consume pending input
  useEffect(() => {
    if (pendingInput) {
      setInput(pendingInput);
      onPendingConsumed?.();
      textareaRef.current?.focus();
    }
  }, [pendingInput, onPendingConsumed]);

  // Auto-resize textarea
  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = 'auto';
      textarea.style.height = Math.min(textarea.scrollHeight, 200) + 'px';
    }
  }, [input]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Ctrl+I for inspiration picker
      if (e.ctrlKey && e.key === 'i') {
        e.preventDefault();
        setShowInspirationPicker((v) => !v);
        setShowSkillPicker(false);
        return;
      }
      // Ctrl+K for skill picker
      if (e.ctrlKey && e.key === 'k') {
        e.preventDefault();
        setShowSkillPicker((v) => !v);
        setShowInspirationPicker(false);
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSendInternal();
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [input, mode]
  );

  const handleSendInternal = useCallback(() => {
    const trimmed = input.trim();
    if (!trimmed) return;
    if (!session) {
      showToast('请先选择或创建一个会话', 'info');
      return;
    }
    onSend(trimmed, mode);
    setInput('');
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, mode, session, onSend]);

  // Placeholder based on context
  const placeholder = !session
    ? '选择会话开始对话...'
    : session.agentType === 'claude'
    ? '向 Claude Code 发送消息... (Ctrl+I 灵感, Ctrl+K 技能)'
    : '向 Hermes Agent 发送消息... (Ctrl+I 灵感, Ctrl+K 技能)';

  const getModeBorderColor = (m: ChatMode, isActive: boolean) => {
    if (!isActive) return '1px solid transparent';
    const color = MODE_COLORS[m];
    return `1px solid ${color}44`;
  };

  return (
    <div className="shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
      {/* Mode selector */}
      <div className="flex items-center gap-1 px-4 pt-2">
        {(Object.keys(MODE_LABELS) as ChatMode[]).map((m) => {
          const Icon = MODE_ICONS[m];
          const isActive = mode === m;
          return (
            <button
              key={m}
              onClick={() => setMode(m)}
              className="flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] transition-all"
              style={{
                backgroundColor: isActive ? `${MODE_COLORS[m]}22` : 'transparent',
                color: isActive ? MODE_COLORS[m] : 'var(--text-secondary)',
                border: getModeBorderColor(m, isActive),
              }}
              title={MODE_LABELS[m]}
            >
              <Icon size={11} />
              {MODE_LABELS[m]}
            </button>
          );
        })}
      </div>

      {/* Input area */}
      <div className="flex items-end gap-2 px-4 py-2" ref={pickerAnchorRef}>
        <div className="flex-1 relative">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            rows={1}
            className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none"
            style={{
              backgroundColor: 'var(--bg-secondary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
              minHeight: '36px',
              maxHeight: '200px',
            }}
          />
          {/* Picker panels */}
          {showInspirationPicker && (
            <InspirationPicker
              onSelect={(content) => {
                setInput((prev) => prev + content);
                setShowInspirationPicker(false);
              }}
              onClose={() => setShowInspirationPicker(false)}
            />
          )}
          {showSkillPicker && (
            <SkillPicker
              agentType={session?.agentType ?? 'claude'}
              onSelect={(content) => {
                setInput((prev) => prev + content);
                setShowSkillPicker(false);
              }}
              onClose={() => setShowSkillPicker(false)}
            />
          )}
        </div>

        {/* Action buttons */}
        <div className="flex items-center gap-1 shrink-0 pb-0.5">
          <button
            onClick={() => setShowInspirationPicker((v) => !v)}
            className="p-2 rounded-lg transition-colors"
            style={{
              color: showInspirationPicker ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: showInspirationPicker ? 'var(--border)' : 'transparent',
            }}
            title="灵感搜索 (Ctrl+I)"
          >
            <Lightbulb size={16} />
          </button>
          <button
            onClick={() => setShowSkillPicker((v) => !v)}
            className="p-2 rounded-lg transition-colors"
            style={{
              color: showSkillPicker ? 'var(--hermes-tag)' : 'var(--text-secondary)',
              backgroundColor: showSkillPicker ? 'var(--border)' : 'transparent',
            }}
            title="技能列表 (Ctrl+K)"
          >
            <Cpu size={16} />
          </button>
          {isGenerating ? (
            <button
              onClick={onStop}
              className="p-2 rounded-lg transition-colors"
              style={{ backgroundColor: '#EF444422', color: '#EF4444' }}
              title="停止生成"
            >
              <Square size={16} />
            </button>
          ) : (
            <button
              onClick={handleSendInternal}
              disabled={!input.trim() || !session}
              className="p-2 rounded-lg transition-colors disabled:opacity-30"
              style={{
                backgroundColor: input.trim() && session ? 'var(--accent)' : 'var(--border)',
                color: input.trim() && session ? '#fff' : 'var(--text-secondary)',
              }}
              title="发送"
            >
              <Send size={16} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
