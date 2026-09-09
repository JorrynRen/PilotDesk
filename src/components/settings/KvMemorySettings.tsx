import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { Search, Plus, Trash2, Pencil, X, Check, Star, Eraser } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import {
  deleteMemoryEntry,
  getMemoryStats,
  listMemoryEntries,
  previewMemoryMaintenance,
  runMemoryMaintenance,
  saveMemoryEntry,
  setMemoryPin,
  type MemoryEntryView,
  type MemoryStats,
} from '../../types';
import { showToast } from '../../utils/toast';

const CATEGORY_OPTIONS = ['fact', 'preference', 'skill', 'event'];

function formatTime(ts: number): string {
  const d = new Date(ts * 1000);
  return d.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}

interface InlineForm {
  rowKey: string | null; // null = 顶部“新增”占位；否则为对应行的 key
  key: string;
  value: string;
  category: string;
  important: boolean;
  tags: string;
}

const inputStyle: CSSProperties = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
};

/** 列表分块每页条数 */
const PAGE_SIZE = 100;

/** 设置「记忆管理」→ 全局 KV 记忆（MEMORY.db）：列表行内编辑 + pin 保护 + 自动维护两步清理 */
export function KvMemorySettings() {
  const [entries, setEntries] = useState<MemoryEntryView[]>([]);
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [form, setForm] = useState<InlineForm | null>(null);
  const [saving, setSaving] = useState(false);
  // 清理流：'idle'=空闲；'preview'=展示候选待确认；'running'=执行中
  const [cleanPhase, setCleanPhase] = useState<'idle' | 'preview' | 'running'>('idle');
  const [preview, setPreview] = useState<MemoryEntryView[]>([]);
  // 列表分块展示：初始/过滤后每块 100 条，底部「加载更多」逐步增加
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  // 跨异步调用即时防重入（React 状态更新有延迟，连点可能在重渲染前二次进入）
  const busyRef = useRef(false);

  const refresh = useCallback(async (cat?: string, q?: string) => {
    setLoading(true);
    setError('');
    try {
      const [list, st] = await Promise.all([
        listMemoryEntries(cat || undefined, q || undefined),
        getMemoryStats(),
      ]);
      setEntries(list);
      setStats(st);
      // 结果变少时收敛可见条数，避免越界空白
      setVisibleCount((prev) => Math.min(prev, Math.max(list.length, 1)));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // 过滤条件变化：回到第一块
  useEffect(() => {
    setVisibleCount(PAGE_SIZE);
    refresh(category, query);
  }, [category, query, refresh]);

  const startNew = () => setForm({ rowKey: null, key: '', value: '', category: 'fact', important: false, tags: '' });
  const startEdit = (e: MemoryEntryView) =>
    setForm({ rowKey: e.key, key: e.key, value: e.value, category: e.category, important: e.pin, tags: e.tags });
  const cancelEdit = () => setForm(null);

  const handleSave = async () => {
    if (!form || saving) return;
    setSaving(true);
    setError('');
    try {
      await saveMemoryEntry(form.key, form.value, form.category || 'fact', form.important, form.tags);
      setForm(null);
      await refresh(category, query);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (key: string) => {
    if (!window.confirm(`确定删除记忆「${key}」？删除后不可恢复。`)) return;
    setError('');
    try {
      await deleteMemoryEntry(key);
      if (form?.rowKey === key) setForm(null);
      await refresh(category, query);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleTogglePin = async (key: string, pin: boolean) => {
    setError('');
    try {
      await setMemoryPin(key, pin);
      await refresh(category, query);
    } catch (e) {
      setError(String(e));
    }
  };

  // 两步清理：先预览（不删除），确认后执行
  const openPreview = async () => {
    if (cleanPhase !== 'idle' || busyRef.current) return;
    busyRef.current = true;
    setError('');
    try {
      const list = await previewMemoryMaintenance();
      if (list.length === 0) {
        setCleanPhase('idle');
        setPreview([]);
        showToast('当前没有可自动清理的冷记忆', 'info');
        return;
      }
      setPreview(list);
      setCleanPhase('preview');
      showToast(`可清理 ${list.length} 条冷记忆，请确认`, 'warning');
    } catch (e) {
      setCleanPhase('idle');
      showToast(`预览冷记忆失败: ${String(e)}`, 'error');
    } finally {
      busyRef.current = false;
    }
  };

  const confirmClean = async () => {
    if (cleanPhase !== 'preview' || busyRef.current) return;
    busyRef.current = true;
    setError('');
    try {
      const removed = await runMemoryMaintenance();
      setCleanPhase('idle');
      setPreview([]);
      showToast(
        removed.length > 0 ? `已清理 ${removed.length} 条冷记忆` : '没有条目被清理',
        removed.length > 0 ? 'success' : 'info'
      );
      await refresh(category, query);
    } catch (e) {
      setCleanPhase('idle');
      showToast(`清理失败: ${String(e)}`, 'error');
    } finally {
      busyRef.current = false;
    }
  };

  // 顶部“新增”占位（也是列表内的一行，保持原位置编辑风格）
  const showNewRow = form !== null && form.rowKey === null;
  const policy = stats?.policy;

  return (
    <SettingsSection title="全局 KV 记忆（MEMORY.db）">
      <p className="text-xs mb-2" style={{ color: 'var(--text-secondary)' }}>
        全局跨项目记忆，与 <code>save_memory</code> / <code>search_memory</code> 工具读写同一库。发送消息时，系统会结合当前对话意图检索少量相关记忆注入模型上下文（重要/pin 记忆优先）；无相关记忆或意图路由不可用时可能回退到评分最高的条目，也可能不注入（模型仍可用 <code>search_memory</code> 按需检索）。
      </p>

      {stats && policy && (
        <div className="rounded-lg px-3 py-2 mb-2 text-[11px] space-y-1" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          <div style={{ color: 'var(--text-primary)' }}>
            共 {stats.total} 条 · pin 保护 {stats.pinned} 条 · 自动维护候选 {stats.candidates} 条
          </div>
          <div style={{ color: 'var(--text-tertiary)' }}>
            自动维护规则：总条数超 {policy.maxEntries} 条时按“最久未访问”驱逐；超过 {policy.idleDays} 天未被访问且访问次数 ≤ {policy.minAccess} 次的未 pin 条目会被清理（pin 条目永不自动删除）。
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {stats.injected.length > 0 ? (
              <span style={{ color: 'var(--text-tertiary)' }}>
                高频 top-5（仅统计参考，非实际注入；实际注入随当前对话意图检索决定）：{stats.injected.map((x) => x.key).join('、')}
              </span>
            ) : (
              <span style={{ color: 'var(--text-tertiary)' }}>暂无记忆</span>
            )}
            <div className="flex-1" />
            <SettingsButton onClick={openPreview} disabled={cleanPhase !== 'idle'} icon={<Eraser size={12} />}>
              清理冷记忆
            </SettingsButton>
          </div>
        </div>
      )}

      {/* 清理预览确认（两步安全：先看将被删除的条目；确认后进入执行中） */}
      {cleanPhase !== 'idle' && (
        <div className="rounded-lg px-3 py-2 mb-2 text-xs" style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}>
          {cleanPhase === 'running' ? (
            <div style={{ color: 'var(--text-secondary)' }}>清理执行中…</div>
          ) : (
            <>
              <div className="mb-1.5 font-medium" style={{ color: 'var(--text-primary)' }}>
                将清理以下 {preview.length} 条冷记忆（不可恢复，pin 条目不会被清理）：
              </div>
              <ul className="space-y-0.5 max-h-40 overflow-y-auto" style={{ color: 'var(--text-secondary)' }}>
                {preview.map((e) => (
                  <li key={e.key} className="truncate">
                    <span className="font-mono" style={{ color: 'var(--text-primary)' }}>{e.key}</span>
                    {e.value ? ` — ${e.value}` : ''}
                  </li>
                ))}
              </ul>
              <div className="flex items-center justify-end gap-2 mt-2">
                <SettingsButton onClick={() => { setCleanPhase('idle'); setPreview([]); }}>
                  <X size={12} /> 取消
                </SettingsButton>
                <SettingsButton variant="primary" onClick={confirmClean}>
                  <Check size={12} /> 确认清理
                </SettingsButton>
              </div>
            </>
          )}
        </div>
      )}

      {/* 工具栏 */}
      <div className="flex items-center gap-2 mb-2">
        <div className="relative flex-1">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索 key/value…"
            className="w-full pl-7 px-2.5 py-1.5 rounded-lg text-xs outline-none"
            style={inputStyle}
          />
        </div>
        <select
          value={category}
          onChange={(e) => setCategory(e.target.value)}
          className="px-2 py-1.5 rounded-lg text-xs outline-none"
          style={inputStyle}
        >
          <option value="">全部分类</option>
          {CATEGORY_OPTIONS.map((c) => (
            <option key={c} value={c}>{c}</option>
          ))}
        </select>
        <SettingsButton onClick={startNew} icon={<Plus size={12} />} disabled={showNewRow}>
          新增
        </SettingsButton>
      </div>

      {error && <div className="text-xs mb-2" style={{ color: 'var(--danger, #EF4444)' }}>{error}</div>}

      {loading ? (
        <div className="text-xs py-6 text-center" style={{ color: 'var(--text-secondary)' }}>加载中…</div>
      ) : (
        <div className="flex flex-col gap-1.5">
          {/* 新增行（列表最前） */}
          {showNewRow && (
            <InlineRowEditor
              form={form!}
              setForm={setForm}
              saving={saving}
              onSave={handleSave}
              onCancel={cancelEdit}
              isNew
            />
          )}

          {entries.length === 0 && !showNewRow ? (
            <div className="text-xs py-6 text-center" style={{ color: 'var(--text-tertiary)' }}>
              {query || category ? '未找到匹配记忆' : '暂无 KV 记忆，点击「新增」添加'}
            </div>
          ) : (
            entries.slice(0, visibleCount).map((e) =>
              form?.rowKey === e.key ? (
                <InlineRowEditor
                  key={e.key}
                  form={form}
                  setForm={setForm}
                  saving={saving}
                  onSave={handleSave}
                  onCancel={cancelEdit}
                />
              ) : (
                <div key={e.key} className="relative group rounded-lg px-2.5 py-2" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
                  {/* 内容区：占满整行，右侧不再被操作按钮列挤占 */}
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-[11px] font-semibold truncate" style={{ color: 'var(--text-primary)' }}>{e.key}</span>
                      <span className="text-[9px] px-1.5 py-0.5 rounded shrink-0" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>{e.category}</span>
                      {e.pin && (
                        <span className="text-[9px] px-1.5 py-0.5 rounded shrink-0" style={{ backgroundColor: '#FBBF2422', color: '#D97706' }} title="重要记忆，不会自动清理">pin</span>
                      )}
                    </div>
                    <p className="text-xs leading-relaxed break-words mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                      {e.value}
                    </p>
                    <div className="flex items-start gap-2 mt-1">
                      {e.tags.trim() && (
                        <div className="flex flex-wrap gap-1 flex-1 min-w-0">
                          {e.tags.split(',').map((t) => t.trim()).filter(Boolean).map((t) => (
                            <span key={t} className="text-[9px] px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                              #{t}
                            </span>
                          ))}
                        </div>
                      )}
                      <div className="text-[9px] ml-auto shrink-0 text-right" style={{ color: 'var(--text-tertiary)' }}>
                        {formatTime(e.updatedAt)} 编辑 · {formatTime(e.lastAccessedAt)} 访问 · {e.accessCount} 次
                      </div>
                    </div>
                  </div>
                  {/* 操作按钮：卡片悬停即显示，不占行宽 */}
                  <div
                    className="absolute top-1.5 right-1.5 flex items-center gap-0.5 opacity-0 group-hover:opacity-100 pointer-events-none group-hover:pointer-events-auto transition-opacity"
                    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', borderRadius: 6 }}
                  >
                    <button
                      onClick={() => handleTogglePin(e.key, !e.pin)}
                      className="pd-btn p-1 rounded"
                      title={e.pin ? '取消 pin（将参与自动清理）' : 'pin 保护（自动清理永不删除）'}
                      style={{ color: e.pin ? '#D97706' : 'var(--text-secondary)' }}
                    >
                      <Star size={12} fill={e.pin ? 'currentColor' : 'none'} />
                    </button>
                    <button onClick={() => startEdit(e)} title="编辑" className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
                      <Pencil size={12} />
                    </button>
                    <button onClick={() => handleDelete(e.key)} title="删除" className="pd-btn p-1 rounded" style={{ color: 'var(--danger, #EF4444)' }}>
                      <Trash2 size={12} />
                    </button>
                  </div>
                </div>
              ),
            )
          )}

          {/* 分块：提示可见/总数；有剩余时才出现「加载更多」 */}
          {entries.length > 0 && (
            <div className="flex items-center justify-between gap-2 px-1 pt-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              <span>已显示 {Math.min(visibleCount, entries.length)} / 共 {entries.length} 条</span>
              {entries.length > visibleCount && (
                <SettingsButton onClick={() => setVisibleCount((v) => Math.min(v + PAGE_SIZE, entries.length))}>
                  加载更多（每批 {PAGE_SIZE}）
                </SettingsButton>
              )}
            </div>
          )}
        </div>
      )}
    </SettingsSection>
  );
}

/** 行内编辑表单（新增行/编辑行共用），替换该行的展示内容 */
function InlineRowEditor({
  form,
  setForm,
  saving,
  onSave,
  onCancel,
  isNew,
}: {
  form: InlineForm;
  setForm: (f: InlineForm | null) => void;
  saving: boolean;
  onSave: () => void;
  onCancel: () => void;
  isNew?: boolean;
}) {
  const change = (patch: Partial<InlineForm>) => setForm({ ...form, ...patch });
  return (
    <div className="rounded-lg px-2.5 py-2 flex flex-col gap-2" style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--accent)' }}>
      <div className="grid grid-cols-[minmax(140px,240px)_120px] gap-2 items-center">
        <input
          value={form.key}
          onChange={(e) => change({ key: e.target.value })}
          placeholder={isNew ? 'key（已存在的 key 将覆盖）' : 'key'}
          className="px-2 py-1.5 rounded-md text-xs outline-none"
          style={inputStyle}
        />
        <select
          value={form.category || 'fact'}
          onChange={(e) => change({ category: e.target.value })}
          className="px-2 py-1.5 rounded-md text-xs outline-none"
          style={inputStyle}
        >
          {CATEGORY_OPTIONS.map((c) => (
            <option key={c} value={c}>{c}</option>
          ))}
        </select>
      </div>
      <textarea
        value={form.value}
        onChange={(e) => change({ value: e.target.value })}
        placeholder="value（多行内容）"
        rows={3}
        className="px-2 py-1.5 rounded-md text-xs font-mono leading-relaxed outline-none resize-y"
        style={inputStyle}
      />
      <input
        value={form.tags}
        onChange={(e) => change({ tags: e.target.value })}
        placeholder="检索标签（可选，中英文逗号/顿号分隔，如 db,连接配置）"
        className="px-2 py-1.5 rounded-md text-xs outline-none"
        style={inputStyle}
      />
      <label className="flex items-center gap-1.5 text-xs cursor-pointer select-none" style={{ color: 'var(--text-secondary)' }}>
        <input
          type="checkbox"
          checked={form.important}
          onChange={(e) => change({ important: e.target.checked })}
        />
        重要记忆（pin 保护，自动清理永不删除）
      </label>
      <div className="flex items-center gap-2">
        <SettingsButton variant="primary" onClick={onSave} disabled={saving || !form.key.trim() || !form.value.trim()}>
          <Check size={12} /> {saving ? '保存中…' : '保存'}
        </SettingsButton>
        <SettingsButton onClick={onCancel}>
          <X size={12} /> 取消
        </SettingsButton>
      </div>
    </div>
  );
}
