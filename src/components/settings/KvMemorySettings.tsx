import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Search, Plus, Trash2, Pencil, X, Check, Star, Eraser } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import { Select } from '../common/Select';
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
import { errorMessage } from '../../utils/errorMessage';
import { confirmDialog } from '../../stores/confirmStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';

/** 分类候选：值仍是英文枚举（与 DB / 工具口径一致），展示时带中文备注便于理解 */
const CATEGORY_OPTIONS = ['fact', 'preference', 'skill', 'event'];
const CATEGORY_LABEL: Record<string, string> = {
  fact: '事实',
  preference: '偏好',
  skill: '技能',
  event: '事件',
};
/** `fact（事实）`：值保持不变，只在显示层加备注 */
const categoryLabel = (c: string) => (CATEGORY_LABEL[c] ? `${c}（${CATEGORY_LABEL[c]}）` : c);

/**
 * 解析 `memory_intent_model` 设置：
 * - 新格式 `{"providerId":"...","model":"..."}`；
 * - 旧格式（裸模型名）= 只覆盖模型名、沿用会话提供商，回显在 legacyModel 里提示用户重选一次。
 */
function parseIntentModelSetting(raw: string | null): {
  providerId: string;
  model: string;
  legacyModel: string;
} {
  const text = (raw ?? '').trim();
  if (!text) return { providerId: '', model: '', legacyModel: '' };
  try {
    const v = JSON.parse(text) as { providerId?: unknown; model?: unknown };
    return {
      providerId: typeof v.providerId === 'string' ? v.providerId.trim() : '',
      model: typeof v.model === 'string' ? v.model.trim() : '',
      legacyModel: '',
    };
  } catch {
    return { providerId: '', model: '', legacyModel: text };
  }
}

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

/** 置于 --bg-tertiary 面板内的输入框：面板本身就是该色，输入框须改用 --bg-primary 才能分辨。 */
const inputOnPanelStyle: CSSProperties = {
  backgroundColor: 'var(--bg-primary)',
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
  // 跨异步调用即时防重入（React 状态更新有延迟，连点在重渲染前二次进入）
  const busyRef = useRef(false);
  // 意图检索注入设置（app_settings：memory_intent_enabled / memory_intent_model / memory_intent_timeout_secs）
  const [intentEnabled, setIntentEnabled] = useState(true);
  // 路由模型覆盖 = 提供商 + 模型（与知识库「指定模型」同一套口径）：
  // 指定提供商后，endpoint / 格式 / Key 一并换，可以让路由单独走便宜的小模型。
  const { providers, fetchProviders } = useApiProviderStore();
  const [intentSaved, setIntentSaved] = useState<{ providerId: string; model: string }>({ providerId: '', model: '' });
  const [intentProviderId, setIntentProviderId] = useState('');
  const [intentModel, setIntentModel] = useState('');
  // 旧值（早期只存了一个裸模型名）：仍按"只覆盖模型名"生效，界面提示重选一次
  const [intentLegacyModel, setIntentLegacyModel] = useState('');
  const [intentModelSaving, setIntentModelSaving] = useState(false);
  const [intentTimeout, setIntentTimeout] = useState('');
  const [intentTimeoutDraft, setIntentTimeoutDraft] = useState('');

  // 读取意图检索设置：缺省视为启用；"0"/"false"/"off" 视为关闭
  useEffect(() => {
    void fetchProviders();
    (async () => {
      try {
        const [enabled, model, timeout] = await Promise.all([
          invoke<string | null>('get_app_setting', { key: 'memory_intent_enabled' }),
          invoke<string | null>('get_app_setting', { key: 'memory_intent_model' }),
          invoke<string | null>('get_app_setting', { key: 'memory_intent_timeout_secs' }),
        ]);
        const off = enabled != null && ['0', 'false', 'off'].includes(enabled.trim().toLowerCase());
        setIntentEnabled(!off);
        const parsed = parseIntentModelSetting(model);
        setIntentSaved({ providerId: parsed.providerId, model: parsed.model });
        setIntentProviderId(parsed.providerId);
        setIntentModel(parsed.model);
        setIntentLegacyModel(parsed.legacyModel);
        setIntentTimeout(timeout ?? '');
        setIntentTimeoutDraft(timeout ?? '');
      } catch {
        // 读取失败保持默认（启用，模型跟随会话），不影响其它功能
      }
    })();
  }, [fetchProviders]);

  const toggleIntent = async (next: boolean) => {
    setIntentEnabled(next);
    try {
      await invoke('set_app_setting', { key: 'memory_intent_enabled', value: next ? '1' : '0' });
      showToast(
        next
          ? '已启用意图检索注入（命中记忆与知识库时自动带入上下文）'
          : '已关闭意图检索注入（记忆与知识库都不再自动注入，模型仍可调用 search_memory 按需检索）',
        'success'
      );
    } catch (e) {
      setIntentEnabled(!next);
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

  const intentProvider = providers.find((p) => p.id === intentProviderId);
  const intentModels = intentProvider?.models ?? [];
  const intentProviderIsAnthropic = (intentProvider?.apiFormat ?? '').toLowerCase().includes('anthropic');
  const intentModelDirty =
    intentProviderId !== intentSaved.providerId || intentModel !== intentSaved.model;

  /** 保存路由模型覆盖；空值 = 清掉覆盖，提供商与模型都跟随当前会话 */
  const saveIntentModel = async (providerId: string, model: string) => {
    const value = providerId && model ? JSON.stringify({ providerId, model }) : '';
    setIntentModelSaving(true);
    try {
      await invoke('set_app_setting', { key: 'memory_intent_model', value });
      setIntentSaved({ providerId, model });
      setIntentProviderId(providerId);
      setIntentModel(model);
      setIntentLegacyModel('');
      showToast(
        value
          ? `意图路由模型已设为 ${providers.find((p) => p.id === providerId)?.name ?? providerId} · ${model}`
          : '已清空覆盖：意图路由跟随当前会话的提供商与模型',
        'success'
      );
    } catch (e) {
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    } finally {
      setIntentModelSaving(false);
    }
  };

  const resetIntentModel = () => void saveIntentModel('', '');

  const saveIntentTimeout = async () => {
    const value = intentTimeoutDraft.trim();
    if (value === intentTimeout.trim()) return;
    try {
      await invoke('set_app_setting', { key: 'memory_intent_timeout_secs', value });
      setIntentTimeout(value);
      showToast(value ? `意图路由超时已设为 ${value} 秒` : '已恢复为默认超时（60 秒）', 'success');
    } catch (e) {
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

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
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // 过滤条件变化：回到第一块并重新拉取（effect 体内不允许同步 setState：推迟一个微任务，观感与原先一致）
  useEffect(() => {
    queueMicrotask(() => {
      setVisibleCount(PAGE_SIZE);
      void refresh(category, query);
    });
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
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (key: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: `确定删除记忆「${key}」？删除后不可恢复。`,
      confirmText: '删除',
    });
    if (!ok) return;
    setError('');
    try {
      await deleteMemoryEntry(key);
      if (form?.rowKey === key) setForm(null);
      await refresh(category, query);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const handleTogglePin = async (key: string, pin: boolean) => {
    setError('');
    try {
      await setMemoryPin(key, pin);
      await refresh(category, query);
    } catch (e) {
      setError(errorMessage(e));
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
      showToast(`预览冷记忆失败: ${errorMessage(e)}`, 'error');
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
      showToast(`清理失败: ${errorMessage(e)}`, 'error');
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
        全局跨项目记忆，与 <code>search_memory</code> / <code>list_memory</code> / <code>save_memory</code> / <code>update_memory</code> / <code>delete_memory</code> 工具读写同一库；其中删除是高风险操作，每次调用都要你在会话里审批，且重要（pin）标记只免自动清理、<strong>不阻止删除</strong>。发送消息时，系统会先用一次轻量意图路由判断当前话题，命中相关记忆才注入模型上下文（重要/pin 记忆优先）；路由不可用或处于熔断冷却期时<strong>本轮不注入</strong>（模型仍可用 <code>search_memory</code> 按需检索）。<strong>知识库条目不在本列表</strong>——它们由「知识库」页管理（删除语义是"从库中移除"），但仍照旧参与记忆检索与注入。
      </p>

      {/* 意图检索注入设置：开关 + 可选路由模型覆盖 */}
      <div className="rounded-lg px-3 py-2 mb-2 text-[11px] space-y-2" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
        <label className="flex items-center gap-1.5 text-xs cursor-pointer select-none" style={{ color: 'var(--text-primary)' }}>
          <input
            type="checkbox"
            checked={intentEnabled}
            onChange={(e) => toggleIntent(e.target.checked)}
          />
          {/* 这条开关现在同时管两条链路（会话记忆 + 知识库），标签必须说出来 ——
              否则用户为了"别注入记忆"关掉它，会连带把知识库检索也关掉，而且看不出来 */}
          启用意图检索注入（按当前消息意图检索记忆与知识库）
        </label>
        <div className="flex items-center gap-2 flex-wrap">
          <span className="shrink-0" style={{ color: 'var(--text-secondary)' }}>意图路由模型</span>
          <Select
            value={intentProviderId}
            onChange={(v) => { setIntentProviderId(v); setIntentModel(''); }}
            options={[
              { value: '', label: '跟随当前会话' },
              ...providers.map((p) => ({
                value: p.id,
                label: (p.apiFormat ?? '').toLowerCase().includes('anthropic') ? `${p.name}（不支持路由）` : p.name,
              })),
            ]}
            placeholder="跟随当前会话"
            size="sm"
            title="路由用的提供商；指定后连接口地址与 Key 一并使用该提供商"
          />
          <Select
            value={intentModel}
            onChange={setIntentModel}
            options={intentModels.map((m) => ({ value: m, label: m }))}
            size="sm"
            placeholder={intentProviderId ? (intentModels.length ? '选择模型' : '该提供商下没有模型') : '跟随当前会话'}
            disabled={!intentProviderId || intentModels.length === 0}
            title="路由用的模型；建议选更小更快的模型以降低每条消息的前置开销"
          />
          <SettingsButton
            onClick={() => void saveIntentModel(intentProviderId, intentModel)}
            disabled={intentModelSaving || !intentModelDirty || !intentProviderId || !intentModel}
          >
            保存
          </SettingsButton>
          {intentSaved.providerId && (
            <SettingsButton onClick={resetIntentModel} disabled={intentModelSaving}>
              改为跟随会话
            </SettingsButton>
          )}
          {intentModelDirty && (
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>有未保存的改动</span>
          )}
        </div>
        {intentProviderIsAnthropic && (
          <div style={{ color: '#F59E0B' }}>
            该提供商是 Anthropic 原生格式：没有 /chat/completions 端点，意图路由调用会被判为不可用（本轮不注入）。请改选 OpenAI 兼容的提供商。
          </div>
        )}
        {intentLegacyModel && (
          <div style={{ color: 'var(--text-tertiary)' }}>
            当前是旧格式配置（只记了模型名「{intentLegacyModel}」，提供商跟随会话）。重新选择提供商与模型并保存后，会按新格式记录。
          </div>
        )}
        <div className="flex items-center gap-2">
          <span className="shrink-0" style={{ color: 'var(--text-secondary)' }}>路由超时（秒）</span>
          <input
            value={intentTimeoutDraft}
            onChange={(e) => setIntentTimeoutDraft(e.target.value)}
            onBlur={saveIntentTimeout}
            placeholder="留空 = 60（合法 1~60）"
            className="w-40 px-2 py-1 rounded-md text-xs outline-none"
            style={inputOnPanelStyle}
          />
        </div>
        <div style={{ color: 'var(--text-tertiary)' }}>
          关闭后不再发起意图路由调用，KV 记忆不再自动注入；<strong>MEMORY.md / USER.md 仍随 system prompt 注入</strong>，模型仍可调用 <code>search_memory</code> 按需检索。路由模型默认跟随当前会话，也可以单独指定提供商 + 模型（建议选更小/更快的模型以降低开销，例如主对话用大模型、路由用便宜的小模型）；路由超时是<strong>上限而非等待时长</strong> —— 模型返回即继续（实际可能只需 1~2 秒），只有上游卡住才会等满，因此填大不会让每条消息都变慢。
        </div>
      </div>

      {stats && policy && (
        <div className="rounded-lg px-3 py-2 mb-2 text-[11px] space-y-1" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          <div style={{ color: 'var(--text-primary)' }}>
            共 {stats.total} 条 · pin 保护 {stats.pinned} 条 · 自动维护候选 {stats.candidates} 条
          </div>
          <div style={{ color: 'var(--text-tertiary)' }}>
            自动维护规则：总条数超 {policy.maxEntries} 条时按“最久未活跃”驱逐；超过 {policy.idleDays} 天既未被检索也未被编辑、且访问次数 ≤ {policy.minAccess} 次的未 pin 条目会被清理（pin 条目永不自动删除）。<strong>知识库条目不参与以上两项自动维护</strong>——它们既不占 {policy.maxEntries} 条配额，也不会被判为冷记忆；生命周期只由所属知识库决定（删库或从库中移除，且仅当不再被任何库关联时才连同条目删除）。
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
            placeholder="搜索 key/标签…"
            className="w-full pl-7 px-2.5 py-1.5 rounded-lg text-xs outline-none"
            style={inputStyle}
          />
        </div>
        <Select
          value={category}
          onChange={(v) => setCategory(v)}
          options={[
            { value: '', label: '全部分类' },
            ...CATEGORY_OPTIONS.map((c) => ({ value: c, label: categoryLabel(c) })),
          ]}
          placeholder="全部分类"
          size="sm"
        />
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
        <Select
          value={form.category || 'fact'}
          onChange={(v) => change({ category: v })}
          options={CATEGORY_OPTIONS.map((c) => ({ value: c, label: categoryLabel(c) }))}
          className="w-full"
          size="sm"
        />
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
