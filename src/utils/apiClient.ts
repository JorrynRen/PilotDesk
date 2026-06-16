/**
 * Unified API client for LLM providers (OpenAI-compatible & Anthropic).
 * Used by: API connection test (SettingsPage), API chat (useAgentEvent), Agent config (future).
 *
 * All callers share the same:
 * - Endpoint resolution (auto-append /chat/completions or /v1/messages)
 * - Auth header construction
 * - Request body building
 */

export type ApiFormat = 'openai' | 'anthropic';

export interface ApiRequestOptions {
  /** Base API URL, e.g. https://api.deepseek.com */
  endpoint: string;
  /** Provider ID or name (used to infer format if endpoint is ambiguous) */
  providerId?: string;
  /** API key */
  apiKey: string;
  /** Model name */
  model: string;
  /** Message history (role/content pairs) */
  messages: Array<{ role: string; content: string }>;
  /** Whether to use streaming (SSE) — default false */
  stream?: boolean;
  /** Max tokens for non-stream requests — default 1 */
  maxTokens?: number;
  /** Request timeout in ms — default 15000 */
  timeout?: number;
  /** AbortSignal for cancellation */
  signal?: AbortSignal;
}

export interface ApiResponse {
  ok: boolean;
  status: number;
  /** Parsed response body (only when ok && !stream) */
  data?: Record<string, unknown>;
  /** Error message */
  message: string;
  /** Round-trip latency in ms */
  latency: number;
}

/**
 * Infer API format from provider ID or endpoint URL.
 */
export function inferApiFormat(providerId: string, endpoint: string): ApiFormat {
  if (providerId === 'anthropic' || endpoint.includes('anthropic.com')) {
    return 'anthropic';
  }
  return 'openai';
}

/**
 * Resolve a base URL to the full chat completions endpoint.
 * - OpenAI: append /chat/completions if not already present
 * - Anthropic: append /v1/messages if not already present
 */
export function resolveChatUrl(endpoint: string, fmt: ApiFormat): string {
  const ep = endpoint.replace(/\/+$/, '');
  if (fmt === 'anthropic') {
    return ep.endsWith('/messages') ? ep : `${ep}/v1/messages`;
  }
  return ep.endsWith('/chat/completions') ? ep : `${ep}/chat/completions`;
}

/**
 * Build request headers based on API format.
 */
export function buildHeaders(fmt: ApiFormat, apiKey: string): Record<string, string> {
  if (fmt === 'anthropic') {
    return {
      'Content-Type': 'application/json',
      'x-api-key': apiKey,
      'anthropic-version': '2023-06-01',
    };
  }
  return {
    'Content-Type': 'application/json',
    'Authorization': `Bearer ${apiKey}`,
  };
}

/**
 * Build request body based on API format.
 */
export function buildBody(
  fmt: ApiFormat,
  options: Pick<ApiRequestOptions, 'model' | 'messages' | 'stream' | 'maxTokens'>,
): Record<string, unknown> {
  const { model, messages, stream = false, maxTokens } = options;
  if (fmt === 'anthropic') {
    return {
      model,
      max_tokens: maxTokens ?? 4096,
      stream,
      messages: messages.map((m) => ({ role: m.role, content: m.content })),
    };
  }
  return {
    model,
    stream,
    ...(maxTokens != null ? { max_tokens: maxTokens } : {}),
    messages: messages.map((m) => ({ role: m.role, content: m.content })),
  };
}

/**
 * Map raw API error details to a user-friendly message with actionable guidance.
 * Covers common scenarios: auth failure, insufficient balance, model access, rate limits, etc.
 */
export function friendlyApiError(status: number, detail: string): string {
  const d = detail.toLowerCase();

  // Insufficient balance (common with 403)
  if (d.includes('insufficient') && (d.includes('balance') || d.includes('quota') || d.includes('credit'))) {
    return `账户余额不足 (HTTP ${status})。请前往 API 提供商后台充值后重试。`;
  }
  if (d.includes('balance') && (d.includes('not enough') || d.includes('insufficient'))) {
    return `账户余额不足 (HTTP ${status})。请前往 API 提供商后台充值后重试。`;
  }

  // Authentication / API key issues
  if (status === 401) {
    return `认证失败 (HTTP 401)：API Key 无效或已过期。请检查 API Key 是否正确，或重新生成后重试。`;
  }
  if (status === 403 && (d.includes('auth') || d.includes('permission') || d.includes('forbidden') || d.includes('access'))) {
    return `权限不足 (HTTP 403)：API Key 无权限访问此模型。请检查 API Key 的模型访问权限，或更换模型后重试。`;
  }
  if (status === 403 && (d.includes('not found') || d.includes('model') || d.includes('not support'))) {
    return `模型不可用 (HTTP 403)：当前模型不存在或暂不支持。请检查模型名称是否正确，或更换其他模型后重试。`;
  }
  // Generic 403 with detail
  if (status === 403 && detail) {
    return `请求被拒 (HTTP 403)：${detail.slice(0, 200)}。请检查 API Key 权限、账户余额或模型可用性。`;
  }

  // 404 - endpoint not found
  if (status === 404) {
    return `端点不存在 (HTTP 404)：请检查 API 地址（base_url）是否正确。示例：https://api.openai.com/v1`;
  }

  // Rate limiting
  if (status === 429) {
    return `请求过于频繁 (HTTP 429)：已触发速率限制，请稍后重试。`;
  }

  // Model not found in response body
  if (d.includes('model') && (d.includes('not found') || d.includes('does not exist') || d.includes('not support'))) {
    return `模型不可用：当前模型名称不存在或该账户无权使用。请检查模型名称是否正确，或更换模型后重试。`;
  }

  // Context length / token limit
  if (d.includes('context') || d.includes('token') && (d.includes('exceed') || d.includes('too long') || d.includes('maximum'))) {
    return `上下文长度超限：输入内容超出模型最大上下文限制，请精简输入后重试。`;
  }

  // Server errors
  if (status >= 500) {
    return `服务端错误 (HTTP ${status})：API 提供商服务异常，请稍后重试或联系提供商技术支持。`;
  }

  // Return original detail if no friendly mapping
  return detail;
}

/**
 * Send a non-streaming API request. Returns parsed response.
 * Suitable for connection tests.
 */
export async function sendApiRequest(options: ApiRequestOptions): Promise<ApiResponse> {
  const fmt = inferApiFormat(options.providerId ?? '', options.endpoint);
  const url = resolveChatUrl(options.endpoint, fmt);
  const headers = buildHeaders(fmt, options.apiKey);
  const body = buildBody(fmt, {
    model: options.model,
    messages: options.messages,
    stream: false,
    maxTokens: options.maxTokens ?? 1,
  });
  const timeout = options.timeout ?? 15000;

  const start = performance.now();
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
      signal: options.signal ?? AbortSignal.timeout(timeout),
    });

    const latency = Math.round(performance.now() - start);

    if (res.status === 401 || res.status === 403) {
      // Read response body for actual error details (e.g. insufficient balance, model access)
      let detail = '';
      try {
        const errBody = await res.text();
        if (errBody) {
          const parsed = JSON.parse(errBody);
          detail = parsed.error?.message || parsed.message || parsed.error || '';
        }
      } catch { /* ignore parse errors */ }
      const msg = friendlyApiError(res.status, detail);
      return { ok: false, status: res.status, message: msg, latency };
    }
    if (res.status === 404) {
      return { ok: false, status: res.status, message: friendlyApiError(404, ''), latency };
    }
    if (!res.ok) {
      const errText = await res.text().catch(() => '');
      const msg = friendlyApiError(res.status, errText.slice(0, 200));
      return { ok: false, status: res.status, message: msg, latency };
    }

    const data = await res.json();

    // Check format-specific error fields
    if (fmt === 'anthropic' && data.type === 'error') {
      const detail = data.error?.message || JSON.stringify(data.error);
      return { ok: false, status: res.status, message: friendlyApiError(res.status, detail), latency };
    }
    if (fmt === 'openai' && data.error) {
      const detail = data.error.message || JSON.stringify(data.error);
      return { ok: false, status: res.status, message: friendlyApiError(res.status, detail), latency };
    }

    return { ok: true, status: res.status, data, message: '连接成功', latency };
  } catch (err) {
    const latency = Math.round(performance.now() - start);
    const msg = err instanceof DOMException && err.name === 'TimeoutError'
      ? '连接超时 (15s)：API 端点无响应，请检查网络连接或 API 地址是否正确。'
      : `网络错误: ${err instanceof Error ? err.message : String(err)}`;
    return { ok: false, status: 0, message: msg, latency };
  }
}
