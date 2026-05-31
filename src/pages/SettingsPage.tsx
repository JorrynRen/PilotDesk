import { useState } from 'react';
import {
  ArrowLeft, Settings, Globe, Cpu, Key, Info,
  Sun, Moon, Monitor, FolderOpen, ChevronDown,
  Plus, Trash2, Edit3, Check, X, Pencil,
} from 'lucide-react';
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

  const handlePickWorkspace = () => {
    const val = prompt('请输入工作区目录路径:', workspace);
    if (val) {
      setWorkspace(val);
      localStorage.setItem('pilotdesk-workspace', val);
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
            title="选择目录"
          >
            <FolderOpen size={12} />
            浏览
          </button>
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          Claude Code / Hermes Agent 会话的默认工作目录
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
    <div className="h-full flex flex-col overflow-hidden">
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
// 4. API Configuration (list-based with edit/delete)
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

  // Persist providers
  const persist = (list: ApiProvider[]) => {
    setProviders(list);
    localStorage.setItem('pd-api-providers', JSON.stringify(list));
  };

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

    // Save key if entered
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

  return (
    <div className="space-y-4">
      {/* Header with add button */}
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-xs font-semibold" style={{ color: 'var(--text-primary)' }}>
            API 提供商列表
          </h3>
          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            配置 API 后，可以在新建会话时选择直接使用 API 模型对话
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

      {/* Provider list */}
      <div className="space-y-2">
        {providers.length === 0 && (
          <div className="text-center py-8">
            <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              暂无 API 提供商，点击上方按钮添加
            </p>
          </div>
        )}

        {providers.map((p) => {
          const isEditing = editingProvider?.id === p.id;
          const ep = editingProvider || p;

          return (
            <div
              key={p.id}
              className="rounded-lg overflow-hidden"
              style={{
                border: isEditing ? '1px solid var(--accent)' : '1px solid var(--border)',
                backgroundColor: 'var(--bg-secondary)',
              }}
            >
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
                          setEditingProvider({ ...editingProvider!, name: e.target.value })
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
                  {isEditing ? (
                    <>
                      <button
                        onClick={handleSaveEdit}
                        className="p-1 rounded transition-colors"
                        style={{ color: 'var(--success)' }}
                        title="保存"
                      >
                        <Check size={13} />
                      </button>
                      <button
                        onClick={handleCancelEdit}
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
                        onClick={() => handleStartEdit(p)}
                        className="p-1 rounded transition-colors"
                        style={{ color: 'var(--text-secondary)' }}
                        title="编辑"
                      >
                        <Pencil size={13} />
                      </button>
                      <button
                        onClick={() => handleDeleteProvider(p.id)}
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
                        setEditingProvider({ ...editingProvider!, api_endpoint: e.target.value })
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
                        setEditingProvider({ ...editingProvider!, api_key: e.target.value })
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
                          setEditingProvider({ ...editingProvider!, models: e.target.value })
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
                                onClick={() => handleRemoveModel(p.id, model)}
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
                          value={newModelInput === p.id ? '' : newModelInput}
                          onChange={(e) => setNewModelInput(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleAddModelInline(p.id);
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
                          onClick={() => handleAddModelInline(p.id)}
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
          );
        })}
      </div>

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        API Key 保存在本地，不会上传。配置完成后可在新建会话时选择 API 直连模式。
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
        style={{ borderBottom: '1px solid var(--border)' }}
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
        className="shrink-0 px-4 flex gap-0.5 overflow-x-auto"
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
