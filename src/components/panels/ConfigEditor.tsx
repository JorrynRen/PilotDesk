import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useConfigStore, type ClaudeConfigPublic, type HermesConfigPublic } from '../../stores/configStore';
import { useSessionStore } from '../../stores/sessionStore';
import { sendApiRequest } from '../../utils/apiClient';
import { AGENT_THEMES } from '../../types';

interface ConfigEditorProps {
  agent: string;
}

type AgentType = 'claude' | 'hermes';

interface AgentFormConfig {
  type: AgentType;
  themeVar: string;
  defaultEndpoint: string;
  defaultModel: string;
  providerId: string;
  fields: Array<{
    key: string;
    label: string;
    type: 'text' | 'password' | 'number' | 'textarea';
    placeholder: string;
    hint?: string;
    min?: number;
    max?: number;
    step?: number;
    rows?: number;
  }>;
}

const AGENT_FORM_CONFIGS: Record<AgentType, AgentFormConfig> = {
  claude: {
    type: 'claude',
    themeVar: AGENT_THEMES.claude.cssVar,
    defaultEndpoint: 'https://api.anthropic.com',
    defaultModel: 'claude-sonnet-4-20250514',
    providerId: 'anthropic',
    fields: [
      { key: 'model', label: '模型', type: 'text', placeholder: 'claude-sonnet-4-20250514' },
      { key: 'apiEndpoint', label: 'API 端点', type: 'text', placeholder: 'https://api.anthropic.com' },
      { key: 'apiKey', label: 'API Key', type: 'password', placeholder: '未设置', hint: '留空保持不变，输入新值将覆盖现有 Key' },
      { key: 'customInstructions', label: '自定义指令', type: 'textarea', placeholder: 'Claude Code 自定义系统指令...', rows: 3 },
      { key: 'maxTokens', label: '最大 Tokens', type: 'number', placeholder: '8192' },
    ],
  },
  hermes: {
    type: 'hermes',
    themeVar: AGENT_THEMES.hermes.cssVar,
    defaultEndpoint: 'https://api.siliconflow.cn/v1',
    defaultModel: 'deepseek-ai/DeepSeek-V3',
    providerId: 'openai',
    fields: [
      { key: 'model', label: '模型', type: 'text', placeholder: 'hermes-default' },
      { key: 'apiEndpoint', label: 'API 端点', type: 'text', placeholder: 'https://api.example.com/v1' },
      { key: 'apiKey', label: 'API Key', type: 'password', placeholder: '未设置', hint: '留空保持不变，输入新值将覆盖现有 Key' },
      { key: 'temperature', label: 'Temperature', type: 'number', placeholder: '0.7', min: 0, max: 1, step: 0.1 },
      { key: 'systemPrompt', label: '系统提示词', type: 'textarea', placeholder: 'Hermes Agent 系统提示词...', rows: 3 },
      { key: 'maxTokens', label: '最大 Tokens', type: 'number', placeholder: '8192' },
    ],
  },
};

function AgentConfigForm({ config, agentType }: { config: ClaudeConfigPublic | HermesConfigPublic; agentType: AgentType }) {
  const cfg = AGENT_FORM_CONFIGS[agentType];
  const saveFn = agentType === 'claude'
    ? useConfigStore((s) => s.saveClaudeConfig)
    : useConfigStore((s) => s.saveHermesConfig);
  const saving = useConfigStore((s) => s.saving);

  const [testing, setTesting] = useState(false);
  const [testMsg, setTestMsg] = useState('');
  const [testOk, setTestOk] = useState(false);

  // Form state — initialize from config
  const [model, setModel] = useState((config as any).model ?? '');
  const [apiEndpoint, setApiEndpoint] = useState((config as any).apiEndpoint ?? '');
  const [apiKey, setApiKey] = useState('');
  const [customInstructions, setCustomInstructions] = useState((config as any).customInstructions ?? '');
  const [temperature, setTemperature] = useState((config as any).temperature?.toString() ?? '');
  const [systemPrompt, setSystemPrompt] = useState((config as any).systemPrompt ?? '');
  const [maxTokens, setMaxTokens] = useState((config as any).maxTokens?.toString() ?? '');
  const [modified, setModified] = useState(false);

  // Sync form fields when config prop changes
  useEffect(() => {
    setModel((config as any).model ?? '');
    setApiEndpoint((config as any).apiEndpoint ?? '');
    setCustomInstructions((config as any).customInstructions ?? '');
    setTemperature((config as any).temperature?.toString() ?? '');
    setMaxTokens((config as any).maxTokens?.toString() ?? '');
    setSystemPrompt((config as any).systemPrompt ?? '');
  }, [(config as any).model, (config as any).apiEndpoint, (config as any).customInstructions, (config as any).temperature, (config as any).maxTokens, (config as any).systemPrompt]);

  const hasChanges = useCallback(() => {
    const c = config as any;
    return (
      (model !== (c.model ?? '')) ||
      (apiEndpoint !== (c.apiEndpoint ?? '')) ||
      (apiKey !== '') ||
      (customInstructions !== (c.customInstructions ?? '')) ||
      (temperature !== (c.temperature?.toString() ?? '')) ||
      (maxTokens !== (c.maxTokens?.toString() ?? '')) ||
      (systemPrompt !== (c.systemPrompt ?? ''))
    );
  }, [model, apiEndpoint, apiKey, customInstructions, temperature, maxTokens, systemPrompt, config]);

  useEffect(() => {
    setModified(hasChanges());
  }, [hasChanges]);

  const handleSave = async () => {
    const c = config as any;
    const update: Record<string, unknown> = {};
    if (model !== (c.model ?? '')) update.model = model || null;
    if (apiEndpoint !== (c.apiEndpoint ?? '')) update.apiEndpoint = apiEndpoint || null;
    if (apiKey !== '') update.apiKey = apiKey;
    if (customInstructions !== (c.customInstructions ?? '')) update.customInstructions = customInstructions || null;
    if (temperature !== (c.temperature?.toString() ?? '')) update.temperature = temperature ? parseFloat(temperature) : null;
    if (maxTokens !== (c.maxTokens?.toString() ?? '')) update.maxTokens = maxTokens ? parseInt(maxTokens) : null;
    if (systemPrompt !== (c.systemPrompt ?? '')) update.systemPrompt = systemPrompt || null;
    await (saveFn as any)(update);
    setApiKey('');
    setModified(false);
  };

  const handleTestConnection = async () => {
    const key = apiKey || (await invoke<string | null>('get_agent_api_key', { agentType }).catch(() => null));
    if (!key) {
      setTestMsg('未配置 API Key，请先在表单中填写 API Key 并保存');
      setTestOk(false);
      return;
    }
    const ep = apiEndpoint || cfg.defaultEndpoint;
    const mdl = model || cfg.defaultModel;
    setTesting(true);
    setTestMsg('');
    const result = await sendApiRequest({
      endpoint: ep,
      providerId: cfg.providerId,
      apiKey: key,
      model: mdl,
      messages: [{ role: 'user', content: 'hi' }],
      maxTokens: 1,
      timeout: 15000,
    });
    setTesting(false);
    setTestOk(result.ok);
    setTestMsg(result.ok
      ? `连接成功 - 模型: ${mdl} (${result.latency}ms)`
      : result.message);
  };

  const renderField = (field: AgentFormConfig['fields'][0]) => {
    const getValue = () => {
      switch (field.key) {
        case 'model': return model;
        case 'apiEndpoint': return apiEndpoint;
        case 'apiKey': return apiKey;
        case 'customInstructions': return customInstructions;
        case 'temperature': return temperature;
        case 'systemPrompt': return systemPrompt;
        case 'maxTokens': return maxTokens;
        default: return '';
      }
    };

    const setValue = (v: string) => {
      switch (field.key) {
        case 'model': setModel(v); break;
        case 'apiEndpoint': setApiEndpoint(v); break;
        case 'apiKey': setApiKey(v); break;
        case 'customInstructions': setCustomInstructions(v); break;
        case 'temperature': setTemperature(v); break;
        case 'systemPrompt': setSystemPrompt(v); break;
        case 'maxTokens': setMaxTokens(v); break;
      }
    };

    const inputStyle = {
      backgroundColor: 'var(--bg-tertiary)',
      color: 'var(--text-primary)',
      border: '1px solid var(--border)',
    };

    if (field.type === 'textarea') {
      return (
        <textarea
          value={getValue()}
          onChange={(e) => setValue(e.target.value)}
          placeholder={field.placeholder}
          rows={field.rows ?? 3}
          className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none"
          style={inputStyle}
        />
      );
    }

    if (field.type === 'password') {
      return (
        <div>
          <div className="flex items-center gap-2">
            <input
              type="password"
              value={getValue()}
              onChange={(e) => setValue(e.target.value)}
              placeholder={(config as any).apiKeyMasked ?? field.placeholder}
              className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
              style={inputStyle}
            />
          </div>
          {field.hint && (
            <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>{field.hint}</p>
          )}
        </div>
      );
    }

    return (
      <input
        type={field.type}
        value={getValue()}
        onChange={(e) => setValue(e.target.value)}
        placeholder={field.placeholder}
        min={field.min}
        max={field.max}
        step={field.step}
        className="w-full px-3 py-2 rounded-lg text-sm outline-none"
        style={inputStyle}
      />
    );
  };

  return (
    <div className="space-y-4">
      {cfg.fields.map((field) => (
        <div key={field.key}>
          <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
            {field.label}
          </label>
          {renderField(field)}
        </div>
      ))}

      <div className="flex items-center gap-2 pt-2">
        <button
          onClick={handleSave}
          disabled={!modified || saving}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors whitespace-nowrap disabled:opacity-50"
          style={{
            backgroundColor: modified ? cfg.themeVar : 'var(--bg-tertiary)',
            color: modified ? '#fff' : 'var(--text-secondary)',
          }}
        >
          {saving ? '保存中...' : '保存配置'}
        </button>
        <button
          onClick={handleTestConnection}
          disabled={testing}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors whitespace-nowrap disabled:opacity-50"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        >
          {testing ? '测试中...' : '测试连接'}
        </button>
      </div>
      {testMsg && (
        <div
          className="mt-2 px-3 py-2 rounded-lg text-xs leading-relaxed"
          style={{
            backgroundColor: testOk ? 'rgba(52, 211, 153, 0.08)' : 'rgba(239, 68, 68, 0.08)',
            color: testOk ? 'var(--success)' : 'var(--danger)',
            border: '1px solid',
            borderColor: testOk ? 'rgba(52, 211, 153, 0.2)' : 'rgba(239, 68, 68, 0.2)',
          }}
        >
          {testMsg}
        </div>
      )}
    </div>
  );
}

export function ConfigEditor({ agent }: ConfigEditorProps) {
  const { config, loading, error, fetchConfig, clearError } = useConfigStore();
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });
  // Resolve agent: use prop if provided, otherwise use current session
  const activeAgent = (agent || currentSession?.agentType || 'claude') as AgentType;

  useEffect(() => {
    fetchConfig();
    return () => clearError();
  }, [activeAgent, fetchConfig, clearError]);

if (loading && !config) {
    return (
      <div className="flex items-center justify-center h-full">
        <span className="text-sm" style={{ color: 'var(--text-secondary)' }}>加载配置中...</span>
      </div>
    );
  }

  const agentConfig = activeAgent === 'claude' ? config?.claude : config?.hermes;
  const agentLabel = activeAgent === 'claude' ? 'Claude Code' : 'Hermes Agent';

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          {agentLabel} 配置
        </h3>
        <button
          onClick={fetchConfig}
          className="text-xs px-2 py-1 rounded"
          style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
        >
          刷新
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {error && (
          <div className="mb-4 px-3 py-2 rounded-lg text-xs" style={{ backgroundColor: 'var(--danger-bg)', color: 'var(--danger)' }}>
            {error}
          </div>
        )}

        {agentConfig ? (
          <AgentConfigForm config={agentConfig} agentType={activeAgent} />
        ) : (
          <div className="text-center py-8">
            <p className="text-sm mb-2" style={{ color: 'var(--text-secondary)' }}>
              {agentLabel} 未安装或配置目录不存在
            </p>
            <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
              请先在环境管理中安装 {agentLabel}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
