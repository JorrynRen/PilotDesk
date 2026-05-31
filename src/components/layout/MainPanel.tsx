import { useState, useCallback } from 'react';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useWebSocket } from '../../hooks/useWebSocket';
import type { ChatMode } from '../../types';

export function MainPanel() {
  const {
    currentSessionId,
    sessions,
    messages,
  } = useSessionStore();

  const [isGenerating, setIsGenerating] = useState(false);
  const [streamingContent, setStreamingContent] = useState('');

  const currentSession = sessions.find((s) => s.id === currentSessionId) || null;

  // WebSocket handlers
  const onChunk = useCallback((sessionId: string, content: string) => {
    if (sessionId === currentSessionId) {
      setStreamingContent((prev) => prev + content);
    }
  }, [currentSessionId]);

  const onDone = useCallback((sessionId: string) => {
    if (sessionId === currentSessionId) {
      setIsGenerating(false);
      setStreamingContent('');
    }
  }, [currentSessionId]);

  const useWebSocketProps = {
    onChunk,
    onDone,
    onError: (sessionId: string, error: string) => {
      console.error(`[WS Error] ${sessionId}:`, error);
      setIsGenerating(false);
    },
    onStatus: (sessionId: string, status: string) => {
      console.log(`[WS Status] ${sessionId}: ${status}`);
    },
  };

  const { sendChat, stopGeneration } = useWebSocket(19830, useWebSocketProps);

  const handleSend = useCallback(
    (message: string, mode: ChatMode) => {
      if (!currentSession) return;
      setIsGenerating(true);
      setStreamingContent('');
      sendChat(currentSession.id, message, mode, currentSession.agentType);
    },
    [currentSession, sendChat]
  );

  const handleStop = useCallback(() => {
    if (currentSession) {
      stopGeneration(currentSession.id, currentSession.agentType);
    }
  }, [currentSession, stopGeneration]);

  // Build display messages with streaming content appended
  const displayMessages = streamingContent
    ? [
        ...messages,
        {
          id: 'streaming',
          sessionId: currentSessionId || '',
          role: 'assistant' as const,
          content: messages[messages.length - 1]?.role === 'assistant'
            ? messages[messages.length - 1].content + streamingContent
            : streamingContent,
          mode: 'native' as const,
          timestamp: Date.now() / 1000,
        },
      ]
    : messages;

  return (
    <main
      className="flex-1 flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)' }}
    >
      <MessageList
        messages={displayMessages}
        session={currentSession}
        onEditMessage={(content) => {
          // TODO: implement edit
          console.log('Edit message:', content.slice(0, 50));
        }}
        onSaveInspiration={(content) => {
          // TODO: implement save inspiration
          console.log('Save inspiration:', content.slice(0, 50));
        }}
      />
      <InputBar
        session={currentSession}
        onSend={handleSend}
        onStop={handleStop}
        isGenerating={isGenerating}
      />
    </main>
  );
}
