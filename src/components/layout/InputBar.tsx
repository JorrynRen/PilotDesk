import { useState, useRef, useEffect, useCallback } from 'react';
import { Send, Square, Zap, Brain, GraduationCap, Lightbulb, Cpu, ChevronUp } from 'lucide-react';
import type { ChatMode, Session } from '../../types';
import { MODE_LABELS, MODE_COLORS, MODE_PROMPTS_DEFAULTS, getModePrompt } from '../../types';
import { InspirationPicker } from '../input/InspirationPicker';
import { SkillPicker } from '../input/SkillPicker';
import { showToast } from '../../utils/toast';
import { AGENT_THEMES } from '../../types';


const MODE_ICONS: Record<ChatMode, typeof Send> = {
  native: Send,
  fast: Zap,
  think: Brain,
  expert: GraduationCap,
};

interface InputBarProps {
  session: Session | null;
  onSend: (message: string, mode: ChatMode) => void;
  onStop?: () => void;
  isGenerating?: boolean;
  pendingInput?: string | null;
  onPendingConsumed?: () => void;
}

export function InputBar({ session, onSend, onStop, isGenerating, pendingInput, onPendingConsumed }: InputBarProps) {
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<ChatMode>('native');
  const [showInspirationPicker, setShowInspirationPicker] = useState(false);
  const [showSkillPicker, setShowSkillPicker] = useState(false);
  const [showModeDropdown, setShowModeDropdown] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const pickerAnchorRef = useRef<HTMLDivElement>(null);
  const modeDropdownRef = useRef<HTMLDivElement>(null);

  // Load prompt descriptions for tooltip
  const [modeDescriptions, setModeDescriptions] = useState<Record<string, string>>({});
  useEffect(() => {
    (async () => {
      const descs: Record<string, string> = {};
      for (const m of ['native', 'fast', 'think', 'expert'] as const) {
        const p = await getModePrompt(m);
        if (p && p.trim() !== '') descs[m] = p;
        else descs[m] = '原生模式，使用默认对话风格';
      }
      setModeDescriptions(descs);
    })();
  }, []);
  useEffect(() => {
    if (pendingInput) {
      setInput(pendingInput);
      onPendingConsumed?.();
      textareaRef.current?.focus();
    }
  }, [pendingInput, onPendingConsumed]);

  // Close mode dropdown on outside click
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (modeDropdownRef.current && !modeDropdownRef.current.contains(e.target as Node)) {
        setShowModeDropdown(false);
      }
    };
    if (showModeDropdown) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [showModeDropdown]);

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
    : session.agentType === 'api'
    ? '向 API 模型发送消息... (Ctrl+I 灵感, Ctrl+K 技能)'
    : session.agentType === 'codex'
    ? '向 codeX 发送消息... (Ctrl+I 灵感, Ctrl+K 技能)'
    : '向 Hermes Agent 发送消息... (Ctrl+I 灵感, Ctrl+K 技能)';

  return (
    <div className="shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
      {/* Toolbar: mode selector + inspiration + skill + send */}
      <div className="flex items-center gap-1 px-4 pt-3">
        {/* Mode dropdown */}
        <div className="relative" ref={modeDropdownRef}>
          <button
            onClick={() => setShowModeDropdown((v) => !v)}
            className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: MODE_COLORS[mode],
              border: '1px solid var(--border)',
            }}
            title={modeDescriptions[mode] || '加载中...'}
          >
            {(() => {
              const Icon = MODE_ICONS[mode];
              return <Icon size={11} />;
            })()}
            {MODE_LABELS[mode]}
            <ChevronUp size={11} style={{ color: 'var(--text-secondary)' }} />
          </button>
          {showModeDropdown && (
            <div
              className="absolute left-0 bottom-full mb-1 py-1 rounded-lg shadow-lg z-50"
              style={{
                backgroundColor: 'var(--bg-panel)',
                border: '1px solid var(--border)',
                minWidth: '200px',
              }}
            >
              {(Object.keys(MODE_LABELS) as ChatMode[]).map((m) => {
                const Icon = MODE_ICONS[m];
                const isActive = mode === m;
                return (
                  <button
                    key={m}
                    onClick={() => { setMode(m); setShowModeDropdown(false); }}
                    className="flex items-center gap-2 w-full px-3 py-1.5 text-xs transition-colors text-left"
                    style={{
                      color: isActive ? MODE_COLORS[m] : 'var(--text-primary)',
                      backgroundColor: isActive ? `${MODE_COLORS[m]}11` : 'transparent',
                    }}
                    title={modeDescriptions[m] || '加载中...'}
                  >
                    <Icon size={12} />
                    <span className="flex-1">{MODE_LABELS[m]}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* Inspiration & Skill buttons */}
        <button
          onClick={() => { setShowInspirationPicker((v) => !v); setShowSkillPicker(false); }}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: showInspirationPicker ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: showInspirationPicker ? 'var(--border)' : 'transparent',
          }}
          title="灵感搜索 (Ctrl+I)"
        >
          <Lightbulb size={12} />
          灵感
        </button>
        <button
          onClick={() => { setShowSkillPicker((v) => !v); setShowInspirationPicker(false); }}
          className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors"
          style={{
            color: showSkillPicker ? AGENT_THEMES.hermes.cssVar : 'var(--text-secondary)',
            backgroundColor: showSkillPicker ? 'var(--border)' : 'transparent',
          }}
          title="技能列表 (Ctrl+K)"
        >
          <Cpu size={12} />
          技能
        </button>

        {/* Spacer */}
        <div className="flex-1" />

        {/* Stop / Send button */}
        {isGenerating ? (
          <button
            onClick={onStop}
            className="p-1.5 rounded-lg transition-colors"
            style={{ backgroundColor: '#EF444422', color: '#EF4444' }}
            title="停止生成"
          >
            <Square size={14} />
          </button>
        ) : (
          <button
            onClick={handleSendInternal}
            disabled={!input.trim() || !session}
            className="p-1.5 rounded-lg transition-colors disabled:opacity-30"
            style={{
              backgroundColor: input.trim() && session ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: input.trim() && session ? '#fff' : 'var(--text-secondary)',
            }}
            title="发送"
          >
            <Send size={14} />
          </button>
        )}
      </div>

      {/* Input area */}
      <div className="flex items-end px-4 py-3" ref={pickerAnchorRef}>
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
      </div>
    </div>
  );
}
