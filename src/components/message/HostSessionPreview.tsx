import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowLeft, Download, Loader2, Info } from 'lucide-react';

import { MessageBubble } from '../message/MessageBubble';
import { SettingsButton } from '../settings';
import { useHostPreviewStore } from '../../stores/hostPreviewStore';
import { useSessionStore } from '../../stores/sessionStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import type { Message } from '../../types';

/**
 * 宿主会话只读预览（右侧消息区）。
 *
 * 与常规聊天视图刻意分开：
 * - 数据来自 `acp_load_host_session` 的一次性回放，**不落库**，关闭即丢；
 * - 只有正文（ACP `session/load` 只回 user/assistant chunk），没有工具/思考/审批/用量；
 * - 输入框换成「导入并继续」：真正要继续对话必须先导入成一条本地会话。
 */
export function HostSessionPreview() {
  const preview = useHostPreviewStore((s) => s.preview);
  const closePreview = useHostPreviewStore((s) => s.closePreview);
  const createSession = useSessionStore((s) => s.createSession);
  const selectSession = useSessionStore((s) => s.selectSession);
  const deleteSession = useSessionStore((s) => s.deleteSession);
  const { getDisplayName } = useAgentRegistry();
  const [importing, setImporting] = useState(false);

  /**
   * 逐条回放：`session/load` 一次性把整段历史返回，若直接整屏铺开，用户看不出在回放、
   * 容易以为卡住。进度存在 store 里（左侧列表要同步显示"回放中"），这里只负责推进。
   */
  const replay = useHostPreviewStore((s) => s.replay);
  const advanceReplay = useHostPreviewStore((s) => s.advanceReplay);
  const total = preview?.messages.length ?? 0;
  const revealed = replay ? replay.revealed : total;
  const replaying = replay !== null;
  const percent = total > 0 ? Math.min(100, (revealed / total) * 100) : 0;
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!replaying) return;
    // 长会话按条数加速，避免几十条时回放拖太久
    const stepMs = total > 24 ? 60 : 120;
    const timer = window.setInterval(() => advanceReplay(), stepMs);
    return () => window.clearInterval(timer);
  }, [replaying, total, advanceReplay]);

  // 跟着回放往下走：新的一条露出后滚到它，避免用户还要手动追
  useEffect(() => {
    if (revealed === 0) return;
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: replaying ? 'smooth' : 'auto' });
  }, [revealed, replaying]);

  /** 导入为本地会话并切换过去：这是"继续对话"的唯一入口 */
  const handleImport = useCallback(async () => {
    if (!preview || importing) return;
    setImporting(true);
    let createdId: string | null = null;
    try {
      const local = await createSession(
        preview.agentType,
        preview.cwd ?? undefined,
        preview.title || '导入的会话',
      );
      createdId = local.id;
      await invoke<number>('acp_import_host_session', {
        agentType: preview.agentType,
        hostSessionId: preview.hostSessionId,
        cwd: preview.cwd,
        localSessionId: local.id,
      });
      closePreview();
      void selectSession(local.id);
      showToast('已导入，可继续对话', 'success');
    } catch (err) {
      // 导入失败不留空会话
      if (createdId) {
        try { await deleteSession(createdId); } catch { /* ignore */ }
      }
      showToast(`导入失败: ${errorMessage(err)}`, 'error');
    } finally {
      setImporting(false);
    }
  }, [preview, importing, createSession, deleteSession, closePreview, selectSession]);

  if (!preview) return null;

  // 回放消息没有可靠时间戳：统一给 0 并让气泡隐藏时间（编造时间比不显示更误导）
  const messages: Message[] = preview.messages.map((m, i) => ({
    id: m.id || `host-${i}`,
    sessionId: preview.hostSessionId,
    role: m.role,
    content: m.content,
    mode: 'native',
    timestamp: 0,
  }));

  return (
    <div
      className="flex-1 flex flex-col overflow-hidden relative rounded-lg"
      style={{ backgroundColor: 'var(--bg-content)' }}
    >
      {/* 头部：来源 + 只读标识 + 回放进度 + 返回 */}
      <div
        className="shrink-0 px-4 flex items-center gap-2 border-b relative"
        style={{ height: '36px', borderColor: 'var(--border)', backgroundColor: 'var(--bg-content)' }}
      >
        <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
          {preview.title || '（无标题）'}
        </span>
        <span
          className="text-[10px] px-1.5 py-0.5 rounded-full shrink-0"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
        >
          只读预览 · {getDisplayName(preview.agentType)}
        </span>
        <div className="flex-1" />
        {/* 回放进度紧贴标题栏：不占消息区高度，播完即消失，避免消息区整体位移造成"抖动" */}
        {replaying && (
          <span
            className="flex items-center gap-1 text-[10px] shrink-0"
            style={{ color: 'var(--text-tertiary)' }}
          >
            <Loader2 size={10} className="animate-spin" />
            {revealed}/{total}
          </span>
        )}
        <button
          onClick={closePreview}
          className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0"
          style={{ color: 'var(--text-secondary)' }}
        >
          <ArrowLeft size={11} />
          返回
        </button>
        {/* 2px 进度线压在标题栏下边框上，属于头部的一部分，不参与消息区布局 */}
        {replaying && (
          <div
            className="absolute left-0 bottom-0 h-[2px] transition-all duration-150"
            style={{ width: `${percent}%`, backgroundColor: 'var(--accent)' }}
          />
        )}
      </div>

      {preview.cwd && (
        <div
          className="shrink-0 px-4 py-1 text-[10px] truncate"
          title={preview.cwd}
          style={{ color: 'var(--text-tertiary)' }}
        >
          {preview.cwd}
        </div>
      )}

      <div ref={scrollRef} className="flex-1 overflow-y-auto min-h-0 px-4 py-3 space-y-4">
        {total === 0 ? (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6">
            <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
              宿主没有返回任何历史内容
            </span>
            <span
              className="text-[10px] leading-relaxed"
              style={{ color: 'var(--text-tertiary)', opacity: 0.8 }}
            >
              常见原因是宿主适配器未能恢复这条会话（例如模型 Provider 未配置好）。
              可在宿主应用里确认该会话能正常打开后再试。
            </span>
          </div>
        ) : (
          messages.slice(0, revealed).map((m, i) => {
            // 末条 = 此刻"正在回放"的那一条：高亮 + 淡入，回放结束即恢复正常
            const isCurrent = replaying && i === revealed - 1;
            return (
              <div
                key={m.id}
                className={isCurrent ? 'pd-replay-item pd-replay-current' : 'pd-replay-item'}
              >
                <MessageBubble
                  message={m}
                  agentType={preview.agentType}
                  hideTimestamp
                />
              </div>
            );
          })
        )}
      </div>

      {/* 底部：说明 + 导入继续（不支持在预览里直接对宿主发消息） */}
      <div
        className="shrink-0 px-4 py-2 flex items-center gap-2 border-t"
        style={{ borderColor: 'var(--border)', backgroundColor: 'var(--bg-content)' }}
      >
        <Info size={11} className="shrink-0" style={{ color: 'var(--text-tertiary)' }} />
        <span className="text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }}>
          仅预览消息正文；要继续对话请先导入为本应用的会话
        </span>
        <div className="flex-1" />
        <SettingsButton
          variant="primary"
          onClick={() => void handleImport()}
          disabled={importing}
          icon={importing ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
        >
          {importing ? '导入中…' : '导入并继续'}
        </SettingsButton>
      </div>
    </div>
  );
}
