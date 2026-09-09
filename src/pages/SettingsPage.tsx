import { useState, useRef, useCallback, useEffect } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import {
  ArrowLeft, Settings, Globe, Key, Info, Bot, Brain,
  Sun, Moon, Monitor, FolderOpen, ChevronDown,
  Plus, Trash2, Edit3, Check, X, Pencil,
  Loader2, Zap, GripVertical, ShieldCheck, Plug, Search, Bookmark, Wrench, History,
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
import { ThemeCustomizer } from '../components/settings/ThemeCustomizer';
import { EnvManager } from '../components/env/EnvManager';
import { AgentManager } from '../components/env/AgentManager';
import { UpdateChecker } from '../components/panels/UpdateChecker';
import { ModePromptSettings } from '../components/panels/ModePromptSettings';
import { PermissionSettings } from '../components/settings/AgentPermissionSettings';
import { McpSettings } from '../components/settings/McpSettings';
import { SearchSettings } from '../components/settings/SearchSettings';
import { FileHistorySettings } from '../components/settings/FileHistorySettings';
import { useApiProviderStore, getApiKey } from '../stores/apiProviderStore';
import { sendApiRequest } from '../utils/apiClient';
import type { ApiProvider as StoreApiProvider } from '../stores/apiProviderStore';
import { useTerminal, type ViewMode } from '../TerminalManager';

interface SettingsPageProps {
  onBack: () => void;
}

type SettingsTab = 'general' | 'environment' | 'agents' | 'api' | 'mode' | 'permission' | 'mcp' | 'search' | 'tools' | 'customtabs' | 'filehistory' | 'memory' | 'about';

interface SettingsTabItem {
  id: SettingsTab;
  icon: typeof Settings;
  label: string;
}

// 设置项按类别分组，左侧侧边栏渲染（避免 11 个 tab 平铺顶部拥挤）
const SETTINGS_GROUPS: { title: string; items: SettingsTabItem[] }[] = [
  {
    title: '应用',
    items: [
      { id: 'general', icon: Settings, label: '通用设置' },
      { id: 'environment', icon: Globe, label: '环境检测' },
      { id: 'about', icon: Info, label: '关于' },
    ],
  },
  {
    title: 'Agent 与 API',
    items: [
      { id: 'agents', icon: Bot, label: 'Agent集成配置' },
      { id: 'api', icon: Key, label: 'API集成配置' },
      { id: 'mode', icon: Zap, label: '对话模式' },
      { id: 'permission', icon: ShieldCheck, label: '权限规则' },
    ],
  },
  {
    title: '连接',
    items: [
      { id: 'mcp', icon: Plug, label: 'MCP 服务器' },
      { id: 'search', icon: Search, label: '联网搜索' },
    ],
  },
  {
    title: '工具与扩展',
    items: [
      { id: 'tools', icon: Wrench, label: '工具管理' },
      { id: 'customtabs', icon: Bookmark, label: '自定义标签' },
    ],
  },
  {
    title: '数据管理',
    items: [
      { id: 'memory', icon: Brain, label: '记忆管理' },
      { id: 'filehistory', icon: History, label: '文件历史' },
    ],
  },
];



import { SettingsSection, SettingsButton } from '../components/settings';
import { UsageStats } from '../components/settings/UsageStats';
import { CustomTabsSettings } from '../components/settings/CustomTabsSettings';
import { ProjectMemorySettings } from '../components/settings/ProjectMemorySettings';
import { UserMemorySettings } from '../components/settings/UserMemorySettings';
import { KvMemorySettings } from '../components/settings/KvMemorySettings';
import { ToolSettings } from '../components/settings/ToolSettings';
import { TitleBar, StatusBar } from '../components/layout';

// ============================================================
// 1. General Settings
// ============================================================
function GeneralSettings() {
  const { theme, setTheme } = useTheme();
  const [language] = useState('zh-CN');
  const [workspace, setWorkspace] = useState('');
  const [workspaceLoaded, setWorkspaceLoaded] = useState(false);
  const [maxConcurrency, setMaxConcurrency] = useState(10);
  const [maxSubflowDepth, setMaxSubflowDepth] = useState(3);
  const [streamIdleSecs, setStreamIdleSecs] = useState(90);

  // Load workspace from SQLite on mount
  useEffect(() => {
    (async () => {
      try {
        const val = await invoke<string | null>('get_app_setting', { key: 'pilotdesk-workspace' });
        if (val) setWorkspace(val);
      } catch { /* ignore */ }
      try {
        const mc = await invoke<number | null>('get_workflow_max_concurrency');
        if (mc) setMaxConcurrency(mc);
      } catch { /* ignore */ }
      try {
        const msd = await invoke<string | null>('get_app_setting', { key: 'workflow_max_subflow_depth' });
        if (msd) setMaxSubflowDepth(parseInt(msd));
      } catch { /* ignore */ }
      try {
        const idle = await invoke<string | null>('get_app_setting', { key: 'api_stream_idle_secs' });
        if (idle && !Number.isNaN(parseInt(idle))) setStreamIdleSecs(parseInt(idle));
      } catch { /* ignore */ }
      setWorkspaceLoaded(true);
    })();
  }, []);

  const themeOptions = [
    { value: 'dark' as const, icon: Moon, label: '深色' },
    { value: 'light' as const, icon: Sun, label: '浅色' },
    { value: 'system' as const, icon: Monitor, label: '跟随系统' },
  ];

  const handleMaxConcurrencyChange = async (value: number) => {
    const clamped = Math.max(1, Math.min(10, value));
    setMaxConcurrency(clamped);
    try {
      await invoke('set_workflow_max_concurrency', { maxConcurrency: clamped });
    } catch { /* ignore */ }
  };

  const handleMaxSubflowDepthChange = async (value: number) => {
    const clamped = Math.max(1, Math.min(10, value));
    setMaxSubflowDepth(clamped);
    try {
      await invoke('set_app_setting', { key: 'workflow_max_subflow_depth', value: clamped.toString() });
    } catch { /* ignore */ }
  };

  const handleStreamIdleSecsChange = async (value: number) => {
    if (Number.isNaN(value)) return;
    // 0 = 禁用空闲检测（调试用）；其余钳制在 10-600 秒。
    const clamped = value === 0 ? 0 : Math.max(10, Math.min(600, value));
    setStreamIdleSecs(clamped);
    try {
      await invoke('set_app_setting', { key: 'api_stream_idle_secs', value: clamped.toString() });
    } catch { /* ignore */ }
  };

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



  return (
    <div className="space-y-6">
      {/* Theme */}
      <SettingsSection title="主题模式">
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
        <div className="mt-3">
          <ThemeCustomizer />
        </div>
      </SettingsSection>

      {/* Workspace directory */}
      <SettingsSection title="工作区目录">
        <div className="flex items-center gap-2">
          <div
            className="flex-1 px-3 py-2 rounded-lg text-sm truncate"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            title={workspace || '未设置'}
          >
            {workspace || '未设置'}
          </div>
          <SettingsButton onClick={handlePickWorkspace} icon={<FolderOpen size={12} />}>
            浏览
          </SettingsButton>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          Agent 会话的默认工作目录。Agent 可在该目录下创建和修改工作产物文件。
        </p>
      </SettingsSection>

      {/* Language */}
      <SettingsSection title="语言">
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
      </SettingsSection>

      {/* Max Concurrency */}
      <SettingsSection title="并行节点数">
        <div className="flex items-center gap-2">
          <input
            type="range"
            min="1"
            max="20"
            value={maxConcurrency}
            onChange={(e) => handleMaxConcurrencyChange(parseInt(e.target.value))}
            className="flex-1"
            style={{ accentColor: 'var(--accent)' }}
          />
          <span
            className="px-2 py-1 rounded text-xs font-mono min-w-[24px] text-center"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
          >
            {maxConcurrency}
          </span>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          工作流执行时同一时刻最多并行运行的节点数。增加此值可加速并行节点较多的工作流，但会消耗更多系统资源。
        </p>
      </SettingsSection>

      {/* Max Subflow Depth */}
      <SettingsSection title="子工作流嵌套深度">
        <div className="flex items-center gap-2">
          <input
            type="range"
            min="1"
            max="10"
            value={maxSubflowDepth}
            onChange={(e) => handleMaxSubflowDepthChange(parseInt(e.target.value))}
            className="flex-1"
            style={{ accentColor: 'var(--accent)' }}
          />
          <span
            className="px-2 py-1 rounded text-xs font-mono min-w-[24px] text-center"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
          >
            {maxSubflowDepth}
          </span>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          子工作流（Subflow 节点）允许的最大递归嵌套层数（1-10）。超过此限制将阻止执行以防止无限递归。
        </p>
      </SettingsSection>

      {/* 模型流式空闲超时 */}
      <SettingsSection title="模型流式空闲超时">
        <div className="flex items-center gap-2">
          <input
            type="number"
            min={0}
            max={600}
            step={10}
            value={streamIdleSecs}
            onChange={(e) => handleStreamIdleSecsChange(parseInt(e.target.value))}
            className="flex-1 px-3 py-2 rounded-lg text-sm font-mono"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
          />
          <span className="text-xs whitespace-nowrap" style={{ color: 'var(--text-tertiary)' }}>秒</span>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          流式响应超过该秒数仍未收到任何数据时，判定为上游连接假死并终止本次调用（会话与群聊共用）。0 表示关闭空闲检测（调试用）；合法范围 10-600，默认 90。
        </p>
      </SettingsSection>
    </div>
  );
}
// ============================================================
// 4. API Configuration
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
  apiFormat?: 'openai' | 'anthropic';
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
  editingNotes,
  onTestConnection,
  onStartEdit,
  onDeleteProvider,
  onSetEditingProvider,
  onSaveEdit,
  onCancelEdit,
  onAddModelInline,
  onRemoveModel,
  onEditingNotesChange,
}: {
  provider: ApiProvider;
  isEditing: boolean;
  testResult: TestResult | undefined;
  editingProvider: EditingProvider | null;
  editingNotes: string;
  onTestConnection: (id: string) => void;
  onStartEdit: (p: ApiProvider) => void;
  onDeleteProvider: (id: string) => void;
  onSetEditingProvider: (ep: EditingProvider) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onAddModelInline: (id: string, raw: string) => void;
  onRemoveModel: (id: string, model: string) => void;
  onEditingNotesChange: (val: string) => void;
}) {
  // 快捷添加模型输入：卡片内本地状态（勿提为父级共享，否则一张卡片输入会联动到所有卡片）。
  const [newModelInput, setNewModelInput] = useState('');
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
        {/* Card content */}
        <div className="flex-1 min-w-0">
          {/* Card header */}
          <div
            className="flex items-center justify-between px-3 py-2"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <div className="flex items-center gap-2">
              <div
                className="flex items-center justify-center cursor-grab active:cursor-grabbing"
                {...attributes}
                {...listeners}
                title="拖拽排序"
              >
                <GripVertical size={12} style={{ color: 'var(--text-tertiary)' }} />
              </div>
              <span className="text-xs " style={{ color: 'var(--text-primary)' }}>
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
                    className="pd-btn p-1 rounded transition-colors"
                    style={{ color: 'var(--success)' }}
                    title="保存"
                  >
                    <Check size={13} />
                  </button>
                  <button
                    onClick={onCancelEdit}
                    className="pd-btn p-1 rounded transition-colors"
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
              <span className="">
                {testResult.status === 'success' ? '✓' : '✗'}
              </span>
              {testResult.message}
            </div>
          )}

          {/* Card body */}
          <div className="px-3 py-2 space-y-2">
            {/* API Endpoint */}
            <div>
              <label className="text-[10px] " style={{ color: 'var(--text-tertiary)' }}>
                API Base URL
              </label>
              {isEditing ? (
                <input
                  type="text"
                  value={editingProvider!.apiEndpoint}
                  onChange={(e) =>
                    onSetEditingProvider({ ...editingProvider!, apiEndpoint: e.target.value })
                  }
                  className="w-full mt-0.5 px-2 py-1 rounded text-xs outline-none"
                  placeholder="Base URL，如 https://api.siliconflow.cn/v1（部分服务含 /v1；不要拼 /chat/completions 等接口路径）"
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
                <label className="text-[10px] " style={{ color: 'var(--text-tertiary)' }}>
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
              <label className="text-[10px] " style={{ color: 'var(--text-tertiary)' }}>
                可用模型
              </label>
              {isEditing ? (
                <div className="mt-0.5">
                  <textarea
                    value={editingNotes}
                    onChange={(e) => onEditingNotesChange(e.target.value)}
                    rows={5}
                    placeholder="每行一个模型：模型名 = 备注（备注可选，供 LLM 选择模型时参考；如：dall-e-3 = 文生图/图生图）"
                    className="w-full px-2 py-1 rounded text-xs outline-none resize-y"
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
                      onChange={(e) => setNewModelInput(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          onAddModelInline(p.id, newModelInput);
                          setNewModelInput('');
                        }
                      }}
                      placeholder="模型名 = 备注（逗号分隔可加多个）"
                      className="flex-1 px-2 py-0.5 rounded text-[10px] outline-none"
                      style={{
                        backgroundColor: 'var(--bg-tertiary)',
                        color: 'var(--text-primary)',
                        border: '1px solid var(--border)',
                      }}
                    />
                    <button
                      onClick={() => {
                        onAddModelInline(p.id, newModelInput);
                        setNewModelInput('');
                      }}
                      disabled={!newModelInput.trim()}
                      className="px-1.5 py-0.5 rounded text-[10px] transition-colors disabled:opacity-30"
                      style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                    >
                      添加
                    </button>
                  </div>
                </div>
              )}

              {/* API Format */}
              {isEditing && (
                <div>
                  <label className="text-[10px] " style={{ color: 'var(--text-tertiary)' }}>
                    API 协议格式
                  </label>
                  <select
                    value={editingProvider!.apiFormat}
                    onChange={(e) =>
                      onSetEditingProvider({ ...editingProvider!, apiFormat: e.target.value as 'openai' | 'anthropic' })
                    }
                    className="w-full mt-0.5 px-2 py-1 rounded text-xs outline-none"
                    style={{
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-primary)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    <option value="openai">OpenAI 兼容（默认）</option>
                    <option value="anthropic">Anthropic 原生</option>
                  </select>
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
  // 模型备注：providerId → modelName → 用途描述（存 app_settings，供 list_models 工具带给 LLM）
  const [modelNotes, setModelNotes] = useState<Record<string, Record<string, string>>>({});
  const [editingNotes, setEditingNotes] = useState('');
  const [testResults, setTestResults] = useState<Map<string, TestResult>>(new Map());
  const abortRef = useRef<Map<string, AbortController>>(new Map());
  const [apiTab, setApiTab] = useState<'providers' | 'usage'>('providers');

  // Load providers from SQLite on mount
  useEffect(() => {
    fetchProviders();
    invoke<Record<string, Record<string, string>>>('get_model_notes_cmd')
      .then(setModelNotes)
      .catch(() => {});
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
    const notes = modelNotes[p.id] ?? {};
    // 无备注的模型补全「=」，保证每行统一为「模型名 = 备注」格式
    setEditingNotes(p.models.map((m) => (notes[m] ? `${m} = ${notes[m]}` : `${m} =`)).join('\n'));
  };

  // Cancel editing
  const handleCancelEdit = () => {
    setEditingProvider(null);
    setEditingNotes('');
  };

  // Save editing
  const handleSaveEdit = async () => {
    if (!editingProvider) return;
    const name = editingProvider.name.trim() || '未命名提供商';
    const endpoint = editingProvider.apiEndpoint.trim() || 'https://';

    // 可用模型统一从备注行输入解析（每行：模型名 = 备注）；备注为空字符串时正常保存
    const models: string[] = [];
    const notesForProvider: Record<string, string> = {};
    for (const line of editingNotes.split('\n')) {
      const idx = line.indexOf('=');
      const mname = (idx >= 0 ? line.slice(0, idx) : line).trim();
      const note = (idx >= 0 ? line.slice(idx + 1) : '').trim();
      if (mname) {
        if (!models.includes(mname)) models.push(mname);
        notesForProvider[mname] = note;
      }
    }

    // 保留原 sortOrder：upsert 缺省会把 sort_order 重置为当前时间戳，导致编辑后卡片跳到列表末尾。
    const original = providers.find((x) => x.id === editingProvider.id);

    await saveProvider({
      id: editingProvider.id,
      name,
      apiEndpoint: endpoint,
      apiKey: editingProvider.apiKey.trim() || undefined,
      models,
      apiFormat: editingProvider.apiFormat,
      sortOrder: original?.sortOrder,
    });

    // 备注持久化到 app_settings
    const next = { ...modelNotes, [editingProvider.id]: notesForProvider };
    setModelNotes(next);
    try {
      await invoke('set_model_notes_cmd', { notes: next });
    } catch { /* ignore */ }

    setEditingProvider(null);
    setEditingNotes('');
  };

  // Add model to existing provider (non-editing mode)
  // 支持「模型名 = 备注」格式，逗号（中英文）分隔多组：模型名 = 备注, 模型名 = 备注……
  const handleAddModelInline = async (providerId: string, raw: string) => {
    const trimmed = (raw ?? '').trim();
    if (!trimmed) return;
    const p = providers.find(x => x.id === providerId);
    if (!p) return;

    const added: string[] = [];
    const newNotes: Record<string, string> = {};
    for (const part of trimmed.split(/[,，]/).map((s) => s.trim()).filter(Boolean)) {
      const idx = part.indexOf('=');
      const mname = (idx >= 0 ? part.slice(0, idx) : part).trim();
      const note = (idx >= 0 ? part.slice(idx + 1) : '').trim();
      if (mname) {
        if (!p.models.includes(mname) && !added.includes(mname)) added.push(mname);
        newNotes[mname] = note;
      }
    }
    if (added.length === 0 && Object.keys(newNotes).length === 0) return;

    await saveProvider({
      id: providerId,
      name: p.name,
      apiEndpoint: p.apiEndpoint,
      models: [...p.models, ...added],
      // 保留原 sortOrder，避免快捷添加后卡片跳到列表末尾。
      sortOrder: p.sortOrder,
    });

    // 备注同步（含纯备注更新场景）
    if (Object.keys(newNotes).length > 0) {
      const base = { ...(modelNotes[p.id] ?? {}) };
      for (const [k, v] of Object.entries(newNotes)) base[k] = v;
      const next = { ...modelNotes, [p.id]: base };
      setModelNotes(next);
      try {
        await invoke('set_model_notes_cmd', { notes: next });
      } catch { /* ignore */ }
    }
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
      // 保留原 sortOrder，避免移除模型后卡片跳到列表末尾。
      sortOrder: p.sortOrder,
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
      {/* 子 Tab：提供商列表 / 用量统计（与「工具管理」一致） */}
      <div className="flex items-center gap-1">
        {([
          { key: 'providers', label: 'API 提供商列表' },
          { key: 'usage', label: '用量统计' },
        ] as const).map((t) => (
          <button
            key={t.key}
            onClick={() => setApiTab(t.key)}
            className={'pd-tab' + (apiTab === t.key ? ' pd-tab-active' : '')}
          >
            {t.label}
          </button>
        ))}
      </div>

      {apiTab === 'providers' && (
        <>
      {/* Header with add button */}
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
            提供商配置
          </h3>
          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            拖拽左侧手柄调整排序 · 配置完成后可在新建会话时选择 API 直连模式
          </p>
        </div>
        <button
          onClick={handleAddProvider}
          className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs  transition-colors"
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
                editingNotes={editingNotes}
                onTestConnection={handleTestConnection}
                onStartEdit={handleStartEdit}
                onDeleteProvider={handleDeleteProvider}
                onSetEditingProvider={setEditingProvider}
                onSaveEdit={handleSaveEdit}
                onCancelEdit={handleCancelEdit}
                onAddModelInline={handleAddModelInline}
                onRemoveModel={handleRemoveModel}
                onEditingNotesChange={setEditingNotes}
              />
            ))}
          </div>
        </SortableContext>
      </DndContext>

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        API Key 保存在本地 SQLite 数据库，不会上传。排序结果自动保存。
      </p>
        </>
      )}

      {apiTab === 'usage' && <UsageStats />}
    </div>
  );
}


// ============================================================
// 5. About
// ============================================================
function AboutSection() {
  const { theme, setTheme } = useTheme();

  return (
    <div className="flex flex-col items-center gap-6 py-8">
      {/* Logo */}
      <img
        src="/logo-lg.png"
        alt="PilotDesk"
        className="w-20 h-20 rounded-2xl"
        draggable={false}
      />

      {/* App name + version */}
      <div className="text-center">
        <h3 className="text-lg font-semibold" style={{ color: 'var(--text-primary)' }}>
          PilotDesk
        </h3>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          v0.1.0
        </p>
      </div>

      {/* Description */}
      <p className="text-xs text-center leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
        Agent 统一桌面客户端。
        集成多 Agent 管理、流式对话、灵感市集、API 直连等功能。
      </p>

      {/* Update Checker */}
      <div className="w-full">
        <UpdateChecker />
      </div>

      {/* Tech stack */}
      <div
        className="grid grid-cols-2 gap-2 w-full"
        style={{ fontSize: '11px' }}
      >
        {[
          ['前端', 'React 19 + TypeScript + TailwindCSS v4'],
          ['桌面', 'Tauri 2.0'],
          ['后端', 'Rust + SQLite'],
          ['Agent', 'Rust AgentManager + Tauri Event'],
        ].map(([label, value]) => (
          <div
            key={label}
            className="flex items-center gap-2 px-3 py-2 rounded-lg"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <span style={{ color: 'var(--text-tertiary)' }}>{label}</span>
            <span>{value}</span>
          </div>
        ))}
      </div>

      {/* Copyright */}
      <div className="text-center text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
        <p>Copyright &copy; @简意工作室（jorryn）</p>
        <p className="mt-1">本项目基于 MIT License 开源</p>
      </div>
    </div>
  );
}

export function SettingsPage({ onBack }: SettingsPageProps) {
  const [searchParams] = useSearchParams();
  const urlTab = searchParams.get('tab');
  const [activeTab, setActiveTab] = useState<SettingsTab>(() => {
    if (urlTab === 'environment') return 'environment';
    if (urlTab === 'agents') return 'agents';
    if (urlTab === 'customtabs') return 'customtabs';
    if (urlTab === 'memory') return 'memory';
    return 'general';
  });
  // URL 中 tab 参数变化时同步激活对应 tab（支持 /settings?tab=xxx 直接定位）
  useEffect(() => {
    if (urlTab === 'environment' || urlTab === 'agents' || urlTab === 'customtabs' || urlTab === 'memory') {
      setActiveTab(urlTab);
    }
  }, [urlTab]);

  // 记忆管理页内部子 tab（项目记忆 / 全局 KV 记忆）
  const [memTab, setMemTab] = useState<'project' | 'user' | 'kv'>('project');

  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();
  // 组合开关点击：切换模式并回到主布局（设置页不再提供返回按钮）
  const handleModeChange = useCallback((mode: ViewMode) => {
    setMode(mode);
    navigate('/');
  }, [setMode, navigate]);

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* TitleBar：组合开关注入「设置」段（thumb 定位到设置），点击其它模式段跳回主布局 */}
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        onToggleRightPanel={undefined}
        rightPanelOpen={false}
        showBackButton={false}
        settingsOpen
        onOpenSettings={() => { /* 已在设置页，无需动作 */ }}
      />

      {/* 主体：左侧分组侧边栏 + 右侧内容区 */}
      <div className="flex flex-1 overflow-hidden">
        {/* 侧边栏导航 */}
        <aside
          className="shrink-0 w-44 overflow-y-auto px-2 py-3 space-y-4"
          style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
        >
          {SETTINGS_GROUPS.map((group) => (
            <div key={group.title}>
              <div className="px-2 pb-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                {group.title}
              </div>
              <div className="space-y-0.5">
                {group.items.map(({ id, icon: Icon, label }) => (
                  <button
                    key={id}
                    onClick={() => setActiveTab(id)}
                    className={"pd-side-item" + (activeTab === id ? " pd-side-item-active" : "")}
                    title={label}
                  >
                    <Icon size={13} className="shrink-0" />
                    <span className="truncate">{label}</span>
                  </button>
                ))}
              </div>
            </div>
          ))}
        </aside>

        {/* 内容区 */}
        <div className="flex-1 overflow-y-auto">
          <div className="p-4 max-w-2xl mx-auto w-full">
            {activeTab === 'general' && <GeneralSettings />}
            {activeTab === 'environment' && <EnvManager />}
            {activeTab === 'agents' && <AgentManager />}
            {activeTab === 'api' && <ApiConfig />}
            {activeTab === 'mode' && <ModePromptSettings />}
            {activeTab === 'permission' && <PermissionSettings />}
            {activeTab === 'mcp' && <McpSettings />}
            {activeTab === 'search' && <SearchSettings />}
            {activeTab === 'tools' && <ToolSettings />}
            {activeTab === 'customtabs' && <CustomTabsSettings />}
            {activeTab === 'memory' && (
              <div className="space-y-4">
                {/* 记忆管理页内子 tab（样式与工具管理页场景切换一致） */}
                <div className="flex items-center gap-1">
                  {(['project', 'user', 'kv'] as const).map((t) => (
                    <button
                      key={t}
                      onClick={() => setMemTab(t)}
                      className={'pd-tab' + (memTab === t ? ' pd-tab-active' : '')}
                    >
                      {t === 'project' ? '项目记忆' : t === 'user' ? '用户偏好' : '全局 KV 记忆'}
                    </button>
                  ))}
                </div>
                {memTab === 'project' ? (
                  <ProjectMemorySettings />
                ) : memTab === 'user' ? (
                  <UserMemorySettings />
                ) : (
                  <KvMemorySettings />
                )}
              </div>
            )}
            {activeTab === 'filehistory' && <FileHistorySettings />}
            {activeTab === 'about' && <AboutSection />}
          </div>
        </div>
      </div>

      {/* StatusBar */}
      <StatusBar onOpenSettings={onBack} />
    </div>
  );
}
