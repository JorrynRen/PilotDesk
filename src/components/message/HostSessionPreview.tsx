import { useCallback, useState } from 'react';
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
      {/* 头部：来源 + 只读标识 + 返回 */}
      <div
        className="shrink-0 px-4 flex items-center gap-2 border-b"
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
        <button
          onClick={closePreview}
          className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0"
          style={{ color: 'var(--text-secondary)' }}
        >
          <ArrowLeft size={11} />
          返回
        </button>
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

      <div className="flex-1 overflow-y-auto min-h-0 px-4 py-3 space-y-4">
        {messages.length === 0 ? (
          <div className="h-full flex items-center justify-center text-xs" style={{ color: 'var(--text-tertiary)' }}>
            该会话没有可回放的消息正文
          </div>
        ) : (
          messages.map((m) => (
            <MessageBubble
              key={m.id}
              message={m}
              agentType={preview.agentType}
              hideTimestamp
            />
          ))
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
