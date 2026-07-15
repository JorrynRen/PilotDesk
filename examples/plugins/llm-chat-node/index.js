/**
 * LLM Chat Node — PilotDesk 工作流插件
 *
 * 导出：
 * - PluginNodeConfig:  工作流 plugin 节点中渲染的配置组件（props: params, onParamsChange, api）
 * - LLMChatPanel:      独立测试面板组件
 * - onLoad / onUnload: 插件生命周期
 */

// ═══════════════════════════════════════════════════════════
// API 调用工具函数
// ═══════════════════════════════════════════════════════════

function inferApiFormat(endpoint) {
  if (/anthropic\.com/.test(endpoint)) return 'anthropic';
  return 'openai';
}

function resolveChatUrl(endpoint, fmt) {
  if (fmt === 'anthropic') return endpoint.replace(/\/+$/, '') + '/v1/messages';
  return endpoint.replace(/\/+$/, '') + '/v1/chat/completions';
}

function buildHeaders(fmt, apiKey) {
  var headers = { 'Content-Type': 'application/json' };
  if (fmt === 'anthropic') {
    headers['x-api-key'] = apiKey;
    headers['anthropic-version'] = '2023-06-01';
  } else {
    headers['Authorization'] = 'Bearer ' + apiKey;
  }
  return headers;
}

function buildBody(fmt, opts) {
  if (fmt === 'anthropic') {
    var body = { model: opts.model, max_tokens: opts.maxTokens || 4096, stream: false, messages: opts.messages };
    if (opts.systemPrompt) body.system = opts.systemPrompt;
    return body;
  }
  var body = { model: opts.model, max_tokens: opts.maxTokens || 4096, temperature: opts.temperature ?? 0.7, stream: false, messages: opts.messages };
  return body;
}

async function chatWithLLM(api, params) {
  var providerId = params.api_provider_id;
  var model = params.model;
  var prompt = params.prompt;
  var systemPrompt = params.system_prompt || '';

  var provider = await api.data.invoke('get_api_provider', { id: providerId });
  if (!provider || !provider.apiEndpoint) throw new Error('未找到 API 供应商: ' + providerId);

  var apiKey = await api.data.invoke('get_api_key', { id: providerId });
  if (!apiKey) throw new Error('请先在「设置 - API 配置」中为「' + (provider.name || providerId) + '」添加 API Key');

  var fmt = inferApiFormat(provider.apiEndpoint);
  var chatUrl = resolveChatUrl(provider.apiEndpoint, fmt);
  var messages = [];
  if (systemPrompt && fmt !== 'anthropic') messages.push({ role: 'system', content: systemPrompt });
  messages.push({ role: 'user', content: prompt });

  var body = buildBody(fmt, {
    model: model, maxTokens: params.max_tokens || 4096, temperature: params.temperature,
    systemPrompt: fmt === 'anthropic' ? systemPrompt : undefined, messages: messages
  });

  var res = await fetch(chatUrl, { method: 'POST', headers: buildHeaders(fmt, apiKey), body: JSON.stringify(body) });
  if (!res.ok) { var errText = await res.text(); throw new Error('API 错误 (' + res.status + '): ' + errText); }

  var data = await res.json();
  var content = fmt === 'anthropic'
    ? (data.content || []).map(function (b) { return b.text || ''; }).join('')
    : ((data.choices || [{}])[0].message || {}).content || '';

  return { content: content, model: model, provider: provider.name || providerId };
}

// ═══════════════════════════════════════════════════════════
// PluginNodeConfig — 工作流 plugin 节点的配置组件
// ═══════════════════════════════════════════════════════════

var PluginNodeConfig = function (props) {
  var params = props.params || {};
  var onParamsChange = props.onParamsChange || function () {};
  var api = props.api;

  var stateCommandId = React.useState(params.command_id || '');
  var stateProviderId = React.useState(params.api_provider_id || '');
  var stateModel = React.useState(params.model || '');
  var statePrompt = React.useState(params.prompt || '');
  var stateSystemPrompt = React.useState(params.system_prompt || '');
  var stateMaxTokens = React.useState(params.max_tokens ?? 4096);
  var stateTemperature = React.useState(params.temperature ?? 0.7);
  var stateProviders = React.useState([]);
  var stateModels = React.useState([]);
  var stateCommands = React.useState([]);
  var stateLoading = React.useState(false);
  var stateError = React.useState('');
  var stateTestResult = React.useState('');

  var commandId = stateCommandId[0]; var setCommandId = stateCommandId[1];
  var providerId = stateProviderId[0]; var setProviderId = stateProviderId[1];
  var model = stateModel[0]; var setModel = stateModel[1];
  var prompt = statePrompt[0]; var setPrompt = statePrompt[1];
  var systemPrompt = stateSystemPrompt[0]; var setSystemPrompt = stateSystemPrompt[1];
  var maxTokens = stateMaxTokens[0]; var setMaxTokens = stateMaxTokens[1];
  var temperature = stateTemperature[0]; var setTemperature = stateTemperature[1];
  var providers = stateProviders[0]; var setProviders = stateProviders[1];
  var models = stateModels[0]; var setModels = stateModels[1];
  var commands = stateCommands[0]; var setCommands = stateCommands[1];
  var loading = stateLoading[0]; var setLoading = stateLoading[1];
  var error = stateError[0]; var setError = stateError[1];
  var testResult = stateTestResult[0]; var setTestResult = stateTestResult[1];

  // 初始化：自动设置 command_id（本插件仅有一个命令）
  React.useEffect(function () {
    if (!params.command_id) {
      onParamsChange('command_id', 'llm-chat.chat');
    }
  }, []);

  // 加载 API 供应商
  React.useEffect(function () {
    if (!api) return;
    api.data.invoke('list_api_providers').then(function (list) {
      setProviders(list || []);
    }).catch(function (err) {
      console.error('[PluginNodeConfig] 获取供应商失败:', err);
    });
  }, []);

  // 供应商变更时更新模型列表
  React.useEffect(function () {
    if (providers.length === 0) return; // 供应商列表尚未加载完成，跳过以避免误清空已恢复的 model
    var p = providers.find(function (item) { return item.id === providerId; });
    setModels((p && p.models) ? p.models : []);
    if (!p) { setModel(''); onParamsChange('model', ''); }
  }, [providerId, providers.length]);

  var handleChange = function (key, value) {
    onParamsChange(key, value);
    return value;
  };

  // 样式
  var S = {
    label: { fontSize: 12, fontWeight: 600, color: 'var(--text-tertiary)', marginBottom: 4, display: 'block' },
    select: { width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none' },
    input: { width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none' },
    textarea: { width: '100%', padding: '8px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none', minHeight: 60, resize: 'vertical' },
    button: { padding: '6px 14px', borderRadius: 6, border: '1px solid var(--accent)', background: 'var(--accent)', color: '#fff', fontSize: 12, fontWeight: 600, cursor: 'pointer' },
    gap: { display: 'flex', flexDirection: 'column', gap: 10 },
    row: { display: 'flex', gap: 8 },
    field: { flex: 1, display: 'flex', flexDirection: 'column' },
    result: { padding: 10, borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-secondary)', fontSize: 12, lineHeight: 1.5, whiteSpace: 'pre-wrap', maxHeight: 150, overflow: 'auto' },
    errorText: { fontSize: 11, color: '#f85149', marginTop: 4 }
  };

  return React.createElement('div', { style: S.gap },


    // API 供应商
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, 'API 供应商'),
      React.createElement('select', {
        value: providerId,
        onChange: function (e) { var v = e.target.value; setProviderId(v); onParamsChange('api_provider_id', v); },
        style: S.select
      },
        React.createElement('option', { value: '' }, '-- 选择供应商 --'),
        providers.map(function (p) { return React.createElement('option', { key: p.id, value: p.id }, p.name + (p.apiKeySet ? '' : ' (未配置 Key)')); })
      ),
      providers.length === 0 ? React.createElement('span', { style: { fontSize: 11, color: 'var(--text-tertiary)' } }, '暂无供应商，请在「设置 - API 配置」中添加') : null
    ),

    // 模型名称
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '模型名称'),
      models.length > 0
        ? React.createElement('select', {
            value: model,
            onChange: function (e) { var v = e.target.value; setModel(v); onParamsChange('model', v); },
            style: S.select
          },
            React.createElement('option', { value: '' }, '-- 选择模型 --'),
            models.map(function (m) { return React.createElement('option', { key: m, value: m }, m); })
          )
        : React.createElement('input', { type: 'text', value: model, onChange: function (e) { var v = e.target.value; setModel(v); onParamsChange('model', v); }, placeholder: providerId ? '输入模型名称' : '请先选择供应商', disabled: !providerId, style: S.input })
    ),

    // 提示词
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '提示词'),
      React.createElement('textarea', {
        value: prompt,
        onChange: function (e) { var v = e.target.value; setPrompt(v); onParamsChange('prompt', v); },
        placeholder: '输入提示词...',
        style: Object.assign({}, S.textarea, { minHeight: 60 }),
        rows: 3
      })
    ),

    // 系统提示词
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '系统提示词（可选）'),
      React.createElement('textarea', {
        value: systemPrompt,
        onChange: function (e) { var v = e.target.value; setSystemPrompt(v); onParamsChange('system_prompt', v); },
        placeholder: '设定 AI 角色，如：你是一个专业的翻译助手...',
        style: Object.assign({}, S.textarea, { minHeight: 46 }),
        rows: 2
      })
    ),

    // 高级参数
    React.createElement('div', { style: S.row },
      React.createElement('div', { style: S.field },
        React.createElement('label', { style: S.label }, '最大 Token'),
        React.createElement('input', { type: 'number', value: maxTokens, onChange: function (e) { var v = Number(e.target.value); setMaxTokens(v); onParamsChange('max_tokens', v); }, style: S.input })
      ),
      React.createElement('div', { style: S.field },
        React.createElement('label', { style: S.label }, '温度'),
        React.createElement('input', { type: 'number', value: temperature, step: 0.1, min: 0, max: 2, onChange: function (e) { var v = Number(e.target.value); setTemperature(v); onParamsChange('temperature', v); }, style: S.input })
      )
    ),

    // 错误提示
    error ? React.createElement('div', { style: S.errorText }, error) : null,

    // 测试结果
    testResult ? React.createElement('div', null,
      React.createElement('label', { style: S.label }, '测试结果'),
      React.createElement('div', { style: S.result }, testResult)
    ) : null
  );
};

// ═══════════════════════════════════════════════════════════
// LLMChatPanel — 独立预览面板
// ═══════════════════════════════════════════════════════════

var LLMChatPanel = function () {
  var stateProviderId = React.useState('');
  var stateModel = React.useState('');
  var statePrompt = React.useState('');
  var stateSystemPrompt = React.useState('');
  var stateResponse = React.useState('');
  var stateLoading = React.useState(false);
  var stateError = React.useState('');
  var stateProviders = React.useState([]);
  var stateModels = React.useState([]);

  var providerId = stateProviderId[0]; var setProviderId = stateProviderId[1];
  var model = stateModel[0]; var setModel = stateModel[1];
  var prompt = statePrompt[0]; var setPrompt = statePrompt[1];
  var systemPrompt = stateSystemPrompt[0]; var setSystemPrompt = stateSystemPrompt[1];
  var response = stateResponse[0]; var setResponse = stateResponse[1];
  var loading = stateLoading[0]; var setLoading = stateLoading[1];
  var error = stateError[0]; var setError = stateError[1];
  var providers = stateProviders[0]; var setProviders = stateProviders[1];
  var models = stateModels[0]; var setModels = stateModels[1];

  var api = window.__llmChatNodeAPI;

  React.useEffect(function () {
    if (!api) return;
    api.data.invoke('list_api_providers').then(function (list) { setProviders(list || []); }).catch(function () {});
  }, []);

  React.useEffect(function () {
    var p = providers.find(function (item) { return item.id === providerId; });
    setModels((p && p.models) ? p.models : []);
    if (!p) { setModel(''); }
    else if (p.models && p.models.length > 0 && !p.models.includes(model)) setModel(p.models[0]);
  }, [providerId, providers]);

  var handleSubmit = function () {
    if (!providerId) { setError('请选择 API 供应商'); return; }
    if (!model) { setError('请选择或输入模型名称'); return; }
    if (!prompt.trim()) { setError('请输入提示词'); return; }
    setError(''); setResponse(''); setLoading(true);
    chatWithLLM(api, { api_provider_id: providerId, model: model, prompt: prompt, system_prompt: systemPrompt })
      .then(function (r) { setLoading(false); setResponse(r.content || '(无回复内容)'); })
      .catch(function (e) { setLoading(false); setError(String(e)); });
  };

  var S = {
    container: { padding: 16, display: 'flex', flexDirection: 'column', gap: 12 },
    title: { fontSize: 15, fontWeight: 600, margin: '0 0 8 0' },
    label: { fontSize: 12, fontWeight: 600, color: 'var(--text-tertiary)', marginBottom: 4, display: 'block' },
    select: { width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none' },
    input: { width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none' },
    textarea: { width: '100%', padding: '8px 10px', borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-primary)', color: 'var(--text-primary)', fontSize: 13, outline: 'none', minHeight: 80, resize: 'vertical' },
    button: { padding: '8px 16px', borderRadius: 6, border: 'none', background: 'var(--accent)', color: '#fff', fontSize: 13, fontWeight: 600, cursor: 'pointer' },
    response: { padding: 12, borderRadius: 6, border: '1px solid var(--border)', background: 'var(--bg-secondary)', fontSize: 13, lineHeight: 1.6, whiteSpace: 'pre-wrap' },
    error: { padding: 8, borderRadius: 6, border: '1px solid #f85149', background: '#f8514915', fontSize: 12, color: '#f85149' },
    hint: { fontSize: 11, color: 'var(--text-tertiary)', marginTop: 2 }
  };

  return React.createElement('div', { style: S.container },
    React.createElement('h3', { style: S.title }, 'LLM 对话'),
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, 'API 供应商'),
      React.createElement('select', { value: providerId, onChange: function (e) { setProviderId(e.target.value); }, style: S.select },
        React.createElement('option', { value: '' }, '-- 请选择供应商 --'),
        providers.map(function (p) { return React.createElement('option', { key: p.id, value: p.id }, p.name + (p.apiKeySet ? '' : ' (未配置 Key)')); })
      ),
      providers.length === 0 ? React.createElement('span', { style: S.hint }, '暂无供应商，请先在「设置 - API 配置」中添加') : null
    ),
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '模型名称'),
      models.length > 0
        ? React.createElement('select', { value: model, onChange: function (e) { setModel(e.target.value); }, style: S.select },
            React.createElement('option', { value: '' }, '-- 请选择模型 --'),
            models.map(function (m) { return React.createElement('option', { key: m, value: m }, m); })
          )
        : React.createElement('input', { type: 'text', value: model, onChange: function (e) { setModel(e.target.value); }, placeholder: providerId ? '输入模型名称' : '请先选择供应商', disabled: !providerId, style: S.input })
    ),
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '提示词'),
      React.createElement('textarea', { value: prompt, onChange: function (e) { setPrompt(e.target.value); }, placeholder: '输入你想问 LLM 的问题...', style: Object.assign({}, S.textarea, { minHeight: 60 }), rows: 3 })
    ),
    React.createElement('div', null,
      React.createElement('label', { style: S.label }, '系统提示词（可选）'),
      React.createElement('textarea', { value: systemPrompt, onChange: function (e) { setSystemPrompt(e.target.value); }, placeholder: '设定 AI 角色，如：你是一个专业的翻译助手...', style: Object.assign({}, S.textarea, { minHeight: 50 }), rows: 2 })
    ),
    React.createElement('button', { onClick: handleSubmit, disabled: loading || !providerId || !model, style: S.button }, loading ? '思考中...' : '发送'),
    error ? React.createElement('div', { style: S.error }, error) : null,
    response ? React.createElement('div', null, React.createElement('label', { style: S.label }, 'LLM 回复'), React.createElement('div', { style: S.response }, response)) : null
  );
};

// ═══════════════════════════════════════════════════════════
// 插件入口
// ═══════════════════════════════════════════════════════════

export default {
  PluginNodeConfig: PluginNodeConfig,

  onLoad: function (api) {
    console.log('[LLMChatNode] Plugin loaded');

    window.__llmChatNodeAPI = api;

    // 注册命令 handler
    api.commands.register('llm-chat.chat', async function (params) {
      try {
        var result = await chatWithLLM(api, {
          api_provider_id: params.api_provider_id,
          model: params.model,
          prompt: params.prompt,
          system_prompt: params.system_prompt || '',
          max_tokens: params.max_tokens || 4096,
          temperature: params.temperature ?? 0.7
        });
        return { success: true, data: result };
      } catch (err) {
        return { success: false, data: null, error: String(err) };
      }
    });

    console.log('[LLMChatNode] Plugin ready');
  },

  onUnload: function () {
    console.log('[LLMChatNode] Plugin unloaded');
    if (window.__llmChatNodeAPI) delete window.__llmChatNodeAPI;
  }
};
