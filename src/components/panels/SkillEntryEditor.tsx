/**
 * SkillEntryEditor — 技能主文件（SKILL.md）内置编辑器
 *
 * 只做「读 → 改 → 写」：路径交给后端按该 Agent 的技能根做越界校验，
 * 保存前后端会校验 frontmatter —— 把 `---` 或 name/description 改没了会被拒绝并回显原因，
 * 避免手改出一个"扫不出来"的技能。
 */

import { useEffect, useRef, useState } from 'react';
import { FileText, Loader2, Save, X } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

interface SkillEntryEditorProps {
  agentType: string;
  skillName: string;
  /** 技能入口文件绝对路径（后端会校验它在该 Agent 的技能根内） */
  entryPath: string;
  onClose: () => void;
  /** 保存成功回调（调用方据此刷新技能列表） */
  onSaved: () => void;
}

export function SkillEntryEditor({ agentType, skillName, entryPath, onClose, onSaved }: SkillEntryEditorProps) {
  const [content, setContent] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    (async () => {
      try {
        const text = await invoke<string>('skill_read_entry', { agentType, entryPath });
        if (!disposed) setContent(text);
      } catch (e) {
        if (!disposed) setError(errorMessage(e));
      } finally {
        if (!disposed) setLoaded(true);
      }
    })();
    return () => {
      disposed = true;
    };
  }, [agentType, entryPath]);

  const handleSave = async () => {
    if (saving || !loaded) return;
    setSaving(true);
    setError(null);
    try {
      await invoke('skill_write_entry', { agentType, entryPath, content });
      showToast(`已保存「${skillName}」主文件`, 'success');
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const saveRef = useRef(handleSave);
  // 用 effect 同步 ref（不写依赖数组：handleSave 每次渲染都是新函数，逐次同步即可）：
  // 渲染期写 ref 会被 `react-hooks/refs` 判违规（读取发生在按键回调里，时序无影响）
  useEffect(() => { saveRef.current = handleSave; });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
        return;
      }
      // Ctrl/Cmd+S 保存（与多数编辑器习惯一致）
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void saveRef.current();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[760px] max-w-[92vw] h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <FileText size={15} style={{ color: 'var(--accent)', flexShrink: 0 }} />
          <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>
            编辑「{skillName}」主文件
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

        <div className="px-5 py-1.5 shrink-0 text-[10px] truncate" style={{ borderBottom: '1px solid var(--border)', color: 'var(--text-tertiary)' }} title={entryPath}>
          {entryPath}
        </div>

        <div className="flex-1 overflow-hidden px-5 py-3 flex flex-col gap-2">
          {!loaded ? (
            <div className="flex items-center gap-2 text-xs py-6 justify-center" style={{ color: 'var(--text-tertiary)' }}>
              <Loader2 size={13} className="animate-spin" />
              读取中…
            </div>
          ) : (
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              spellCheck={false}
              className="flex-1 w-full p-3 rounded-lg outline-none resize-none text-xs leading-relaxed"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
                fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
              }}
            />
          )}
          {error && (
            <div className="text-[11px] px-3 py-2 rounded" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
              {error}
            </div>
          )}
        </div>

        <div className="px-5 py-3 shrink-0 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
          <span className="text-[10px] flex-1" style={{ color: 'var(--text-tertiary)' }}>
            顶部 `---` 包裹的 frontmatter 必须保留 name 与 description，否则保存会被拒绝
          </span>
          <button
            onClick={() => void handleSave()}
            disabled={saving || !loaded}
            className="pd-btn px-3 py-1.5 rounded-lg text-xs flex items-center gap-1.5"
            style={{
              backgroundColor: 'var(--accent)',
              color: '#fff',
              border: 'none',
              opacity: saving || !loaded ? 0.6 : 1,
              cursor: saving || !loaded ? 'not-allowed' : 'pointer',
            }}
            title="保存（Ctrl+S）"
          >
            {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
