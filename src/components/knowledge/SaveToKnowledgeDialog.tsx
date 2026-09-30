/**
 * SaveToKnowledgeDialog — 把一段会话内容**显式**存进知识库（进「待确认」候选队列）。
 *
 * 为什么一定要用户选库：知识库是多对多的，「存进哪个库」属于用户意图，
 * 不该由模型或程序替他决定。会话里那个动作只表达"这段值得沉淀"，目标库在这一步显式勾选。
 *
 * 两种方式（用户的显式选择）：
 *   - **原样存入**：`addCandidates`，落的是**对话原文**（`origin='work'`，后端只补标题/标签/属性，不改写正文）。
 *     简单直接，但噪声大。
 *   - **AI 整理成多条**：`digestConversation`，把这段对话**当素材交给 AI 结构化整理** ——
 *     一段对话常含多个主题，产出 0..N 条干净候选（标题/正文/标签/专属属性由 AI 填，并带原文摘录可溯源）。
 *     因为专属属性按库定义填，这种方式**只能选一个库**。
 */

import { useEffect, useState } from 'react';
import { Library, Loader2, Sparkles, FileText } from 'lucide-react';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/** 一条待沉淀的消息（带角色：模型要能分清"用户说的"与"助手建议的"） */
export interface KbDigestInput {
  role: string;
  content: string;
}

interface SaveToKnowledgeDialogProps {
  /** 要沉淀的消息（一条或多条；多条 = 一段对话）。 */
  messages: KbDigestInput[];
  onClose: () => void;
  /** 成功存入后回调（在 `onClose` 之前）—— 多选入口用它退出多选、清空勾选 */
  onSaved?: () => void;
}

/** 预览长度：够看清"存的是哪一段"即可，不必把整条消息塞进弹窗 */
const PREVIEW_CHARS = 240;

export function SaveToKnowledgeDialog({ messages, onClose, onSaved }: SaveToKnowledgeDialogProps) {
  const bases = useKnowledgeStore((s) => s.bases);
  const [selected, setSelected] = useState<string[]>([]);
  /** 'raw' = 原样存入；'digest' = 交给 AI 整理成 0..N 条候选 */
  const [mode, setMode] = useState<'raw' | 'digest'>('raw');
  const [busy, setBusy] = useState(false);

  // 打开时确保库列表已加载，并默认勾选"最近在知识库页用的那个库"。
  // setState 放在 await 之后（不是 effect 同步体里）：这是异步回填，不会引起额外渲染轮。
  useEffect(() => {
    let alive = true;
    (async () => {
      const st = useKnowledgeStore.getState();
      if (st.bases.length === 0) await st.loadBases();
      if (!alive) return;
      const after = useKnowledgeStore.getState();
      const preferred =
        after.activeBaseId && after.bases.some((b) => b.id === after.activeBaseId)
          ? after.activeBaseId
          : after.bases[0]?.id;
      if (preferred) setSelected([preferred]);
    })();
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  const toggle = (id: string) =>
    // AI 整理只能选一个库（专属属性按库定义填）；原样存入可多选
    setSelected((prev) =>
      mode === 'digest' ? [id] : prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );

  const canSubmit = selected.length > 0 && messages.length > 0 && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    try {
      if (mode === 'digest') {
        const count = await useKnowledgeStore
          .getState()
          .digestConversation(selected[0], messages.map((m) => ({ role: m.role, content: m.content })));
        // null = 失败（store 已提示）；失败时保持弹窗，用户可改方式重试
        if (count === null) return;
      } else {
        // key 留空 = 标题交给 AI（与投喂片段同一口径）；整理结果在知识库页「投喂 › 待确认」里核对。
        // addCandidates 内部已处理失败提示（返回 void），所以这里无法区分成败，保持原行为。
        await useKnowledgeStore.getState().addCandidates({
          key: '',
          value: messages.map((m) => m.content).join('\n\n'),
          baseIds: selected,
          origin: 'work',
        });
      }
      onSaved?.();
      onClose();
    } catch (e) {
      // 兜底：意料之外的失败也必须说话，并把按钮从「整理中…」放出来
      // （正常失败已在 store 里提示；这里只接"谁也没想到"的抛出）
      showToast(`提交失败：${errorMessage(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const preview =
    messages.length === 1
      ? messages[0].content.length > PREVIEW_CHARS
        ? `${messages[0].content.slice(0, PREVIEW_CHARS)}…`
        : messages[0].content
      : `共 ${messages.length} 条消息`;

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="rounded-xl p-5 shadow-xl w-full max-w-md mx-4"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
          存入知识库
        </div>

        {/* 方式选择：原样存入 / AI 整理成多条 */}
        <div className="flex gap-1.5 mb-3">
          {([
            { id: 'raw' as const, label: '原样存入', icon: FileText },
            { id: 'digest' as const, label: 'AI 整理成多条', icon: Sparkles },
          ]).map(({ id, label, icon: Icon }) => {
            const on = mode === id;
            return (
              <button
                key={id}
                onClick={() => setMode(id)}
                className="pd-btn flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px]"
                style={{
                  color: on ? 'var(--accent)' : 'var(--text-secondary)',
                  backgroundColor: on ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                  border: '1px solid var(--border)',
                }}
              >
                <Icon size={11} />
                {label}
              </button>
            );
          })}
        </div>

        <div className="text-xs mb-3 leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
          {mode === 'digest' ? (
            <>
              把这段内容<strong>当素材交给 AI 整理</strong>：按主题拆成 0~N 条干净知识
              （标题 / 标签 / 专属属性由 AI 填，并附原文摘录可溯源），逐条核对后采纳。
              没有可沉淀的内容会诚实返回 0 条。
            </>
          ) : (
            <>
              内容会进入所选知识库的「待确认」，由 AI 补标题 / 标签 / 专属属性后，你核对再采纳入库；
              <strong>正文不会被改写</strong>。
            </>
          )}
        </div>

        <div
          className="text-[11px] leading-relaxed whitespace-pre-wrap break-words rounded-lg p-2.5 mb-3 max-h-32 overflow-y-auto"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
        >
          {preview}
        </div>

        {bases.length === 0 ? (
          <div className="text-xs mb-4" style={{ color: 'var(--text-tertiary)' }}>
            还没有知识库。先到「知识库」页新建一个（或投喂一份资料，会自动建库）。
          </div>
        ) : (
          <div className="mb-4 space-y-1.5">
            <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              {mode === 'digest' ? '存入哪个库（AI 整理只能选一个）' : '存入哪个库（可多选）'}
            </div>
            <div className="max-h-44 overflow-y-auto space-y-1">
              {bases.map((b) => (
                <label
                  key={b.id}
                  className="flex items-center gap-2 px-2 py-1.5 rounded-lg cursor-pointer select-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                >
                  <input
                    type={mode === 'digest' ? 'radio' : 'checkbox'}
                    checked={selected.includes(b.id)}
                    onChange={() => toggle(b.id)}
                    style={{ accentColor: 'var(--accent)' }}
                  />
                  <Library size={11} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
                  <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>{b.name}</span>
                  <span className="ml-auto shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {b.entryCount} 条
                  </span>
                </label>
              ))}
            </div>
          </div>
        )}

        <div className="flex justify-end gap-2">
          <button
            onClick={onClose}
            className="pd-btn px-3 py-1.5 text-xs rounded transition-colors"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            取消
          </button>
          <button
            onClick={() => void submit()}
            disabled={!canSubmit}
            className="pd-btn flex items-center gap-1 px-3 py-1.5 text-xs rounded transition-colors"
            style={{
              backgroundColor: 'var(--accent)',
              color: '#fff',
              border: 'none',
              opacity: canSubmit ? 1 : 0.5,
              cursor: canSubmit ? 'pointer' : 'not-allowed',
            }}
          >
            {busy && <Loader2 size={11} className="animate-spin" />}
            {busy ? (mode === 'digest' ? '整理中…' : '提交中…') : mode === 'digest' ? '整理成知识' : '存入待确认'}
          </button>
        </div>
      </div>
    </div>
  );
}
