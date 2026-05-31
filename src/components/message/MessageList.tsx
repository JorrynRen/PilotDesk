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
          <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
            从左侧选择或创建会话开始对话
          </p>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    const agentLabel = AGENT_LABELS[session.agentType] || session.agentType;
    const modelInfo = session.agentType === 'api' && session.apiModel
      ? ` · ${session.apiModel}`
      : '';

    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8">
          <div
            className="inline-flex items-center gap-2 px-3 py-1 rounded-full text-xs mb-3"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: 'var(--accent)' }} />
            {agentLabel}{modelInfo}
          </div>
          <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>
            输入消息开始对话
          </p>
          <p className="text-xs mt-1" style={{ color: 'var(--text-tertiary)' }}>
            {session.agentType === 'api'
              ? '消息将直接通过 API 发送，无需 Agent 中转'
              : '消息将通过 Sidecar 发送给 Agent 处理'}
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
