import { useState, useRef, useCallback } from 'react';
import {
  ArrowLeft, Settings, Globe, Cpu, Key, Info,
  Sun, Moon, Monitor, FolderOpen, ChevronDown,
  Plus, Trash2, Edit3, Check, X, Pencil,
  Loader2, Zap, GripVertical,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { useTheme } from '../hooks/useTheme';
import { EnvManager } from '../components/env/EnvManager';
import { ConfigEditor } from '../components/panels/ConfigEditor';

type SettingsTab = 'general' | 'environment' | 'agent' | 'api' | 'about';

interface SettingsPageProps {
  onBack: () => void;
}

const SETTINGS_TABS: { id: SettingsTab; icon: typeof Settings; label: string }[] = [
  { id: 'general', icon: Settings, label: '通用设置' },
  { id: 'environment', icon: Globe, label: '环境配置' },
  { id: 'agent', icon: Cpu, label: 'Agent 参数配置' },
  { id: 'api', icon: Key, label: 'API 配置' },
  { id: 'about', icon: Info, label: '关于' },
];

// ============================================================
// 1. General Settings
// ============================================================
function GeneralSettings() {
  const { theme, setTheme } = useTheme();
  const [language] = useState('zh-CN');
  const [workspace, setWorkspace] = useState(() =>
    localStorage.getItem('pilotdesk-workspace') || ''
  );

  const themeOptions = [
    { value: 'dark' as const, icon: Moon, label: '深色' },
    { value: 'light' as const, icon: Sun, label: '浅色' },
    { value: 'system' as const, icon: Monitor, label: '跟随系统' },
  ];

  const handlePickWorkspace = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (selected && typeof selected === 'string') {
        setWorkspace(selected);
        localStorage.setItem('pilotdesk-workspace', selected);
      }
    } catch {
      // User cancelled or dialog error — ignore
    }
  };

  return (
    <div className="space-y-6">
      {/* Theme */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          主题设置
        </h3>
        <div className="flex gap-1">
          {themeOptions.map(({ value, icon: Icon, label }) => (
            <button
              key={value}
              onClick={() => setTheme(value)}
              className="flex items-center gap-1 px-3 py-2 rounded-lg text-xs transition-colors"
              style={{
                color: theme === value ? '#fff' : 'var(--text-secondary)',
                backgroundColor: theme === value ? 'var(--accent)' : 'var(--bg-tertiary)',
                border: '1px solid var(--border)',
              }}
            >
              <Icon size={12} />
              {label}
            </button>
          ))}
        </div>
      </section>

      {/* Workspace directory */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          工作区目录
        </h3>
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={workspace}
            onChange={(e) => {
              setWorkspace(e.target.value);
              localStorage.setItem('pilotdesk-workspace', e.target.value);
            }}
            placeholder="设置默认工作区路径..."
            className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
          <button
            onClick={handlePickWorkspace}
            className="flex items-center gap-1 px-3 py-2 rounded-lg text-xs transition-colors"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
            title="浏览选择目录"
          >
            <FolderOpen size={12} />
            浏览
          </button>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          Claude Code / Hermes Agent 会话的默认工作目录。可直接输入路径或点击浏览选择。
        </p>
      </section>

      {/* Language */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          语言
        </h3>
        <div className="relative inline-block">
          <div
            className="flex items-center gap-2 px-3 py-2 rounded-lg text-sm cursor-default"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          >
            <span>简体中文</span>
            <ChevronDown size={12} style={{ color: 'var(--text-secondary)' }} />
          </div>
          <p className="text-xs mt-1" style={{ color: 'var(--text-tertiary)' }}>
            更多语言支持即将推出
          </p>
        </div>
      </section>
    </div>
  );
}

// ============================================================
// 3. Agent Configuration (with Claude + Hermes sub-tabs)
// ============================================================
type AgentSubTab = 'claude' | 'hermes';

const AGENT_SUB_TABS: { id: AgentSubTab; label: string; desc: string }[] = [
  { id: 'claude', label: 'Claude Code', desc: 'Anthropic Claude Code Agent 配置' },
  { id: 'hermes', label: 'Hermes Agent', desc: 'Hermes Agent 配置' },
];

function AgentConfigPanel() {
  const [activeSubTab, setActiveSubTab] = useState<AgentSubTab>('claude');

  return (
    <div className="h-full flex flex-col overflow-hidden min-h-0">
      {/* Sub-tab selector */}
      <div className="flex gap-2 mb-4">
        {AGENT_SUB_TABS.map(({ id, label, desc }) => (
          <button
            key={id}
            onClick={() => setActiveSubTab(id)}
            className="flex-1 px-3 py-2 rounded-lg text-xs transition-colors text-left"
            style={{
              backgroundColor: activeSubTab === id ? 'var(--bg-tertiary)' : 'transparent',
              border: activeSubTab === id
                ? '1px solid var(--accent)'
                : '1px solid var(--border)',
              color: activeSubTab === id ? 'var(--accent)' : 'var(--text-secondary)',
            }}
          >
            <div className="font-medium">{label}</div>
            <div className="text-[10px] mt-0.5 opacity-60">{desc}</div>
          </button>
        ))}
      </div>

      {/* Config editor */}
      <div className="flex-1 overflow-hidden" style={{ border: '1px solid var(--border)', borderRadius: '0.5rem' }}>
        <ConfigEditor agent={activeSubTab} />
      </div>
    </div>
  );
}

// ============================================================
// 4. API Configuration (list-based with edit/delete + test + drag-sort)
// ============================================================
interface ApiProvider {
  id: string;
  name: string;
  api_endpoint: string;
  api_key_masked: string | null;
  api_key_set: boolean;
  models: string[];
}

interface EditingProvider {
  id: string;
  name: string;
  api_endpoint: string;
  api_key: string;
  models: string;
}

type TestStatus = 'idle' | 'testing' | 'success' | 'error';
interface TestResult {
  providerId: string;
  status: TestStatus;
  message: string;
  latency?: number;
}

/** Determine API format from provider id or endpoint pattern */
function inferApiFormat(providerId: string, endpoint: string): 'anthropic' | 'openai' {
  if (providerId === 'anthropic' || endpoint.includes('anthropic.com')) {
    return 'anthropic';
  }
  return 'openai';
}

async function testApiConnection(
  providerId: string,
  endpoint: string,
  apiKey: string,
  models: string[],
): Promise<{ ok: boolean; message: string; latency: number }> {
  const ep = endpoint.replace(/\/+$/, '');
  const fmt = inferApiFormat(providerId, endpoint);
  const model = models[0] || (fmt === 'anthropic' ? 'claude-sonnet-4-20250514' : 'gpt-4o-mini');
  const start = performance.now();

  try {
    if (fmt === 'anthropic') {
      const res = await fetch(`${ep}/v1/messages`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'x-api-key': apiKey,
          'anthropic-version': '2023-06-01',
        },
        body: JSON.stringify({
          model,
          max_tokens: 1,
          stream: false,
          messages: [{ role: 'user', content: 'hi' }],
        }),
        signal: AbortSignal.timeout(15000),
      });

      const latency = Math.round(performance.now() - start);
      if (res.status === 401 || res.status === 403) {
        return { ok: false, message: `认证失败 (HTTP ${res.status})`, latency };
      }
      if (res.status === 404) {
        return { ok: false, message: `端点不存在 (HTTP 404)，请检查 API 地址`, latency };
      }
      if (!res.ok) {
        const errText = await res.text().catch(() => '');
        return { ok: false, message: `请求失败 (HTTP ${res.status}): ${errText.slice(0, 200)}`, latency };
      }
      const data = await res.json();
      if (data.type === 'error') {
        return { ok: false, message: `API 返回错误: ${data.error?.message || JSON.stringify(data.error)}`, latency };
      }
      return { ok: true, message: `连接成功 - 模型: ${model}`, latency };
    } else {
      const res = await fetch(`${ep}/chat/completions`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${apiKey}`,
        },
        body: JSON.stringify({
          model,
          max_tokens: 1,
          stream: false,
          messages: [{ role: 'user', content: 'hi' }],
        }),
        signal: AbortSignal.timeout(15000),
      });

      const latency = Math.round(performance.now() - start);
      if (res.status === 401 || res.status === 403) {
        return { ok: false, message: `认证失败 (HTTP ${res.status})`, latency };
      }
      if (res.status === 404) {
        return { ok: false, message: `端点不存在 (HTTP 404)，请检查 API 地址`, latency };
      }
      if (!res.ok) {
        const errText = await res.text().catch(() => '');
        return { ok: false, message: `请求失败 (HTTP ${res.status}): ${errText.slice(0, 200)}`, latency };
      }
      const data = await res.json();
      if (data.error) {
        return { ok: false, message: `API 返回错误: ${data.error.message || JSON.stringify(data.error)}`, latency };
      }
      return { ok: true, message: `连接成功 - 模型: ${model}`, latency };
    }
  } catch (err) {
    const latency = Math.round(performance.now() - start);
    const msg = err instanceof DOMException && err.name === 'TimeoutError'
      ? '连接超时 (15s)，请检查网络或端点地址'
      : `网络错误: ${err instanceof Error ? err.message : String(err)}`;
    return { ok: false, message: msg, latency };
  }
}

// ============================================================
// Sortable Provider Card
// ============================================================
function SortableProviderCard({
  provider,
  isEditing,
  testResult,
  editingProvider,
  newModelInput,
  onTestConnection,
  onStartEdit,
  onDeleteProvider,
  onSetEditingProvider,
  onSaveEdit,
  onCancelEdit,
  onAddModelInline,
  onRemoveModel,
  onNewModelInputChange,
}: {
  provider: ApiProvider;
  isEditing: boolean;
  testResult: TestResult | undefined;
  editingProvider: EditingProvider | null;
  newModelInput: string;
  onTestConnection: (id: string) => void;
  onStartEdit: (p: ApiProvider) => void;
  onDeleteProvider: (id: string) => void;
  onSetEditingProvider: (ep: EditingProvider) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onAddModelInline: (id: string) => void;
  onRemoveModel: (id: string, model: string) => void;
  onNewModelInputChange: (val: string) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: provider.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };

  const p = provider;

  return (
    <div
      ref={setNodeRef}
      style={style}
      className="rounded-lg overflow-hidden"
      data-provider-id={p.id}
    >
      <div
        className="flex"
        style={{
          border: isEditing ? '1px solid var(--accent)' : '1px solid var(--border)',
          backgroundColor: 'var(--bg-secondary)',
          borderRadius: '0.5rem',
        }}
      >
        {/* Drag handle */}
        <div
          className="flex items-center justify-center px-1.5 shrink-0 cursor-grab active:cursor-grabbing"
          style={{ borderRight: '1px solid var(--border)' }}
          {...attributes}
          {...listeners}
          title="拖拽排序"
        >
          <GripVertical size={12} style={{ color: 'var(--text-tertiary)' }} />
        </div>

        {/* Card content */}
        <div className="flex-1 min-w-0">
          {/* Card header */}
          <div
            className="flex items-center justify-between px-3 py-2"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <div className="flex items-center gap-2">
              <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                {isEditing ? (
                  <input
                    type="text"
                    value={editingProvider!.name}
                    onChange={(e) =>
                      onSetEditingProvider({ ...editingProvider!, name: e.target.value })
                    }
                    className="px-2 py-0.5 rounded text-xs outline-none"
                    style={{
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-primary)',
                      border: '1px solid var(--border)',
                      width: 180,
                    }}
                    placeholder="提供商名称"
                    autoFocus
                  />
                ) : (
                  p.name
                )}
              </span>
              {p.api_key_set && !isEditing && (
                <span
                  className="text-[10px] px-1.5 py-0.5 rounded"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--success)' }}
                >
                  已配置
                </span>
              )}
            </div>

            <div className="flex items-center gap-1">
              {!isEditing && (
                <button
                  onClick={() => onTestConnection(p.id)}
                  disabled={testResult?.status === 'testing' || !p.api_key_set}
                  className="flex items-center gap-1 px-2 py-0.5 rounded text-[10px] transition-colors disabled:opacity-30"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: testResult?.status === 'testing'
                      ? 'var(--text-tertiary)'
                      : 'var(--accent)',
                    border: '1px solid var(--border)',
                  }}
                  title="测试连接"
                >
                  {testResult?.status === 'testing' ? (
                    <Loader2 size={11} className="animate-spin" />
                  ) : (
                    <Zap size={11} />
                  )}
                  {testResult?.status === 'testing' ? '测试中' : '测试'}
                </button>
              )}

              {isEditing ? (
                <>
                  <button
                    onClick={onSaveEdit}
                    className="p-1 rounded transition-colors"
                    style={{ color: 'var(--success)' }}
                    title="保存"
                  >
                    <Check size={13} />
                  </button>
                  <button
                    onClick={onCancelEdit}
                    className="p-1 rounded transition-colors"
                    style={{ color: 'var(--text-secondary)' }}
                    title="取消"
                  >
                    <X size={13} />
                  </button>
                </>
              ) : (
                <>
                  <button
                    onClick={() => onStartEdit(p)}
                    className="p-1 rounded transition-colors"
                    style={{ color: 'var(--text-secondary)' }}
                    title="编辑"
                  >
                    <Pencil size={13} />
                  </button>
                  <button
                    onClick={() => onDeleteProvider(p.id)}
                    className="p-1 rounded transition-colors"
                    style={{ color: 'var(--danger)' }}
                    title="删除"
                  >
                    <Trash2 size={13} />
                  </button>
                </>
              )}
            </div>
          </div>

          {/* Test result banner */}
          {testResult && testResult.status !== 'idle' && testResult.status !== 'testing' && (
            <div
              className="px-3 py-1.5 text-[10px] flex items-center gap-1.5"
              style={{
                backgroundColor: testResult.status === 'success'
                  ? 'rgba(52, 211, 153, 0.08)'
                  : 'rgba(239, 68, 68, 0.08)',
                color: testResult.status === 'success'
                  ? 'var(--success)'
                  : 'var(--danger)',
              }}
            >
              <span className="font-medium">
                {testResult.status === 'success' ? '✓' : '✗'}
              </span>
              {testResult.message}
            </div>
          )}

          {/* Card body */}
          <div className="px-3 py-2 space-y-2">
            {/* API Endpoint */}
            <div>
              <label className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                API 端点
              </label>
              {isEditing ? (
                <input
                  type="text"
                  value={editingProvider!.api_endpoint}
                  onChange={(e) =>
                    onSetEditingProvider({ ...editingProvider!, api_endpoint: e.target.value })
                  }
                  className="w-full mt-0.5 px-2 py-1 rounded text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
              ) : (
                <p className="text-xs mt-0.5 truncate" style={{ color: 'var(--text-secondary)' }}>
                  {p.api_endpoint}
                </p>
              )}
            </div>

            {/* API Key */}
            {isEditing && (
              <div>
                <label className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                  API Key {p.api_key_set && '（留空保持不变）'}
                </label>
                <input
                  type="password"
                  value={editingProvider!.api_key}
                  onChange={(e) =>
                    onSetEditingProvider({ ...editingProvider!, api_key: e.target.value })
                  }
                  placeholder={p.api_key_set ? '输入新 Key 覆盖' : '输入 API Key'}
                  className="w-full mt-0.5 px-2 py-1 rounded text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
              </div>
            )}

            {/* Models */}
            <div>
              <label className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                可用模型
              </label>
              {isEditing ? (
                <div className="mt-0.5 space-y-1">
                  <input
                    type="text"
                    value={editingProvider!.models}
                    onChange={(e) =>
                      onSetEditingProvider({ ...editingProvider!, models: e.target.value })
                    }
                    placeholder="用逗号分隔模型名称，如: gpt-4o, gpt-4o-mini"
                    className="w-full px-2 py-1 rounded text-xs outline-none"
                    style={{
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-primary)',
                      border: '1px solid var(--border)',
                    }}
                  />
                </div>
              ) : (
                <div className="mt-1">
                  {p.models.length === 0 ? (
                    <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      暂无模型，点击编辑添加
                    </p>
                  ) : (
                    <div className="flex flex-wrap gap-1">
                      {p.models.map((model) => (
                        <span
                          key={model}
                          className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px] group"
                          style={{
                            backgroundColor: 'var(--bg-tertiary)',
                            color: 'var(--text-secondary)',
                            border: '1px solid var(--border)',
                          }}
                        >
                          {model}
                          <button
                            onClick={() => onRemoveModel(p.id, model)}
                            className="opacity-0 group-hover:opacity-60 transition-opacity"
                            style={{ color: 'var(--danger)' }}
                            title="移除"
                          >
                            <X size={9} />
                          </button>
                        </span>
                      ))}
                    </div>
                  )}

                  {/* Quick add model input */}
                  <div className="flex items-center gap-1 mt-1.5">
                    <input
                      type="text"
                      value={newModelInput}
                      onChange={(e) => onNewModelInputChange(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') onAddModelInline(p.id);
                      }}
                      placeholder="添加模型名称..."
                      className="flex-1 px-2 py-0.5 rounded text-[10px] outline-none"
                      style={{
                        backgroundColor: 'var(--bg-tertiary)',
                        color: 'var(--text-primary)',
                        border: '1px solid var(--border)',
                      }}
                    />
                    <button
                      onClick={() => onAddModelInline(p.id)}
                      disabled={!newModelInput.trim()}
                      className="px-1.5 py-0.5 rounded text-[10px] transition-colors disabled:opacity-30"
                      style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                    >
                      添加
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ApiConfig() {
  const [providers, setProviders] = useState<ApiProvider[]>(() => {
    const saved = localStorage.getItem('pd-api-providers');
    if (saved) {
      try { return JSON.parse(saved); } catch { /* ignore */ }
    }
    return [
      {
        id: 'anthropic',
        name: 'Anthropic Claude',
        api_endpoint: 'https://api.anthropic.com',
        api_key_masked: localStorage.getItem('pd-api-anthropic-masked'),
        api_key_set: !!localStorage.getItem('pd-api-anthropic-masked'),
        models: ['claude-sonnet-4-20250514', 'claude-opus-4-20250514'],
      },
      {
        id: 'openai',
        name: 'OpenAI',
        api_endpoint: 'https://api.openai.com/v1',
        api_key_masked: localStorage.getItem('pd-api-openai-masked'),
        api_key_set: !!localStorage.getItem('pd-api-openai-masked'),
        models: ['gpt-4o', 'gpt-4o-mini'],
      },
    ];
  });

  const [editingProvider, setEditingProvider] = useState<EditingProvider | null>(null);
  const [newModelInput, setNewModelInput] = useState('');
  const [testResults, setTestResults] = useState<Map<string, TestResult>>(new Map());
  const abortRef = useRef<Map<string, AbortController>>(new Map());

  // DnD sensors — use pointer sensor for drag, keyboard for accessibility
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  // Persist providers
  const persist = (list: ApiProvider[]) => {
    setProviders(list);
    localStorage.setItem('pd-api-providers', JSON.stringify(list));
  };

  // Handle drag end — reorder and persist
  const handleDragEnd = useCallback((event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    setProviders((prev) => {
      const oldIndex = prev.findIndex((p) => p.id === active.id);
      const newIndex = prev.findIndex((p) => p.id === over.id);
      if (oldIndex === -1 || newIndex === -1) return prev;
      const reordered = arrayMove(prev, oldIndex, newIndex);
      localStorage.setItem('pd-api-providers', JSON.stringify(reordered));
      return reordered;
    });
  }, []);

  // Add provider
  const handleAddProvider = () => {
    const id = 'custom_' + Date.now();
    const newP: ApiProvider = {
      id,
      name: '',
      api_endpoint: 'https://',
      api_key_masked: null,
      api_key_set: false,
      models: [],
    };
    const list = [...providers, newP];
    persist(list);
    setEditingProvider({
      id,
      name: '',
      api_endpoint: 'https://',
      api_key: '',
      models: '',
    });
  };

  // Delete provider
  const handleDeleteProvider = (id: string) => {
    if (!confirm('确定删除该 API 提供商配置吗？')) return;
    localStorage.removeItem(`pd-api-${id}-key`);
    localStorage.removeItem(`pd-api-${id}-masked`);
    persist(providers.filter((p) => p.id !== id));
    if (editingProvider?.id === id) setEditingProvider(null);
    setTestResults((prev) => { const m = new Map(prev); m.delete(id); return m; });
  };

  // Start editing
  const handleStartEdit = (p: ApiProvider) => {
    const key = localStorage.getItem(`pd-api-${p.id}-key`) || '';
    setEditingProvider({
      id: p.id,
      name: p.name,
      api_endpoint: p.api_endpoint,
      api_key: '',
      models: p.models.join(', '),
    });
    setNewModelInput('');
  };

  // Cancel editing
  const handleCancelEdit = () => {
    setEditingProvider(null);
    setNewModelInput('');
  };

  // Save editing
  const handleSaveEdit = () => {
    if (!editingProvider) return;
    const name = editingProvider.name.trim() || '未命名提供商';
    const endpoint = editingProvider.api_endpoint.trim() || 'https://';
    const models = editingProvider.models
      .split(/[,，]/)
      .map((s) => s.trim())
      .filter(Boolean);

    if (editingProvider.api_key.trim()) {
      const key = editingProvider.api_key.trim();
      const masked = key.slice(0, 4) + '****' + key.slice(-4);
      localStorage.setItem(`pd-api-${editingProvider.id}-key`, key);
      localStorage.setItem(`pd-api-${editingProvider.id}-masked`, masked);
      persist(
        providers.map((p) =>
          p.id === editingProvider.id
            ? { ...p, name, api_endpoint: endpoint, api_key_masked: masked, api_key_set: true, models }
            : p
        )
      );
    } else {
      persist(
        providers.map((p) =>
          p.id === editingProvider.id
            ? { ...p, name, api_endpoint: endpoint, models }
            : p
        )
      );
    }

    setEditingProvider(null);
    setNewModelInput('');
  };

  // Add model to existing provider (non-editing mode)
  const handleAddModelInline = (providerId: string) => {
    if (!newModelInput.trim()) return;
    persist(
      providers.map((p) =>
        p.id === providerId
          ? { ...p, models: [...p.models, newModelInput.trim()] }
          : p
      )
    );
    setNewModelInput('');
  };

  // Remove model from provider
  const handleRemoveModel = (providerId: string, model: string) => {
    persist(
      providers.map((p) =>
        p.id === providerId
          ? { ...p, models: p.models.filter((m) => m !== model) }
          : p
      )
    );
  };

  // Test connection for a provider
  const handleTestConnection = useCallback(async (providerId: string) => {
    const existing = abortRef.current.get(providerId);
    existing?.abort();

    const provider = providers.find((p) => p.id === providerId);
    if (!provider) return;

    const apiKey = localStorage.getItem(`pd-api-${providerId}-key`);
    if (!apiKey) {
      setTestResults((prev) => new Map(prev).set(providerId, {
        providerId,
        status: 'error',
        message: '未配置 API Key，请先编辑并填入 API Key',
      }));
      return;
    }

    setTestResults((prev) => new Map(prev).set(providerId, {
      providerId,
      status: 'testing',
      message: '正在测试连接...',
    }));

    const result = await testApiConnection(providerId, provider.api_endpoint, apiKey, provider.models);

    setTestResults((prev) => new Map(prev).set(providerId, {
      providerId,
      status: result.ok ? 'success' : 'error',
      message: result.ok
        ? `${result.message} (${result.latency}ms)`
        : result.message,
      latency: result.latency,
    }));
  }, [providers]);

  return (
    <div className="space-y-4">
      {/* Header with add button */}
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-xs font-semibold" style={{ color: 'var(--text-primary)' }}>
            API 提供商列表
          </h3>
          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            拖拽左侧手柄调整排序 · 配置完成后可在新建会话时选择 API 直连模式
          </p>
        </div>
        <button
          onClick={handleAddProvider}
          className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium transition-colors"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
          }}
        >
          <Plus size={12} />
          添加提供商
        </button>
      </div>

      {/* Sortable provider list */}
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={providers.map((p) => p.id)}
          strategy={verticalListSortingStrategy}
        >
          <div className="space-y-2">
            {providers.length === 0 && (
              <div className="text-center py-8">
                <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                  暂无 API 提供商，点击上方按钮添加
                </p>
              </div>
            )}

            {providers.map((p) => (
              <SortableProviderCard
                key={p.id}
                provider={p}
                isEditing={editingProvider?.id === p.id}
                testResult={testResults.get(p.id)}
                editingProvider={editingProvider}
                newModelInput={newModelInput}
                onTestConnection={handleTestConnection}
                onStartEdit={handleStartEdit}
                onDeleteProvider={handleDeleteProvider}
                onSetEditingProvider={setEditingProvider}
                onSaveEdit={handleSaveEdit}
                onCancelEdit={handleCancelEdit}
                onAddModelInline={handleAddModelInline}
                onRemoveModel={handleRemoveModel}
                onNewModelInputChange={setNewModelInput}
              />
            ))}
          </div>
        </SortableContext>
      </DndContext>

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        API Key 保存在本地，不会上传。排序结果自动保存。
      </p>
    </div>
  );
}

// ============================================================
// 5. About
// ============================================================
function AboutSection() {
  return (
    <div className="space-y-6">
      {/* App icon + name */}
      <div className="flex flex-col items-center gap-3 py-4">
        <img
          src="/logo-lg.png"
          alt="PilotDesk"
          className="w-16 h-16 rounded-2xl"
          draggable={false}
        />
        <div className="text-center">
          <h3 className="text-sm font-bold" style={{ color: 'var(--text-primary)' }}>
            PilotDesk
          </h3>
          <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
            v0.1.0
          </p>
        </div>
      </div>

      {/* Description */}
      <div className="rounded-lg p-3" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
        <p className="text-xs leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
          PilotDesk 是 Claude Code 与 Hermes Agent 的统一桌面客户端。支持多会话管理、
          API 直连对话、灵感收集、技能浏览器等能力，为日常 AI 协作提供一站式体验。
        </p>
      </div>

      {/* Tech stack */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          技术栈
        </h3>
        <div className="grid grid-cols-2 gap-2">
          {[
            { label: '前端框架', value: 'React 19 + TypeScript' },
            { label: 'UI 样式', value: 'TailwindCSS v4' },
            { label: '状态管理', value: 'Zustand' },
            { label: '桌面框架', value: 'Tauri 2.0' },
            { label: '后端语言', value: 'Rust' },
            { label: '本地存储', value: 'SQLite (rusqlite)' },
          ].map(({ label, value }) => (
            <div key={label} className="rounded-lg px-3 py-2" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
              <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{label}</div>
              <div className="text-xs font-medium mt-0.5" style={{ color: 'var(--text-primary)' }}>{value}</div>
            </div>
          ))}
        </div>
      </section>

      {/* Copyright */}
      <section
        className="rounded-lg px-3 py-3 text-center"
        style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
      >
        <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
          Copyright (c) 2026 PilotDesk by 简意工作室 (jorryn)
        </p>
        <p className="text-[10px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
          本项目代码基于 MIT 协议开源
        </p>
      </section>
    </div>
  );
}

// ============================================================
// Main SettingsPage
// ============================================================
export function SettingsPage({ onBack }: SettingsPageProps) {
  const [activeTab, setActiveTab] = useState<SettingsTab>('general');

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* Header */}
      <div
        className="shrink-0 px-4 py-3 flex items-center gap-3"
      >
        <button
          onClick={onBack}
          className="p-1 rounded transition-colors hover:opacity-80"
          style={{ color: 'var(--text-secondary)' }}
        >
          <ArrowLeft size={16} />
        </button>
        <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          设置
        </h2>
      </div>

      {/* Tab navigation */}
      <div
        className="shrink-0 px-4 flex gap-0.5 overflow-x-clip"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        {SETTINGS_TABS.map(({ id, icon: Icon, label }) => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className="flex items-center gap-1.5 px-3 py-2 text-xs rounded-t-lg transition-colors whitespace-nowrap"
            style={{
              color: activeTab === id ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: activeTab === id ? 'var(--bg-tertiary)' : 'transparent',
              borderBottom: activeTab === id ? '2px solid var(--accent)' : '2px solid transparent',
              marginBottom: '-1px',
            }}
          >
            <Icon size={12} />
            {label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-y-auto">
        <div className="p-4 max-w-2xl mx-auto w-full">
          {activeTab === 'general' && <GeneralSettings />}
          {activeTab === 'environment' && <EnvManager />}
          {activeTab === 'agent' && <AgentConfigPanel />}
          {activeTab === 'api' && <ApiConfig />}
          {activeTab === 'about' && <AboutSection />}
        </div>
      </div>
    </div>
  );
}
