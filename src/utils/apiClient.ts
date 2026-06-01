/**
 * Unified API client for LLM providers (OpenAI-compatible & Anthropic).
 * Used by: API connection test (SettingsPage), API chat (useWebSocket), Agent config (future).
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
      return { ok: false, status: res.status, message: `认证失败 (HTTP ${res.status})`, latency };
    }
    if (res.status === 404) {
      return { ok: false, status: res.status, message: `端点不存在 (HTTP 404)，请检查 API 地址`, latency };
    }
    if (!res.ok) {
      const errText = await res.text().catch(() => '');
      return { ok: false, status: res.status, message: `请求失败 (HTTP ${res.status}): ${errText.slice(0, 200)}`, latency };
    }

    const data = await res.json();

    // Check format-specific error fields
    if (fmt === 'anthropic' && data.type === 'error') {
      return { ok: false, status: res.status, message: `API 返回错误: ${data.error?.message || JSON.stringify(data.error)}`, latency };
    }
    if (fmt === 'openai' && data.error) {
      return { ok: false, status: res.status, message: `API 返回错误: ${data.error.message || JSON.stringify(data.error)}`, latency };
    }

    return { ok: true, status: res.status, data, message: '连接成功', latency };
  } catch (err) {
    const latency = Math.round(performance.now() - start);
    const msg = err instanceof DOMException && err.name === 'TimeoutError'
      ? '连接超时 (15s)，请检查网络或端点地址'
      : `网络错误: ${err instanceof Error ? err.message : String(err)}`;
    return { ok: false, status: 0, message: msg, latency };
  }
}
