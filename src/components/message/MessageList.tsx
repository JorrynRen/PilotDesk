import { useRef, useEffect, useCallback, useState, useMemo, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { CheckSquare, Loader2, Sparkles, X } from 'lucide-react';
import { MessageBubble } from './MessageBubble';
import { MessageSelectionBar } from './MessageSelectionBar';
import { copyToClipboard, useMessageSelection } from './messageSelection';
import { SaveToKnowledgeDialog, type KbDigestInput } from '../knowledge/SaveToKnowledgeDialog';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import type { Message, Session } from '../../types';
import { isApiSession } from '../../utils/sessionType';
import type { ThinkingChainStep } from '../layout/MainPanel';
import type { ConfirmationBlockData } from '../confirmation/ConfirmationCard';

interface MessageListProps {
  messages: Message[];
  session: Session | null;
  isGenerating?: boolean;
  /** 工具执行进度（generate_video 等），在流式助手消息气泡内展示 */
  streamingProgress?: string;
  thinkingChain?: ThinkingChainStep[];
  /** ask_user 确认块（内嵌到最后一条 assistant 消息）。 */
  confirmation?: ConfirmationBlockData | null;
  onEditMessage?: (content: string) => void;
  onResendMessage?: (content: string) => void;
  /**
   * 搜索栏右侧的操作位（会话页放「转为工作流 / 转为群聊」）。
   * 用插槽而不是具体按钮：消息列表保持通用，业务动作仍定义在会话页。
   */
  headerActions?: ReactNode;
  /**
   * 关闭当前会话（渲染在顶部操作行**最右侧**）。会话页传 `startNewSession`：
   * 只清空选中、回到「快捷开始」，不删除任何数据。
   */
  onCloseSession?: () => void;
}

export function MessageList({ messages, session, isGenerating, streamingProgress, thinkingChain, confirmation, onEditMessage, onResendMessage, headerActions, onCloseSession }: MessageListProps) {
  const { getTheme } = useAgentRegistry();
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const [searchResultIndex, setSearchResultIndex] = useState(0);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Message[] | null>(null);
  const [isSearchingMessages, setIsSearchingMessages] = useState(false);
  const [highlightedMessageId, setHighlightedMessageId] = useState<string | null>(null);
  /** 多选态：勾选若干条消息 → 作为**一段对话**交给 AI 整理成知识（状态机与选择条见 MessageSelection） */
  const { selectMode, selectedIds, enter: enterSelectMode, exit: exitSelectMode, clear: clearSelection, toggle: toggleSelect } = useMessageSelection();
  /** 待沉淀的消息（非空时弹出入库弹窗） */
  const [kbMessages, setKbMessages] = useState<KbDigestInput[] | null>(null);

  /**
   * 选中的消息，**按对话顺序**（不是勾选顺序）—— 整理要按"谁先说、谁后说"来理解上下文。
   * 系统提示（错误/超时通知）不参与沉淀：它是会话运行状态，不是知识。
   */
  const selectedMessages = useMemo(
    () => messages.filter((m) => selectedIds.includes(m.id) && m.role !== 'system'),
    [messages, selectedIds],
  );

  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 依赖用**基本类型**（sessionId）而不是 `session?.id`：后者会被 React Compiler 判为
  // "手工依赖比推断出的依赖更具体"（preserve-manual-memoization），换用基本类型两边一致。
  const sessionId = session?.id;

  /**
   * 「整理为知识」（显式命令）：把本会话**自上次整理以来的新增**交给知识库模型整理成候选。
   *
   * 与「多选 → 沉淀为知识」的区别：多选是用户**自己挑**一段；这里不挑，交给后端按水位取增量
   * （目标库也由模型按内容选）。**不受「自动沉淀开关」与自动限频约束** —— 用户点了就该做，
   * 只受后端"同一会话在途互斥"和"没有新增内容"两道约束。
   */
  const [sedimenting, setSedimenting] = useState(false);
  const handleSediment = useCallback(async () => {
    if (!sessionId) return;
    setSedimenting(true);
    try {
      const r = await invoke<{ count: number; truncated: boolean; skipped: string }>(
        'kb_sediment_session',
        { sessionId },
      );
      const cut = r.truncated ? '（内容过长，仅整理了前一部分）' : '';
      if (r.count > 0) {
        showToast(`已整理 ${r.count} 条候选，去「知识库 › 待确认」核对${cut}`, 'success');
      } else if (r.skipped === 'busy') {
        showToast('该会话正在整理中，请稍候', 'info');
      } else if (r.skipped === 'no_base') {
        showToast('还没有知识库，请先在「知识库」页新建一个', 'info');
      } else {
        showToast('没有新的可沉淀内容', 'info');
      }
    } catch (e) {
      showToast(`整理失败: ${errorMessage(e)}`, 'error');
    } finally {
      setSedimenting(false);
    }
  }, [sessionId]);

  const handleMessageSearch = useCallback((query: string) => {
    setSearchQuery(query);
    // Clear previous timer
    if (searchTimerRef.current) {
      clearTimeout(searchTimerRef.current);
      searchTimerRef.current = null;
    }
    if (!query.trim()) {
      setSearchResults(null);
      setHighlightedMessageId(null);
      return;
    }
    // Debounce search to avoid invoke on every keystroke
    searchTimerRef.current = setTimeout(async () => {
      setIsSearchingMessages(true);
      try {
        const results = await invoke<Message[]>('search_messages', {
          sessionId: sessionId ?? null,
          query: query.trim(),
          limit: 50,
        });
        setSearchResults(results);
        setSearchResultIndex(0);
      } catch { /* ignore */ }
      setIsSearchingMessages(false);
    }, 300);
  }, [sessionId]);

  // Cleanup timer on unmount
  useEffect(() => {
    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, []);

  // 记录上一个会话 ID，用于检测会话切换
  const prevSessionIdRef = useRef<string | null>(null);

  // 切换会话时 → 滚动到底部
  useEffect(() => {
    if (sessionId && sessionId !== prevSessionIdRef.current) {
      prevSessionIdRef.current = sessionId;
      // 换会话就退出多选：上一段对话的勾选在新会话里毫无意义
      exitSelectMode();
      if (messages.length > 0) {
        setTimeout(() => {
          virtuosoRef.current?.scrollToIndex({ index: messages.length - 1, behavior: 'auto' });
        }, 100);
      }
    }
    // `messages.length` 只在上面那个"会话真的换了"的判据成立时才起作用：新消息到达时
    // 该判据为假，这里什么也不做（否则每来一条消息都会强制滚到底）
  }, [sessionId, messages.length, exitSelectMode]);

  const itemContent = useCallback((index: number) => {
    const msg = messages[index];
    if (!msg) return null;
    // 仅在流式生成中且是最后一条 assistant 消息时传入思维链
    const isLastAssistant = isGenerating && index === messages.length - 1 && msg.role === 'assistant';
    return (
      <MessageBubble
        message={msg}
        agentType={session?.agentType ?? ''}
        apiProviderId={session?.apiProvider}
        apiModel={session?.apiModel}
        thinkingChain={isLastAssistant ? thinkingChain : undefined}
        streamingProgress={isLastAssistant ? streamingProgress : undefined}
        confirmation={isLastAssistant ? confirmation : null}
        isStreaming={isLastAssistant}
        isHighlighted={highlightedMessageId === msg.id}
        onEdit={onEditMessage}
        onResend={onResendMessage}
        selectable={selectMode}
        selected={selectedIds.includes(msg.id)}
        onToggleSelect={toggleSelect}
      />
    );
  }, [messages, session, thinkingChain, isGenerating, streamingProgress, confirmation, highlightedMessageId, onEditMessage, onResendMessage, selectMode, selectedIds, toggleSelect]);


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
          <h2 className="text-base font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
            PilotDesk
          </h2>
          <p className="text-xs leading-relaxed mb-4" style={{ color: 'var(--text-secondary)' }}>
            Agent 统一桌面客户端
          </p>
          <div
            className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[11px]"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
            输入消息开始新会话（先选择会话模型与工作目录），或从左侧选择已有会话或新建
          </div>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    const agentTheme = getTheme(session.agentType);
    const agentLabel = agentTheme.label;
    const modelInfo = isApiSession(session.agentType) && session.apiModel
      ? ` · ${session.apiModel}`
      : '';
    const dotColor = agentTheme.color;

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
          <p className="text-sm  mb-1" style={{ color: 'var(--text-primary)' }}>
            开始新的对话
          </p>
          <p className="text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            {isApiSession(session.agentType)
              ? '消息将通过 API 直连发送'
              : '输入消息或使用技能与 Agent 交互'}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* 顶部一栏：搜索框 + 检索导航 + 业务操作位（会话页的「转为工作流 / 转为群聊」）。
          会话标题不再占这一行——标题在左侧列表里已有，省下的横向空间留给搜索与操作。 */}
      {messages.length > 0 && (
        <div className="shrink-0 px-4 flex items-center gap-2 border-b" style={{ borderColor: 'var(--border)', backgroundColor: 'var(--bg-primary)', height: '36px' }}>
          <div className="flex items-center gap-2 flex-1 min-w-0">
            <div className="relative flex-1">
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => handleMessageSearch(e.target.value)}
                placeholder="搜索消息..."
                className="w-full text-xs px-3 py-1 rounded-lg outline-none"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-primary)',
                  border: '1px solid var(--border)',
                }}
              />
              {searchQuery && (
                <button
                  onClick={() => { setSearchQuery(''); setSearchResults(null); setHighlightedMessageId(null); }}
                  className="absolute right-2 top-1/2 -translate-y-1/2"
                  style={{ color: 'var(--text-tertiary)' }}
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                </button>
              )}
              {isSearchingMessages && (
                <div className="absolute right-6 top-1/2 -translate-y-1/2">
                  <div className="w-3 h-3 border-2 border-t-transparent rounded-full animate-spin" style={{ borderColor: 'var(--text-tertiary)', borderTopColor: 'transparent' }} />
                </div>
              )}
            </div>
            {searchResults !== null && (
              <div className="flex items-center gap-1 shrink-0">
                <span className="pd-text-10" style={{ color: 'var(--text-tertiary)' }}>
                  {searchResults.length > 0
                    ? `${searchResultIndex + 1}/${searchResults.length}`
                    : '0 条'}
                </span>
                {searchResults.length > 0 && (
                  <>
                    <button
                      onClick={() => {
                        const next = Math.max(0, searchResultIndex - 1);
                        setSearchResultIndex(next);
                        setHighlightedMessageId(searchResults[next].id);
                        const target = messages.findIndex(m => m.id === searchResults[next].id);
                        if (target >= 0) virtuosoRef.current?.scrollToIndex({ index: target, behavior: 'auto', align: 'center' });
                      }}
                      className="pd-btn pd-text-10 px-1 py-0.5 rounded transition-colors hover:opacity-80"
                      style={{ color: searchResultIndex === 0 ? 'var(--text-quaternary)' : 'var(--text-secondary)' }}
                      title="上一条"
                    >
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5"><polyline points="18 15 12 9 6 15"/></svg>
                    </button>
                    <button
                      onClick={() => {
                        const next = Math.min(searchResults.length - 1, searchResultIndex + 1);
                        setSearchResultIndex(next);
                        setHighlightedMessageId(searchResults[next].id);
                        const target = messages.findIndex(m => m.id === searchResults[next].id);
                        if (target >= 0) virtuosoRef.current?.scrollToIndex({ index: target, behavior: 'auto', align: 'center' });
                      }}
                      className="pd-btn pd-text-10 px-1 py-0.5 rounded transition-colors hover:opacity-80"
                      style={{ color: searchResultIndex >= searchResults.length - 1 ? 'var(--text-quaternary)' : 'var(--text-secondary)' }}
                      title="下一条"
                    >
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5"><polyline points="6 9 12 15 18 9"/></svg>
                    </button>
                  </>
                )}
                {searchResults.length > 0 && (
                  <button
                    onClick={() => {
                      setSearchResultIndex(0);
                      setHighlightedMessageId(searchResults[0].id);
                      const target = messages.findIndex(m => m.id === searchResults[0].id);
                      if (target >= 0) virtuosoRef.current?.scrollToIndex({ index: target, behavior: 'auto', align: 'center' });
                    }}
                    className="pd-btn pd-text-10 px-1.5 py-0.5 rounded transition-colors hover:opacity-80"
                    style={{ color: 'var(--accent)' }}
                    title="定位到第一条"
                  >
                    定位
                  </button>
                )}
              </div>
            )}
          </div>
          {headerActions && (
            <div className="flex items-center gap-1 shrink-0">{headerActions}</div>
          )}
          {/* 多选态下隐藏：同屏出现「沉淀为知识」与「整理为知识」两个"知识"动作，用户会分不清该点哪个 */}
          {!selectMode && (
            <button
              onClick={() => void handleSediment()}
              disabled={!sessionId || sedimenting}
              className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0 transition-colors disabled:opacity-40"
              style={{ color: 'var(--text-secondary)' }}
              title="把本会话的新增内容交给知识库模型整理成候选（进「待确认」，不直入库）"
            >
              {sedimenting ? <Loader2 size={11} className="animate-spin" /> : <Sparkles size={11} />}
              {sedimenting ? '整理中' : '整理为知识'}
            </button>
          )}
          <button
            onClick={() => (selectMode ? exitSelectMode() : enterSelectMode())}
            className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0 transition-colors"
            style={{
              color: selectMode ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: selectMode ? 'var(--accent-light)' : 'transparent',
            }}
            title="多选消息：勾选若干条，交给 AI 整理成多条知识"
          >
            {selectMode ? <X size={11} /> : <CheckSquare size={11} />}
            {selectMode ? '退出多选' : '多选'}
          </button>
          {/* 关闭会话：行尾图标按钮（样式与群聊页的关闭按钮一致，不带文字） */}
          {onCloseSession && (
            <button
              onClick={onCloseSession}
              className="p-1.5 rounded-md hover:opacity-70 transition-opacity shrink-0"
              style={{ color: 'var(--text-tertiary)' }}
              title="关闭当前会话（回到「快捷开始」，不删除任何数据）"
            >
              <X size={13} />
            </button>
          )}
        </div>
      )}

      {/* 选择条：多选态下出现。把勾选的消息**按对话顺序**交给 AI 整理成 0..N 条知识 */}
      {selectMode && (
        <MessageSelectionBar
          count={selectedMessages.length}
          hint="按对话顺序整理成多条知识（系统提示不参与）"
          onCopy={() =>
            void copyToClipboard(
              selectedMessages.map((m) => `${m.role === 'user' ? '用户' : '助手'}：${m.content}`).join('\n\n'),
              `已复制 ${selectedMessages.length} 条消息`,
            )
          }
          onDigest={() =>
            setKbMessages(selectedMessages.map((m) => ({ role: m.role, content: m.content })))
          }
          onClear={clearSelection}
        />
      )}

      <Virtuoso
        ref={virtuosoRef}
        className="flex-1"
        data={messages}
        itemContent={itemContent}
        followOutput="smooth"
        increaseViewportBy={{ top: 200, bottom: 200 }}
        components={{
          Footer: () => null,
        }}
      />

      {/* 沉淀弹窗：把选中的一段对话交给 AI 整理成 0..N 条知识（成功后退出多选） */}
      {kbMessages && (
        <SaveToKnowledgeDialog
          messages={kbMessages}
          onClose={() => setKbMessages(null)}
          onSaved={exitSelectMode}
        />
      )}
    </div>
  );
}
