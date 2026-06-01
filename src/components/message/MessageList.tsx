import { useRef, useEffect } from 'react';
import { MessageBubble } from './MessageBubble';
import type { Message, Session } from '../../types';

const AGENT_LABELS: Record<string, string> = {
  claude: 'Claude Code',
  hermes: 'Hermes Agent',
  api: 'API 直连',
};

interface MessageListProps {
  messages: Message[];
  session: Session | null;
  onEditMessage?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
}

export function MessageList({ messages, session, onEditMessage, onSaveInspiration }: MessageListProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages.length]);

  if (!session) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8 max-w-xs">
          <img
            src="/logo-lg.png"
            alt="PilotDesk"
            className="w-16 h-16 mx-auto mb-5 rounded-2xl opacity-90"
            draggable={false}
          />
          <h2 className="text-base font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
            PilotDesk
          </h2>
          <p className="text-xs leading-relaxed mb-4" style={{ color: 'var(--text-secondary)' }}>
            Claude Code &amp; Hermes Agent 统一桌面客户端
          </p>
          <div
            className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[11px]"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
            从左侧创建或选择会话开始对话
          </div>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    const agentLabel = AGENT_LABELS[session.agentType] || session.agentType;
    const modelInfo = session.agentType === 'api' && session.apiModel
      ? ` · ${session.apiModel}`
      : '';

    const agentColors: Record<string, string> = {
      claude: 'var(--claude-tag)',
      hermes: 'var(--hermes-tag)',
      api: 'var(--accent)',
    };
    const dotColor = agentColors[session.agentType] || 'var(--accent)';

    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8">
          <div
            className="inline-flex items-center gap-2 px-3 py-1.5 rounded-full text-xs mb-4"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <span className="w-2 h-2 rounded-full" style={{ backgroundColor: dotColor }} />
            {agentLabel}{modelInfo}
          </div>
          <p className="text-sm font-medium mb-1" style={{ color: 'var(--text-primary)' }}>
            开始新的对话
          </p>
          <p className="text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            {session.agentType === 'api'
              ? '消息将通过 API 直连发送'
              : '输入消息或使用技能与 Agent 交互'}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto">
      {messages.map((msg) => (
        <MessageBubble
          key={msg.id}
          message={msg}
          agentType={session.agentType}
          onEdit={onEditMessage}
          onSaveInspiration={onSaveInspiration}
        />
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
