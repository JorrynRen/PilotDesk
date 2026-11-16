/**
 * ProjectMemoryDialog — 项目记忆预览弹窗
 *
 * 只读预览当前项目主目录（未归属项目时为全局工作空间）的 AGENTS.md，
 * 并提供跳转到设置页编辑的入口。
 *
 * 为什么是弹窗而不是右侧面板的常驻 tab：它只有一个只读预览 + 一个跳设置按钮，
 * 常驻会白占一个窄面板（260px）顶部的 tab 位；而"看一眼 Agent 现在记住了什么"属于
 * 低频、看一眼就走的动作，弹窗更合适（入口在输入框工具栏）。
 *
 * 口径提示：普通会话的动态工作根不持久化，由后端按项目主目录/全局工作空间解析；
 * ACP 宿主导入会话的运行 cwd 虽然是宿主恢复目录，PilotDesk 侧项目文件仍固定在
 * 项目主目录或全局工作空间；宿主是否加载它由宿主自行处理，不做显式注入。
 */

import { useEffect, useState } from 'react';
import { MemoryStick, FileText, X } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useSessionStore } from '../../stores/sessionStore';
import { useSessionWorkspaceInfo } from '../../hooks/useSessionWorkspaceInfo';
import { getProjectMemory, type ProjectMemoryInfo } from '../../types';
import { elide } from '../../utils/text';
import { normalizeDisplayPath } from '../../utils/path';

interface ProjectMemoryDialogProps {
  onClose: () => void;
}

export function ProjectMemoryDialog({ onClose }: ProjectMemoryDialogProps) {
  const navigate = useNavigate();
  const currentSession = useSessionStore((s) => s.sessions.find((x) => x.id === s.currentSessionId));
  const { info: workspaceInfo, loading: workspaceLoading } = useSessionWorkspaceInfo(currentSession?.id);
  const [memoryState, setMemoryState] = useState<{
    scope: string;
    mem: ProjectMemoryInfo | null;
  }>({ scope: '', mem: null });

  const sessionTitle = currentSession?.title || '';
  const hasSession = !!currentSession;
  const hostManaged = hasSession && currentSession?.agentType !== 'api';
  const memoryRoot = hasSession ? workspaceInfo?.memoryRoot : undefined;
  const workspaceResolved = !hasSession || (!workspaceLoading && !!workspaceInfo);
  const memoryScope = workspaceResolved ? `root:${memoryRoot ?? '__global__'}` : null;
  const mem = memoryScope && memoryState.scope === memoryScope ? memoryState.mem : null;
  const loading = !memoryScope || memoryState.scope !== memoryScope;
  const displayMemoryRoot = normalizeDisplayPath(mem?.root ?? memoryRoot);
  const sourceLabel = !hasSession
    ? '全局工作空间'
    : workspaceInfo?.projectId
      ? '项目主目录'
      : '全局工作空间';

  useEffect(() => {
    if (!memoryScope) return;
    let disposed = false;
    void getProjectMemory(memoryRoot)
      .then((next) => {
        if (!disposed) setMemoryState({ scope: memoryScope, mem: next });
      })
      .catch(() => {
        if (!disposed) setMemoryState({ scope: memoryScope, mem: null });
      });
    return () => {
      disposed = true;
    };
  }, [memoryRoot, memoryScope]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  const openSettings = () => {
    onClose();
    navigate('/settings?tab=memory');
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[560px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div className="flex items-center gap-2 px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <MemoryStick size={15} style={{ color: 'var(--accent)', flexShrink: 0 }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>项目记忆</span>
          <span className="text-[11px] truncate min-w-0" style={{ color: 'var(--text-tertiary)' }}>
            {hasSession
              ? `${sessionTitle || '未命名会话'} · ${sourceLabel}`
              : `${sourceLabel}记忆`}
          </span>
          <div className="flex-1" />
          <button
            onClick={onClose}
            className="pd-btn p-1 rounded hover:opacity-80 shrink-0"
            style={{ color: 'var(--text-tertiary)' }}
            title="关闭"
          >
            <X size={14} />
          </button>
        </div>

        {/* 记忆根来源：ACP 宿主目录与项目记忆根不是同一个概念，需要明确展示。 */}
        <div className="px-5 py-2 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <span className="text-[11px] break-all" style={{ color: 'var(--text-tertiary)' }}>
            {sourceLabel}记忆根：{displayMemoryRoot || '解析中…'}
          </span>
          {hostManaged && (
            <span className="block mt-1 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              该会话由 CLI/ACP 宿主运行；AGENTS.md 是否加载由宿主自行处理，PilotDesk 不向宿主注入内容。
            </span>
          )}
        </div>

        {/* 内容 */}
        <div className="flex-1 overflow-y-auto px-5 py-4 pd-scroll-stable">
          {loading ? (
            <div className="text-xs py-8 text-center" style={{ color: 'var(--text-tertiary)' }}>加载中…</div>
          ) : !mem || !mem.exists ? (
            <div className="flex flex-col items-center justify-center gap-3 py-8">
              <MemoryStick size={26} style={{ color: 'var(--text-tertiary)' }} />
              <span className="text-xs text-center" style={{ color: 'var(--text-secondary)' }}>
                该工作区还没有 AGENTS.md
              </span>
              {mem && (
                <span className="text-[10px] max-w-full truncate" style={{ color: 'var(--text-tertiary)' }} title={displayMemoryRoot}>
                  记忆根：{displayMemoryRoot}
                </span>
              )}
              <button
                onClick={openSettings}
                className="pd-btn px-3 py-1.5 rounded-lg text-xs"
                style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
              >
                去设置创建
              </button>
            </div>
          ) : (
            <pre
              className="text-xs leading-relaxed whitespace-pre-wrap break-words m-0 rounded-lg px-3 py-2"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            >
              {elide(mem.content, 4000, '\n…（内容过长，预览截断）')}
            </pre>
          )}
        </div>

        {/* 底部操作 */}
        {!loading && mem?.exists && (
          <div className="px-5 py-3 shrink-0 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
            <button
              onClick={openSettings}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs flex items-center gap-1.5"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            >
              <FileText size={12} />
              在设置中编辑
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
