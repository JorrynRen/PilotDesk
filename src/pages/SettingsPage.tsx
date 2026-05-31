import { useState } from 'react';
import {
  ArrowLeft, Settings, Globe, Cpu, Key, Info,
  Sun, Moon, Monitor, FolderOpen, ChevronDown,
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
// 4. API Configuration
// ============================================================
interface ApiProvider {
  id: string;
  name: string;
  api_endpoint: string;
  api_key_masked: string | null;
  api_key_set: boolean;
  models: string[];
}

function ApiConfig() {
  const [providers, setProviders] = useState<ApiProvider[]>([
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
  ]);
  const [selectedProvider, setSelectedProvider] = useState('anthropic');
  const [editingKey, setEditingKey] = useState('');
  const [editingEndpoint, setEditingEndpoint] = useState('');

  const activeProvider = providers.find((p) => p.id === selectedProvider)!;

  const handleSaveEndpoint = () => {
    if (editingEndpoint.trim()) {
      setProviders((prev) =>
        prev.map((p) =>
          p.id === selectedProvider
            ? { ...p, api_endpoint: editingEndpoint.trim() }
            : p
        )
      );
      setEditingEndpoint('');
    }
  };

  const handleSaveKey = () => {
    if (editingKey.trim()) {
      const masked = editingKey.slice(0, 4) + '****' + editingKey.slice(-4);
      localStorage.setItem(`pd-api-${selectedProvider}-key`, editingKey.trim());
      localStorage.setItem(`pd-api-${selectedProvider}-masked`, masked);
      setProviders((prev) =>
        prev.map((p) =>
          p.id === selectedProvider
            ? { ...p, api_key_masked: masked, api_key_set: true }
            : p
        )
      );
      setEditingKey('');
    }
  };

  const handleClearKey = () => {
    localStorage.removeItem(`pd-api-${selectedProvider}-key`);
    localStorage.removeItem(`pd-api-${selectedProvider}-masked`);
    setProviders((prev) =>
      prev.map((p) =>
        p.id === selectedProvider
          ? { ...p, api_key_masked: null, api_key_set: false }
          : p
      )
    );
  };

  const handleAddProvider = () => {
    const name = prompt('API 提供商名称:');
    if (!name) return;
    const endpoint = prompt('API 端点:', 'https://');
    if (!endpoint) return;
    const id = 'custom_' + Date.now();
    setProviders((prev) => [
      ...prev,
      {
        id,
        name,
        api_endpoint: endpoint,
        api_key_masked: null,
        api_key_set: false,
        models: [],
      },
    ]);
    setSelectedProvider(id);
  };

  return (
    <div className="space-y-4">
      {/* Provider selector */}
      <div className="flex items-center gap-2">
        <select
          value={selectedProvider}
          onChange={(e) => setSelectedProvider(e.target.value)}
          className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
        </select>
        <button
          onClick={handleAddProvider}
          className="px-2 py-2 rounded-lg text-xs transition-colors"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
          }}
          title="添加自定义 API 提供商"
        >
          + 自定义
        </button>
      </div>

      {/* API Endpoint */}
      <section>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          API 端点
        </label>
        {editingEndpoint ? (
          <div className="flex items-center gap-2">
            <input
              type="text"
              value={editingEndpoint}
              onChange={(e) => setEditingEndpoint(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleSaveEndpoint()}
              className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--accent)',
              }}
              autoFocus
            />
            <button
              onClick={handleSaveEndpoint}
              className="px-3 py-2 rounded-lg text-xs font-medium"
              style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            >
              保存
            </button>
            <button
              onClick={() => setEditingEndpoint('')}
              className="px-3 py-2 rounded-lg text-xs"
              style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
            >
              取消
            </button>
          </div>
        ) : (
          <div
            className="flex items-center justify-between px-3 py-2 rounded-lg cursor-pointer group"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
            }}
            onClick={() => setEditingEndpoint(activeProvider.api_endpoint)}
          >
            <span className="text-sm" style={{ color: 'var(--text-primary)' }}>
              {activeProvider.api_endpoint}
            </span>
            <span className="text-[10px] opacity-0 group-hover:opacity-60 transition-opacity" style={{ color: 'var(--text-secondary)' }}>
              点击修改
            </span>
          </div>
        )}
      </section>

      {/* API Key */}
      <section>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          API Key
        </label>
        <div className="flex items-center gap-2">
          <input
            type="password"
            value={editingKey}
            onChange={(e) => setEditingKey(e.target.value)}
            placeholder={activeProvider.api_key_set ? '输入新 Key 覆盖' : '输入 API Key'}
            className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
          {activeProvider.api_key_set ? (
            <span className="text-xs px-2 py-1 rounded whitespace-nowrap" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)' }}>
              {activeProvider.api_key_masked}
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-2 mt-2">
          <button
            onClick={handleSaveKey}
            disabled={!editingKey.trim()}
            className="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors disabled:opacity-50"
            style={{
              backgroundColor: editingKey.trim() ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: editingKey.trim() ? '#fff' : 'var(--text-secondary)',
            }}
          >
            保存 Key
          </button>
          {activeProvider.api_key_set && (
            <button
              onClick={handleClearKey}
              className="px-3 py-1.5 rounded-lg text-xs transition-colors"
              style={{ color: 'var(--danger)', backgroundColor: 'transparent' }}
            >
              清除
            </button>
          )}
          <p className="text-xs ml-auto" style={{ color: 'var(--text-tertiary)' }}>
            Key 保存在本地，不会上传
          </p>
        </div>
      </section>

      {/* Available models */}
      {activeProvider.models.length > 0 && (
        <section>
          <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
            可用模型
          </label>
          <div className="flex flex-wrap gap-1">
            {activeProvider.models.map((model) => (
              <span
                key={model}
                className="px-2 py-1 rounded text-[10px]"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                  border: '1px solid var(--border)',
                }}
              >
                {model}
              </span>
            ))}
          </div>
        </section>
      )}

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        配置 API 后，可以在新建会话时选择直接使用 API 模型对话（无需 Agent）。
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

      {/* Links */}
      <section>
        <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
          相关链接
        </h3>
        <div className="space-y-2">
          {[
            { label: 'Claude Code', url: 'https://docs.anthropic.com/en/docs/claude-code' },
            { label: 'Tauri', url: 'https://tauri.app' },
          ].map(({ label, url }) => (
            <div
              key={label}
              className="flex items-center justify-between px-3 py-2 rounded-lg cursor-pointer transition-colors"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
              onClick={() => window.open(url, '_blank')}
            >
              <span className="text-xs" style={{ color: 'var(--text-primary)' }}>{label}</span>
              <span className="text-[10px]" style={{ color: 'var(--accent)' }}>{url}</span>
            </div>
          ))}
        </div>
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
          {activeTab === 'agent' && <ConfigEditor agent="" />}
          {activeTab === 'api' && <ApiConfig />}
          {activeTab === 'about' && <AboutSection />}
        </div>
      </div>
    </div>
  );
}
