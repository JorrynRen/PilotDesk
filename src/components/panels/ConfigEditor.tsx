import { useState, useEffect, useCallback } from 'react';
import { useConfigStore, type ClaudeConfigPublic, type HermesConfigPublic } from '../../stores/configStore';
import { useSessionStore } from '../../stores/sessionStore';

interface ConfigEditorProps {
  agent: string;
}

function ClaudeConfigForm({ config }: { config: ClaudeConfigPublic }) {
  const { saveClaudeConfig, saving, testConnection, testResult } = useConfigStore();
  const [model, setModel] = useState(config.model ?? '');
  const [apiKey, setApiKey] = useState('');
  const [customInstructions, setCustomInstructions] = useState(config.custom_instructions ?? '');
  const [maxTokens, setMaxTokens] = useState(config.max_tokens?.toString() ?? '');
  const [modified, setModified] = useState(false);

  const hasChanges = useCallback(() => {
    return (
      (model !== (config.model ?? '')) ||
      (apiKey !== '') ||
      (customInstructions !== (config.custom_instructions ?? '')) ||
      (maxTokens !== (config.max_tokens?.toString() ?? ''))
    );
  }, [model, apiKey, customInstructions, maxTokens, config]);

  useEffect(() => {
    setModified(hasChanges());
  }, [hasChanges]);

  const handleSave = async () => {
    const update: Record<string, unknown> = {};
    if (model !== (config.model ?? '')) update.model = model || null;
    if (apiKey !== '') update.api_key = apiKey;
    if (customInstructions !== (config.custom_instructions ?? '')) update.custom_instructions = customInstructions || null;
    if (maxTokens !== (config.max_tokens?.toString() ?? '')) update.max_tokens = maxTokens ? parseInt(maxTokens) : null;
    await saveClaudeConfig(update as Parameters<typeof saveClaudeConfig>[0]);
    setApiKey('');
    setModified(false);
  };

  return (
    <div className="space-y-4">
      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          模型
        </label>
        <input
          type="text"
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="claude-sonnet-4-20250514"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          API Key
        </label>
        <div className="flex items-center gap-2">
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={config.api_key_masked ?? '未设置'}
            className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
          {config.api_key_set && (
            <span className="text-xs px-2 py-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--claude-tag)' }}>
              {config.api_key_masked}
            </span>
          )}
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          留空保持不变，输入新值将覆盖现有 Key
        </p>
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          自定义指令
        </label>
        <textarea
          value={customInstructions}
          onChange={(e) => setCustomInstructions(e.target.value)}
          placeholder="Claude Code 自定义系统指令..."
          rows={3}
          className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          最大 Tokens
        </label>
        <input
          type="number"
          value={maxTokens}
          onChange={(e) => setMaxTokens(e.target.value)}
          placeholder="8192"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div className="flex items-center gap-2 pt-2">
        <button
          onClick={handleSave}
          disabled={!modified || saving}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
          style={{
            backgroundColor: modified ? 'var(--claude-tag)' : 'var(--bg-tertiary)',
            color: modified ? '#fff' : 'var(--text-secondary)',
          }}
        >
          {saving ? '保存中...' : '保存配置'}
        </button>
        <button
          onClick={() => testConnection('claude')}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        >
          测试连接
        </button>
        {testResult?.agent_type === 'claude' && (
          <span className="text-xs" style={{ color: testResult.success ? 'var(--success)' : 'var(--danger)' }}>
            {testResult.message}
          </span>
        )}
      </div>
    </div>
  );
}

function HermesConfigForm({ config }: { config: HermesConfigPublic }) {
  const { saveHermesConfig, saving, testConnection, testResult } = useConfigStore();
  const [model, setModel] = useState(config.model ?? '');
  const [apiEndpoint, setApiEndpoint] = useState(config.api_endpoint ?? '');
  const [apiKey, setApiKey] = useState('');
  const [temperature, setTemperature] = useState(config.temperature?.toString() ?? '');
  const [maxTokens, setMaxTokens] = useState(config.max_tokens?.toString() ?? '');
  const [systemPrompt, setSystemPrompt] = useState(config.system_prompt ?? '');
  const [modified, setModified] = useState(false);

  const hasChanges = useCallback(() => {
    return (
      (model !== (config.model ?? '')) ||
      (apiEndpoint !== (config.api_endpoint ?? '')) ||
      (apiKey !== '') ||
      (temperature !== (config.temperature?.toString() ?? '')) ||
      (maxTokens !== (config.max_tokens?.toString() ?? '')) ||
      (systemPrompt !== (config.system_prompt ?? ''))
    );
  }, [model, apiEndpoint, apiKey, temperature, maxTokens, systemPrompt, config]);

  useEffect(() => {
    setModified(hasChanges());
  }, [hasChanges]);

  const handleSave = async () => {
    const update: Record<string, unknown> = {};
    if (model !== (config.model ?? '')) update.model = model || null;
    if (apiEndpoint !== (config.api_endpoint ?? '')) update.api_endpoint = apiEndpoint || null;
    if (apiKey !== '') update.api_key = apiKey;
    if (temperature !== (config.temperature?.toString() ?? '')) update.temperature = temperature ? parseFloat(temperature) : null;
    if (maxTokens !== (config.max_tokens?.toString() ?? '')) update.max_tokens = maxTokens ? parseInt(maxTokens) : null;
    if (systemPrompt !== (config.system_prompt ?? '')) update.system_prompt = systemPrompt || null;
    await saveHermesConfig(update as Parameters<typeof saveHermesConfig>[0]);
    setApiKey('');
    setModified(false);
  };

  return (
    <div className="space-y-4">
      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          模型
        </label>
        <input
          type="text"
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="hermes-default"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          API 端点
        </label>
        <input
          type="text"
          value={apiEndpoint}
          onChange={(e) => setApiEndpoint(e.target.value)}
          placeholder="https://api.example.com/v1"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          API Key
        </label>
        <div className="flex items-center gap-2">
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={config.api_key_masked ?? '未设置'}
            className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
          {config.api_key_set && (
            <span className="text-xs px-2 py-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--hermes-tag)' }}>
              {config.api_key_masked}
            </span>
          )}
        </div>
        <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
          留空保持不变，输入新值将覆盖现有 Key
        </p>
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          Temperature
        </label>
        <input
          type="number"
          min="0"
          max="1"
          step="0.1"
          value={temperature}
          onChange={(e) => setTemperature(e.target.value)}
          placeholder="0.7"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          系统提示词
        </label>
        <textarea
          value={systemPrompt}
          onChange={(e) => setSystemPrompt(e.target.value)}
          placeholder="Hermes Agent 系统提示词..."
          rows={3}
          className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div>
        <label className="block text-xs font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>
          最大 Tokens
        </label>
        <input
          type="number"
          value={maxTokens}
          onChange={(e) => setMaxTokens(e.target.value)}
          placeholder="8192"
          className="w-full px-3 py-2 rounded-lg text-sm outline-none"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
      </div>

      <div className="flex items-center gap-2 pt-2">
        <button
          onClick={handleSave}
          disabled={!modified || saving}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
          style={{
            backgroundColor: modified ? 'var(--hermes-tag)' : 'var(--bg-tertiary)',
            color: modified ? '#fff' : 'var(--text-secondary)',
          }}
        >
          {saving ? '保存中...' : '保存配置'}
        </button>
        <button
          onClick={() => testConnection('hermes')}
          className="px-4 py-2 rounded-lg text-sm font-medium transition-colors"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        >
          测试连接
        </button>
        {testResult?.agent_type === 'hermes' && (
          <span className="text-xs" style={{ color: testResult.success ? 'var(--success)' : 'var(--danger)' }}>
            {testResult.message}
          </span>
        )}
      </div>
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
  const activeAgent = agent || currentSession?.agentType || 'claude';

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

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          {activeAgent === 'claude' ? 'Claude Code 配置' : 'Hermes Agent 配置'}
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

        {activeAgent === 'claude' ? (
          config?.claude ? (
            <ClaudeConfigForm config={config.claude} />
          ) : (
            <div className="text-center py-8">
              <p className="text-sm mb-2" style={{ color: 'var(--text-secondary)' }}>
                Claude Code 未安装或配置目录不存在
              </p>
              <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                请先在环境管理中安装 Claude Code
              </p>
            </div>
          )
        ) : (
          config?.hermes ? (
            <HermesConfigForm config={config.hermes} />
          ) : (
            <div className="text-center py-8">
              <p className="text-sm mb-2" style={{ color: 'var(--text-secondary)' }}>
                Hermes Agent 未安装或配置目录不存在
              </p>
              <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                请先在环境管理中安装 Hermes Agent
              </p>
            </div>
          )
        )}
      </div>
    </div>
  );
}
