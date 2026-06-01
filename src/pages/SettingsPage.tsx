import { useState, useRef, useCallback, useEffect } from 'react';
import {
  ArrowLeft, Settings, Globe, Cpu, Key, Info,
  Sun, Moon, Monitor, FolderOpen, ChevronDown,
  Plus, Trash2, Edit3, Check, X, Pencil,
  Loader2, Zap, GripVertical,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
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
import { useApiProviderStore, getApiKey } from '../stores/apiProviderStore';
import { sendApiRequest } from '../utils/apiClient';
import type { ApiProvider as StoreApiProvider } from '../stores/apiProviderStore';

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
  const [workspace, setWorkspace] = useState('');
  const [workspaceLoaded, setWorkspaceLoaded] = useState(false);

  // Load workspace from SQLite on mount
  useEffect(() => {
    (async () => {
      try {
        const val = await invoke<string | null>('get_app_setting', { key: 'pilotdesk-workspace' });
        if (val) setWorkspace(val);
      } catch { /* ignore */ }
      setWorkspaceLoaded(true);
    })();
  }, []);

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
        await invoke('set_app_setting', { key: 'pilotdesk-workspace', value: selected });
      }
    } catch {
      // User cancelled or dialog error — ignore
    }
  };

  const handleWorkspaceChange = (val: string) => {
    setWorkspace(val);
    // Debounce save to SQLite
    clearTimeout((handleWorkspaceChange as any)._timer);
    (handleWorkspaceChange as any)._timer = setTimeout(async () => {
      try { await invoke('set_app_setting', { key: 'pilotdesk-workspace', value: val }); } catch { /* ignore */ }
    }, 500);
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
            onChange={(e) => handleWorkspaceChange(e.target.value)}
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
  apiEndpoint: string;
  apiKeyMasked: string | null;
  apiKeySet: boolean;
  models: string[];
}

interface EditingProvider {
  id: string;
  name: string;
  apiEndpoint: string;
  apiKey: string;
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
  const fmt = endpoint.includes('anthropic.com') || providerId === 'anthropic' ? 'anthropic' : 'openai';
  const model = models[0] || (fmt === 'anthropic' ? 'claude-sonnet-4-20250514' : 'gpt-4o-mini');

  const result = await sendApiRequest({
    endpoint,
    providerId,
    apiKey,
    model,
    messages: [{ role: 'user', content: 'hi' }],
    maxTokens: 1,
    timeout: 15000,
  });

  if (result.ok) {
    return { ok: true, message: `连接成功 - 模型: ${model}`, latency: result.latency };
  }
  return { ok: false, message: result.message, latency: result.latency };
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
              {p.apiKeySet && !isEditing && (
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
                  disabled={testResult?.status === 'testing' || !p.apiKeySet}
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
                API URL
              </label>
              {isEditing ? (
                <input
                  type="text"
                  value={editingProvider!.apiEndpoint}
                  onChange={(e) =>
                    onSetEditingProvider({ ...editingProvider!, apiEndpoint: e.target.value })
                  }
                  className="w-full mt-0.5 px-2 py-1 rounded text-xs outline-none"
                  placeholder="完整URL，如 https://api.siliconflow.cn/v1/chat/completions"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
              ) : (
                <p className="text-xs mt-0.5 truncate" style={{ color: 'var(--text-secondary)' }}>
                  {p.apiEndpoint}
                </p>
              )}
            </div>

            {/* API Key */}
            {isEditing && (
              <div>
                <label className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                  API Key {p.apiKeySet && '（留空保持不变）'}
                </label>
                <input
                  type="password"
                  value={editingProvider!.apiKey}
                  onChange={(e) =>
                    onSetEditingProvider({ ...editingProvider!, apiKey: e.target.value })
                  }
                  placeholder={p.apiKeySet ? '输入新 Key 覆盖' : '输入 API Key'}
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
  const { providers, loading, fetchProviders, saveProvider, deleteProvider, reorderProviders } = useApiProviderStore();
  const [editingProvider, setEditingProvider] = useState<EditingProvider | null>(null);
  const [newModelInput, setNewModelInput] = useState('');
  const [testResults, setTestResults] = useState<Map<string, TestResult>>(new Map());
  const abortRef = useRef<Map<string, AbortController>>(new Map());

  // Load providers from SQLite on mount
  useEffect(() => {
    fetchProviders();
  }, []);

  // DnD sensors — use pointer sensor for drag, keyboard for accessibility
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  // Handle drag end — reorder via store
  const handleDragEnd = useCallback(async (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const ids = providers.map(p => p.id);
    const oldIndex = ids.indexOf(active.id as string);
    const newIndex = ids.indexOf(over.id as string);
    if (oldIndex === -1 || newIndex === -1) return;
    const reordered = arrayMove(ids, oldIndex, newIndex);
    await reorderProviders(reordered);
  }, [providers, reorderProviders]);

  // Add provider
  const handleAddProvider = async () => {
    const id = 'custom_' + Date.now();
    await saveProvider({
      id,
      name: '未命名提供商',
      apiEndpoint: '',
      models: [],
    });
    setEditingProvider({
      id,
      name: '未命名提供商',
      apiEndpoint: '',
      apiKey: '',
      models: '',
    });
  };

  // Delete provider
  const handleDeleteProvider = async (id: string) => {
    if (!confirm('确定删除该 API 提供商配置吗？')) return;
    await deleteProvider(id);
    if (editingProvider?.id === id) setEditingProvider(null);
    setTestResults((prev) => { const m = new Map(prev); m.delete(id); return m; });
  };

  // Start editing
  const handleStartEdit = (p: ApiProvider) => {
    setEditingProvider({
      id: p.id,
      name: p.name,
      apiEndpoint: p.apiEndpoint,
      apiKey: '',
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
  const handleSaveEdit = async () => {
    if (!editingProvider) return;
    const name = editingProvider.name.trim() || '未命名提供商';
    const endpoint = editingProvider.apiEndpoint.trim() || 'https://';
    const models = editingProvider.models
      .split(/[,，]/)
      .map((s) => s.trim())
      .filter(Boolean);

    await saveProvider({
      id: editingProvider.id,
      name,
      apiEndpoint: endpoint,
      apiKey: editingProvider.apiKey.trim() || undefined,
      models,
    });

    setEditingProvider(null);
    setNewModelInput('');
  };

  // Add model to existing provider (non-editing mode)
  const handleAddModelInline = async (providerId: string) => {
    if (!newModelInput.trim()) return;
    const p = providers.find(x => x.id === providerId);
    if (!p) return;
    await saveProvider({
      id: providerId,
      name: p.name,
      apiEndpoint: p.apiEndpoint,
      models: [...p.models, newModelInput.trim()],
    });
    setNewModelInput('');
  };

  // Remove model from provider
  const handleRemoveModel = async (providerId: string, model: string) => {
    const p = providers.find(x => x.id === providerId);
    if (!p) return;
    await saveProvider({
      id: providerId,
      name: p.name,
      apiEndpoint: p.apiEndpoint,
      models: p.models.filter((m) => m !== model),
    });
  };

  // Test connection for a provider
  const handleTestConnection = useCallback(async (providerId: string) => {
    const existing = abortRef.current.get(providerId);
    existing?.abort();

    const provider = providers.find((p) => p.id === providerId);
    if (!provider) return;

    setTestResults((prev) => new Map(prev).set(providerId, {
      providerId,
      status: 'testing',
      message: '正在测试连接...',
    }));

    let apiKey: string | null = null;
    try {
      apiKey = await getApiKey(providerId);
    } catch { /* ignore */ }

    if (!apiKey) {
      setTestResults((prev) => new Map(prev).set(providerId, {
        providerId,
        status: 'error',
        message: '未配置 API Key，请先编辑并填入 API Key',
      }));
      return;
    }

    const result = await testApiConnection(providerId, provider.apiEndpoint, apiKey, provider.models);

    setTestResults((prev) => new Map(prev).set(providerId, {
      providerId,
      status: result.ok ? 'success' : 'error',
      message: result.ok
        ? `${result.message} (${result.latency}ms)`
        : result.message,
      latency: result.latency,
    }));
  }, [providers]);

  if (loading && providers.length === 0) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 size={20} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
      </div>
    );
  }

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
        API Key 保存在本地 SQLite 数据库，不会上传。排序结果自动保存。
      </p>
    </div>
  );
}


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
