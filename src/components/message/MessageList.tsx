import { useRef, useEffect } from 'react';
import { MessageBubble } from './MessageBubble';
import type { Message, Session } from '../../types';

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
        <div className="text-center px-8">
          <img
            src="/logo-lg.png"
            alt="PilotDesk"
            className="w-20 h-20 mx-auto mb-4 rounded-[18px]"
            draggable={false}
          />
          <h2 className="text-lg font-semibold mb-1">PilotDesk</h2>
          <p className="text-sm mb-1" style={{ color: 'var(--text-secondary)' }}>
            Claude Code & Hermes Agent 统一桌面客户端
          </p>
          <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            从左侧选择或创建会话开始对话
          </p>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>
          开始新对话...
        </p>
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
