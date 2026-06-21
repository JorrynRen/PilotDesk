# PilotDesk Function Calling 与联网搜索 — 技术实现方案 v1.0

> **项目**: PilotDesk | **架构**: Tauri 2.0 + React 19 + TypeScript 6.0 + Rust + SQLite  
> **版本**: v1.1 | **日期**: 2026-06-02 | **状态**: 方案评审（搜索引擎选型已更新）  
> **预估工时**: 18-23h | **难度**: 中高

---

## 目录

1. [背景分析](#1-背景分析)
2. [方案选型](#2-方案选型)
3. [整体架构](#3-整体架构)
4. [详细技术设计](#4-详细技术设计)
5. [数据库改动](#5-数据库改动)
6. [设置页扩展](#6-设置页扩展)
7. [文件变更清单](#7-文件变更清单)
8. [实施路线图](#8-实施路线图)
9. [风险与应对](#9-风险与应对)
10. [设计决策记录](#10-设计决策记录)

---

## 1. 背景分析

### 1.1 现状

PilotDesk 当前通过 `apiClient.ts` 与大模型 API 直连，支持 OpenAI 兼容格式的对话接口。消息流经 `useWebSocket.ts` 中的 `sendApiChat` 函数，构建 HTTP 请求并通过 `ReadableStream` 实时接收 SSE 响应。

核心数据流：

```
用户输入 → handleSend() → sessionStore.addMessage(userMsg)
         → sendApiChat(messages, endpoint, model)
           → buildBody(fmt, { model, messages, stream: true })
           → fetch SSE stream → 解析 choices[0].delta.content → onChunk 回调
           → onDone 回调 → sessionStore.addMessage(assistantMsg)
```

现有文件职责：

| 文件                                            | 职责                                                                                      |
| --------------------------------------------- | --------------------------------------------------------------------------------------- |
| `src/utils/apiClient.ts`                      | 统一 API 调用：`inferApiFormat`、`resolveChatUrl`、`buildHeaders`、`buildBody`、`sendApiRequest` |
| `src/hooks/useWebSocket.ts`                   | WebSocket 管理 + `sendApiChat`（SSE 流式解析）                                                  |
| `src/stores/sessionStore.ts`                  | 会话/消息状态管理（Zustand），含去重保护                                                                |
| `src/components/message/MessageBubble.tsx`    | 消息气泡渲染，用户消息(U头像) / AI消息(卡片+Markdown)                                                    |
| `src/components/message/MarkdownRenderer.tsx` | Markdown 渲染，含表格美化                                                                       |
| `src-tauri/src/commands/session.rs`           | Rust 后端：会话/消息 CRUD（9个 Tauri 命令）                                                         |
| `src-tauri/src/db/init.rs`                    | SQLite 初始化 + Migration 逻辑                                                               |
| `src-tauri/src/db/models.rs`                  | Rust 数据模型（Session、Message、Inspiration 等）                                                |

### 1.2 功能缺失

当前架构存在两个核心能力缺失：

1. **深度推理不可见**：DeepSeek 等模型在 SSE 流中会返回 `reasoning_content` 字段（模型的思考过程），当前 `sendApiChat` 仅解析 `choices[0].delta.content`，推理内容被完全丢弃
2. **无法联网搜索**：请求体中未携带 `tools` 参数，模型无法触发工具调用；即使携带，前端也没有工具执行逻辑

### 1.3 目标

| 功能     | 描述                                       | 优先级 |
| ------ | ---------------------------------------- | --- |
| 推理可视化  | 解析 `reasoning_content` 字段，以可折叠面板展示模型思考过程 | P0  |
| 联网搜索   | 通过 Function Calling 协议让模型自主调用搜索工具获取实时信息  | P0  |
| 通用工具系统 | 可扩展的工具注册机制，为未来文件操作、代码执行等预留接口             | P1  |

---

## 2. 方案选型

### 2.1 联网搜索实现方案对比

| 维度              | 方案 A：标准 Function Calling       | 方案 B：预检索注入                  | 方案 C：Sidecar 代理               |
| --------------- | ------------------------------ | --------------------------- | ----------------------------- |
| **原理**          | API 请求中声明 `tools`，模型自主判断何时调用搜索 | 发送前先执行搜索，结果注入 system prompt | 所有请求经 Sidecar 中转，Sidecar 处理搜索 |
| **搜索时机**        | 模型自主判断（准确率高）                   | 每次对话都搜索（大量冗余）               | Sidecar 代理判断（增加延迟）            |
| **架构侵入**        | 低（改 `sendApiChat` + 新增工具模块）    | 低（改 `handleSend` 前置搜索）      | 高（需重构通信架构）                    |
| **可扩展性**        | 高（通用 tools 协议）                 | 低（硬编码搜索逻辑）                  | 中（Sidecar 统一管理）               |
| **API 兼容性**     | OpenAI 兼容协议，主流模型支持             | 无特殊要求                       | 无特殊要求                         |
| **对 API 直连的影响** | 无影响（`tools` 为可选参数）             | 增加延迟（前置搜索阻塞）                | 破坏直连优势，增加依赖                   |

> **推荐方案 A（标准 Function Calling）**  
> 理由：OpenAI 兼容的 `tools` 协议已成为事实标准（DeepSeek、Qwen、Claude 均已支持）；模型自主判断搜索时机准确率高；改动集中在 `sendApiChat` 不引入新进程依赖；保持 API 直连模式的核心优势；可扩展为通用工具系统。

### 2.2 搜索引擎选型

#### 需求约束

- **国内可用**：DuckDuckGo 在国内无法访问，直接排除
- **零配置开箱即用**：不依赖外部服务注册、API Key、Docker 部署
- **可移植**：搜索能力应内置到软件中，用户安装即可使用

#### 国内可访问搜索引擎对比

| 引擎 | 国内可用 | 反爬难度 | 搜索质量 | 需注册/API | 评估 |
|------|---------|---------|---------|-----------|------|
| **必应国内版** `cn.bing.com` | 可用 | 中等（可控） | 高（中文优秀） | 否 | **首选** |
| 百度 `baidu.com` | 可用 | 高（JS渲染+验证码） | 高 | 否 | 备选，反爬严格 |
| 搜狗 `sogou.com` | 可用 | 低 | 中 | 否 | 备选，质量一般 |
| 360搜索 `so.com` | 可用 | 低 | 中 | 否 | 备选 |
| SearXNG | 需自部署 | 无 | 高 | 可选 | 高级选项 |
| Tavily | 可用 | 无 | 高 | 需Key+付费 | 高级选项 |

> **首选必应国内版（内置），备选 SearXNG / Tavily**  
> 理由：必应国内版可直接访问、中文搜索质量优秀、HTML 结构清晰（`li.b_algo`）、零配置零注册、Rust `scraper` crate 解析简单。搜索逻辑放在 Rust 后端（Tauri command），用 `reqwest` + `scraper` 实现，不增加运行时服务依赖。SearXNG 和 Tavily 作为高级选项供有需要的用户选择。
>
> 详细方案参见：[内置搜索方案-必应国内版.md](./PilotDesk-内置搜索方案-必应国内版.md)

### 2.3 工具循环执行位置

> **在前端（`useWebSocket`）而非 Sidecar**  
> 理由：保持 API 直连模式是 PilotDesk 的核心架构优势；工具调用仅需 HTTP 请求搜索引擎，无需 Sidecar 中转；前端可直接控制循环次数和 UI 反馈状态。

---

## 3. 整体架构

### 3.1 消息流架构（含工具循环）

```
用户输入
  │
  ▼
handleSend()
  ├── addMessage(userMsg) → sessionStore
  │
  ▼
sendApiChat(messages, { tools, toolChoice })
  │
  ├── 构建请求体（含 tools 定义）
  ├── SSE 流式接收
  │     ├── delta.content ──────────────▶ onChunk → 实时渲染
  │     ├── delta.reasoning_content ───▶ onReasoning → 折叠面板
  │     └── delta.tool_calls ──────────▶ 累积工具调用参数
  │
  ▼
API 返回 finish_reason = "tool_calls"
  │
  ▼  ┌──────────────────────────────────┐
  │  │  while (round < MAX_TOOL_ROUNDS)                         │
  │  │                                                                                         │
  │  │  toolExecutor.execute(toolCall)                                       │
  │  │    ├── web_search                                                         │
  │  │    │   └── searchProvider.search                                   │
  │  │    │       └── SearXNG / Tavily                                       │
  │  │    └── 返回搜索结果                                                      │
  │  │                                                                                        │
  │  │  将工具结果追加到 messages 数组                                │
  │  │  继续循环 → sendApiChat(...)                                        │
  │  └──────────────────────────────────┘
  │
  ▼
API 返回最终 content（finish_reason = "stop"）
  │
  ▼
onDone → addMessage(assistantMsg) → sessionStore + SQLite
```

### 3.2 模块依赖关系

```
                    ┌─────────────────────────┐
                    │    types/index.ts                                      │
                    │  (扩展: Tool*, Message)                          │
                    └───────────┬─────────────┘
                                                   │
              ┌───────────── ┼───────────────┐
              │                                   │                                      │
              ▼                                 ▼                                    ▼
    ┌─────── ───┐  ┌────────── ─┐  ┌───────────┐
    │ apiClient.ts        │  │ searchProvider     │  │ toolExecutor       │
    │ (扩展 tools)       │  │  .ts (新建)              │  │  .ts (新建)            │
    └───┬──────┘  └──────┬────  ┘  └──────┬────┘
              │                                      │                                   │
              │                      ┌─────┘                                   │
              │                      │                                                   │
             ▼                     ▼                                                  │
    ┌─────────────────────────────┐        │
    │     useWebSocket.ts                                          │◄─ ┘
    │  (sendApiChat → while 循环)                            │
    └─────────────┬───────────────┘
                                        │
                                       ▼
    ┌─────────────────────────────┐
    │     MessageBubble.tsx                                      │
    │  (推理面板 + 工具状态指示)                              │
    └─────────────────────────────┘
```

---

## 4. 详细技术设计

### 4.1 类型定义扩展（`src/types/index.ts`）

现有 `Message` 接口：

```typescript
export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
}
```

扩展为：

```typescript
export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';  // 新增 'tool'
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
  /** 模型推理/思考内容 */
  reasoningContent?: string;
  /** 工具调用请求（JSON 数组字符串） */
  toolCalls?: string;
  /** 关联的 tool_call ID（role='tool' 时使用） */
  toolCallId?: string;
  /** 工具名称（role='tool' 时使用） */
  toolName?: string;
}

/** OpenAI Function Calling 格式的工具定义 */
export interface ToolDefinition {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>; // JSON Schema
  };
}

/** 模型返回的工具调用 */
export interface ToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string; // JSON string
  };
}

/** 工具执行结果 */
export interface ToolResult {
  toolCallId: string;
  toolName: string;
  content: string;
  isError?: boolean;
}

/** 搜索结果项 */
export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}
```

**设计要点**：

- `role` 新增 `'tool'`，对应 OpenAI 协议中的工具结果消息
- `toolCalls` 使用 JSON 字符串而非对象数组，因为 Rust 端存储为 TEXT，避免序列化复杂性
- `reasoningContent` 可选字段，不支持的模型不返回此字段

### 4.2 apiClient.ts 扩展

#### 4.2.1 `buildBody` 支持 tools 参数

当前 `buildBody` 签名：

```typescript
export function buildBody(
  fmt: ApiFormat,
  options: Pick<ApiRequestOptions, 'model' | 'messages' | 'stream' | 'maxTokens'>,
): Record<string, unknown>
```

扩展为：

```typescript
export interface BuildBodyOptions {
  model: string;
  messages: Array<{ role: string; content: string }>;
  stream: boolean;
  maxTokens?: number;
  /** Function Calling 工具定义列表 */
  tools?: ToolDefinition[];
  /** 工具选择策略 */
  toolChoice?: 'auto' | 'none' | { type: 'function'; function: { name: string } };
}

export function buildBody(fmt: ApiFormat, options: BuildBodyOptions): Record<string, unknown> {
  const { model, messages, stream = false, maxTokens, tools, toolChoice } = options;

  if (fmt === 'anthropic') {
    const body: Record<string, unknown> = {
      model,
      max_tokens: maxTokens ?? 4096,
      stream,
      messages: messages.map((m) => ({ role: m.role, content: m.content })),
    };
    if (tools?.length) {
      // Anthropic 格式: tools 转为 tool_choice + tool 定义
      body.tools = tools.map(t => ({
        name: t.function.name,
        description: t.function.description,
        input_schema: t.function.parameters,
      }));
      if (toolChoice) body.tool_choice = toolChoice;
    }
    return body;
  }

  const body: Record<string, unknown> = {
    model,
    stream,
    ...(maxTokens != null ? { max_tokens: maxTokens } : {}),
    messages: messages.map((m) => ({ role: m.role, content: m.content })),
  };

  if (tools?.length) {
    body.tools = tools;
    if (toolChoice) body.tool_choice = toolChoice;
  }

  return body;
}
```

**设计要点**：

- `tools` 为可选参数，不传时完全退化为现有行为（向后兼容）
- `toolChoice` 默认 `'auto'`（模型自主判断），也支持指定特定工具
- Anthropic 的 tools 格式与 OpenAI 不同，需要转换（`function` → 直接 `name/input_schema`）

#### 4.2.2 SSE 解析扩展（reasoning_content + tool_calls）

当前 OpenAI 格式的 SSE 解析仅提取 `delta.content`：

```typescript
// 现有代码
const delta = event.choices?.[0]?.delta?.content;
if (delta) {
  h?.onChunk?.(sessionId, delta);
}
```

扩展为完整解析：

```typescript
interface SseCallbacks {
  onChunk: (sessionId: string, content: string) => void;
  onReasoning?: (sessionId: string, content: string) => void;
  onToolCallDelta?: (sessionId: string, index: number, field: string, value: string) => void;
  onDone: (sessionId: string) => void;
  onError: (sessionId: string, error: string) => void;
}

function parseOpenAISSE(line: string, sessionId: string, callbacks: SseCallbacks) {
  const data = line.slice(6).trim();
  if (data === '[DONE]') return;

  const event = JSON.parse(data);
  const choice = event.choices?.[0];
  if (!choice) return;

  const delta = choice.delta;
  const finishReason = choice.finish_reason;

  // 文本内容
  if (delta?.content) {
    callbacks.onChunk(sessionId, delta.content);
  }

  // 推理内容（DeepSeek、部分模型）
  if (delta?.reasoning_content) {
    callbacks.onReasoning?.(sessionId, delta.reasoning_content);
  }

  // 工具调用（流式分片累积）
  if (delta?.tool_calls) {
    for (const tc of delta.tool_calls) {
      if (tc.function?.name) {
        callbacks.onToolCallDelta?.(sessionId, tc.index, 'name', tc.function.name);
      }
      if (tc.function?.arguments) {
        callbacks.onToolCallDelta?.(sessionId, tc.index, 'arguments', tc.function.arguments);
      }
    }
  }

  // 流结束判断
  if (finishReason === 'stop' || finishReason === 'tool_calls') {
    callbacks.onDone(sessionId);
  }
}
```

**tool_calls 流式累积机制**：

模型返回 tool_calls 时是分片的，需要按 index 累积：

```typescript
// 工具调用缓冲区（sendApiChat 内部维护）
const toolCallBuffer = new Map<number, { id: string; name: string; arguments: string }>();

// 每次收到 tool_calls 分片
onToolCallDelta: (index, field, value) => {
  if (!toolCallBuffer.has(index)) {
    toolCallBuffer.set(index, { id: '', name: '', arguments: '' });
  }
  const entry = toolCallBuffer.get(index)!;
  if (field === 'id') entry.id = value;
  else if (field === 'name') entry.name = value;
  else if (field === 'arguments') entry.arguments += value;
}
```

### 4.3 搜索引擎模块

#### 方案变更说明

v1.0 方案中搜索引擎模块为前端 TypeScript 实现（`searchProvider.ts`），默认引擎 SearXNG。v1.1 更新为：**默认引擎为必应国内版，实现在 Rust 后端**；前端 TypeScript 仅保留 SearXNG / Tavily 的适配层供高级用户选用。

#### 4.3.1 Rust 端：内置必应搜索（`src-tauri/src/commands/search.rs`，新建）

```rust
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 内置必应搜索（cn.bing.com HTML 抓取）
#[tauri::command]
pub async fn web_search(query: String, max_results: Option<usize>) -> Result<Vec<SearchResult>, AppError> {
    let max = max_results.unwrap_or(5);
    let url = format!(
        "https://cn.bing.com/search?q={}&count={}",
        urlencoding::encode(&query),
        max
    );

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError { code: "ERR_CLIENT".into(), message: "HTTP 客户端创建失败".into(), details: Some(e.to_string()) })?;

    let html = client.get(&url).send().await
        .map_err(|e| AppError { code: "ERR_FETCH".into(), message: "搜索请求失败".into(), details: Some(e.to_string()) })?
        .text().await
        .map_err(|e| AppError { code: "ERR_BODY".into(), message: "读取响应失败".into(), details: Some(e.to_string()) })?;

    let document = Html::parse_document(&html);
    let item_sel = Selector::parse("li.b_algo").unwrap();
    let title_sel = Selector::parse("h2 a").unwrap();
    let link_sel = Selector::parse("h2 a[href]").unwrap();
    let snippet_sel = Selector::parse(".b_caption p, .b_caption .b_attribution").unwrap();

    let mut results = Vec::new();
    for element in document.select(&item_sel).take(max) {
        let title = element.select(&title_sel).next()
            .map(|e| e.text().collect::<String>()).unwrap_or_default();
        let url = element.select(&link_sel).next()
            .and_then(|e| e.value().attr("href")).unwrap_or_default();
        let snippet = element.select(&snippet_sel).next()
            .map(|e| e.text().collect::<String>()).unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult { title, url, snippet });
        }
    }

    Ok(results)
}
```

**Rust 依赖新增**：`scraper`（~50KB）、`urlencoding`

**HTML 选择器说明**：

| 选择器 | 作用 | 说明 |
|--------|------|------|
| `li.b_algo` | 结果容器 | 每条搜索结果的主容器 |
| `h2 a` | 标题文本 | 搜索结果标题 |
| `h2 a[href]` | 链接地址 | 搜索结果 URL |
| `.b_caption p` | 摘要文本 | 搜索结果摘要 |

#### 4.3.2 前端 TypeScript：外部引擎适配层（`src/utils/searchProvider.ts`，新建）

```typescript
/**
 * 前端搜索适配层。
 * 默认使用 Rust 内置必应搜索（通过 Tauri command），
 * 高级用户可切换为 SearXNG / Tavily（通过 HTTP 直接调用）。
 */
export interface SearchProvider {
  name: string;
  search(query: string, maxResults?: number): Promise<SearchResult[]>;
}

// ========== SearXNG 实现（高级选项）==========

export class SearXNGProvider implements SearchProvider {
  name = 'searxng';

  constructor(
    private baseUrl: string,    // e.g. 'http://localhost:8888'
    private apiKey?: string,
  ) {}

  async search(query: string, maxResults = 5): Promise<SearchResult[]> {
    const url = `${this.baseUrl.replace(/\/+$/, '')}/search?`
      + `q=${encodeURIComponent(query)}`
      + `&format=json`
      + `&categories=general`;

    const headers: Record<string, string> = { 'Accept': 'application/json' };
    if (this.apiKey) headers['Authorization'] = `Bearer ${this.apiKey}`;

    const res = await fetch(url, { headers, signal: AbortSignal.timeout(10000) });
    if (!res.ok) throw new Error(`SearXNG 请求失败: HTTP ${res.status}`);

    const data = await res.json();
    return (data.results || []).slice(0, maxResults).map((r: any) => ({
      title: r.title || '',
      url: r.url || '',
      snippet: r.content || '',
    }));
  }
}

// ========== Tavily 实现（高级选项）==========

export class TavilyProvider implements SearchProvider {
  name = 'tavily';

  constructor(private apiKey: string, private maxResults = 5) {}

  async search(query: string, maxResults?: number): Promise<SearchResult[]> {
    const res = await fetch('https://api.tavily.com/search', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        api_key: this.apiKey,
        query,
        max_results: maxResults ?? this.maxResults,
        include_answer: false,
      }),
      signal: AbortSignal.timeout(10000),
    });
    if (!res.ok) throw new Error(`Tavily 请求失败: HTTP ${res.status}`);

    const data = await res.json();
    return (data.results || []).map((r: any) => ({
      title: r.title || '',
      url: r.url || '',
      snippet: r.content || '',
    }));
  }
}
```

### 4.4 工具执行器（`src/utils/toolExecutor.ts`，新建）

```typescript
import { invoke } from '@tauri-apps/api/core';
import type { ToolDefinition, ToolCall, ToolResult, SearchResult } from '../types';
import { SearXNGProvider, TavilyProvider, type SearchProvider } from './searchProvider';

// ========== 内置 web_search 工具定义 ==========
export const WEB_SEARCH_TOOL: ToolDefinition = {
  type: 'function',
  function: {
    name: 'web_search',
    description: '搜索互联网获取最新信息。当用户询问时效性内容、'
      + '最新新闻、实时数据、或你不确定的事实时，调用此工具。',
    parameters: {
      type: 'object',
      properties: {
        query: {
          type: 'string',
          description: '搜索关键词',
        },
      },
      required: ['query'],
    },
  },
};

// ========== 工具执行器 ==========

export type SearchEngineType = 'bing' | 'searxng' | 'tavily';

export class ToolExecutor {
  private engineType: SearchEngineType = 'bing';
  private provider: SearchProvider | null = null;

  /** 设置搜索引擎类型 */
  setEngineType(type: SearchEngineType) {
    this.engineType = type;
  }

  /** 设置外部搜索引擎提供商（SearXNG / Tavily） */
  setProvider(provider: SearchProvider) {
    this.provider = provider;
  }

  /** 执行搜索 */
  private async doSearch(query: string, maxResults = 5): Promise<SearchResult[]> {
    // 默认引擎：Rust 内置必应搜索
    if (this.engineType === 'bing' || !this.provider) {
      return await invoke<SearchResult[]>('web_search', { query, maxResults });
    }
    // 外部引擎：通过前端 HTTP 调用
    return await this.provider.search(query, maxResults);
  }

  /** 执行工具调用 */
  async execute(toolCall: ToolCall): Promise<ToolResult> {
    const { name, arguments: argsStr } = toolCall.function;
    let args: Record<string, unknown>;
    try {
      args = JSON.parse(argsStr);
    } catch {
      return {
        toolCallId: toolCall.id,
        toolName: name,
        content: `工具参数解析失败: ${argsStr}`,
        isError: true,
      };
    }

    switch (name) {
      case 'web_search': {
        try {
          const results = await this.doSearch((args.query as string) || '', 5);
          if (results.length === 0) {
            return { toolCallId: toolCall.id, toolName: name, content: '未找到相关搜索结果。' };
          }
          const content = results
            .map((r) => `【${r.title}】(${r.url})\n${r.snippet}`)
            .join('\n\n');
          return { toolCallId: toolCall.id, toolName: name, content };
        } catch (err) {
          return {
            toolCallId: toolCall.id,
            toolName: name,
            content: `搜索失败: ${err instanceof Error ? err.message : String(err)}`,
            isError: true,
          };
        }
      }

      default:
        return {
          toolCallId: toolCall.id,
          toolName: name,
          content: `未知工具: ${name}`,
          isError: true,
        };
    }
  }

  /** 获取所有已注册的工具定义（用于发送给 API） */
  getToolDefinitions(): ToolDefinition[] {
    // 只要搜索功能启用就注册 web_search 工具
    return [WEB_SEARCH_TOOL];
  }
}

// 全局单例
export const toolExecutor = new ToolExecutor();
```

### 4.5 sendApiChat 工具循环改造（`src/hooks/useWebSocket.ts`）

这是核心改造。当前 `sendApiChat` 是单次请求-响应模式，需改为 **while 循环处理 tool_calls**。

#### 当前签名

```typescript
const sendApiChat = useCallback(async (
  sessionId: string,
  message: string,
  apiEndpoint: string,
  providerId: string,
  model: string,
  history?: Array<{ role: string; content: string }>,
  providerName?: string,
) => { ... }, []);
```

#### 新签名与回调设计

```typescript
interface ApiChatCallbacks {
  onChunk: (sessionId: string, content: string) => void;
  onReasoning?: (sessionId: string, content: string) => void;
  onToolCallStart?: (sessionId: string, toolName: string, query: string) => void;
  onToolResult?: (sessionId: string, result: ToolResult) => void;
  onToolRound?: (sessionId: string, round: number, maxRounds: number) => void;
  onDone: (sessionId: string) => void;
  onError: (sessionId: string, error: string) => void;
  onStatus: (sessionId: string, status: string) => void;
}
```

#### 核心伪代码

```typescript
async function sendApiChat(params: {
  sessionId: string;
  message: string;
  apiEndpoint: string;
  providerId: string;
  model: string;
  history?: Array<{ role: string; content: string }>;
  providerName?: string;
}, callbacks: ApiChatCallbacks) {

  const MAX_TOOL_ROUNDS = 5;
  let round = 0;
  let allMessages = [
    ...(history || []).map(m => ({ role: m.role, content: m.content })),
    { role: 'user', content: message },
  ];
  let fullAssistantContent = '';
  let fullReasoningContent = '';

  // 获取工具定义
  const tools = toolExecutor.getToolDefinitions();

  while (round < MAX_TOOL_ROUNDS) {
    round++;

    // 1. 构建 SSE 请求
    const body = buildBody(fmt, {
      model,
      messages: allMessages,
      stream: true,
      tools: tools.length > 0 ? tools : undefined,
      toolChoice: tools.length > 0 ? 'auto' : undefined,
    });

    // 2. 流式解析
    let assistantContent = '';
    let reasoningContent = '';
    let finishReason = '';
    const toolCallBuffer = new Map<number, Partial<ToolCall>>();

    const res = await fetch(chatUrl, {
      method: 'POST',
      headers: buildHeaders(fmt, key),
      body: JSON.stringify(body),
      signal: abort.signal,
    });

    // 3. SSE 逐行解析
    // ... (逐行解析逻辑，累积 content / reasoning_content / tool_calls)
    // 当 finish_reason === 'tool_calls' 时进入工具执行

    // 4. 将 assistant 消息加入历史
    allMessages.push({ role: 'assistant', content: assistantContent });

    // 5. 判断是否需要执行工具
    if (finishReason === 'tool_calls' && toolCallBuffer.size > 0) {
      // 通知 UI: 工具调用中
      callbacks.onToolRound?.(sessionId, round, MAX_TOOL_ROUNDS);

      // 执行每个工具
      for (const [index, tc] of toolCallBuffer.entries()) {
        const fullTc: ToolCall = {
          id: tc.id || `call_${index}`,
          type: 'function',
          function: { name: tc.function?.name || '', arguments: tc.function?.arguments || '' },
        };

        callbacks.onToolCallStart?.(sessionId, fullTc.function.name, fullTc.function.arguments);
        const result = await toolExecutor.execute(fullTc);
        callbacks.onToolResult?.(sessionId, result);

        // 将工具结果加入消息历史（OpenAI 协议要求 role='tool'）
        allMessages.push({ role: 'tool', content: result.content });
      }

      // 继续循环，让模型处理工具结果
      continue;
    }

    // 6. 最终回复（无工具调用）
    fullAssistantContent = assistantContent;
    fullReasoningContent = reasoningContent;
    break;
  }

  // 7. 完成
  callbacks.onDone(sessionId);
  return { content: fullAssistantContent, reasoningContent: fullReasoningContent };
}
```

#### WsHandlers 扩展

```typescript
interface WsHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSkills?: (agentType: string, skills: string[]) => void;
  // 新增回调
  onReasoning?: (sessionId: string, content: string) => void;
  onToolCallStart?: (sessionId: string, toolName: string, query: string) => void;
  onToolResult?: (sessionId: string, result: ToolResult) => void;
  onToolRound?: (sessionId: string, round: number, maxRounds: number) => void;
}
```

### 4.6 MessageBubble 推理内容渲染

在 AI 消息气泡中，当 `message.reasoningContent` 存在时，显示可折叠的思考过程面板：

```tsx
// MessageBubble.tsx 中 AI 消息区域（卡片内部）

{/* 推理内容折叠面板 */}
{message.reasoningContent && (
  <div className="reasoning-panel">
    <button
      onClick={() => setReasoningOpen(!reasoningOpen)}
      className="reasoning-toggle"
    >
      <span className="reasoning-arrow">{reasoningOpen ? '▾' : '▸'}</span>
      <span className="reasoning-label">思考过程</span>
      <span className="reasoning-meta">
        {message.reasoningContent.length > 200
          ? `${Math.ceil(message.reasoningContent.length / 100)}字`
          : ''}
      </span>
    </button>
    {reasoningOpen && (
      <div className="reasoning-content">
        <MarkdownRenderer content={message.reasoningContent} />
      </div>
    )}
  </div>
)}

{/* 主要回复内容 */}
<MarkdownRenderer content={message.content} />
```

#### 工具状态指示器

在消息列表底部（typing indicator 区域）显示工具执行状态：

```tsx
{/* 工具执行状态（MessageList.tsx 或 MainPanel.tsx 中） */}
{toolStatus && (
  <div className="tool-status-bar">
    {toolStatus.type === 'searching' && (
      <span>
        <LoadingSpinner size={14} />
        正在搜索: {toolStatus.query}
      </span>
    )}
    {toolStatus.type === 'analyzing' && (
      <span>
        <LoadingSpinner size={14} />
        第 {toolStatus.round}/{toolStatus.maxRounds} 轮 · 模型正在分析搜索结果
      </span>
    )}
  </div>
)}
```

### 4.7 sessionStore.ts 同步

`addMessage` 需要支持新字段：

```typescript
// save_message 调用扩展
invoke<Message>('save_message', {
  sessionId: msg.sessionId,
  role: msg.role,
  content: msg.content,
  mode: msg.mode,
  reasoningContent: msg.reasoningContent || null,   // 新增
  toolCalls: msg.toolCalls || null,                    // 新增
  toolCallId: msg.toolCallId || null,                 // 新增
  toolName: msg.toolName || null,                      // 新增
});
```

### 4.8 MainPanel.tsx 集成

`MainPanel` 中的 `handleSend` 需要传递新增的回调给 `sendApiChat`：

```typescript
// 新增状态
const [reasoningContent, setReasoningContent] = useState('');
const [toolStatus, setToolStatus] = useState<ToolStatus | null>(null);

// sendApiChat 调用时新增回调
onReasoning: (sid, content) => {
  if (sid === currentSessionId) setReasoningContent(prev => prev + content);
},
onToolCallStart: (sid, name, query) => {
  if (sid === currentSessionId) {
    setToolStatus({ type: 'searching', toolName: name, query });
  }
},
onToolResult: (sid, result) => {
  if (sid === currentSessionId) {
    setToolStatus(prev => prev ? { ...prev, resultCount: (prev.resultCount || 0) + 1 } : null);
  }
},
onToolRound: (sid, round, maxRounds) => {
  if (sid === currentSessionId) {
    setToolStatus({ type: 'analyzing', round, maxRounds });
  }
},
```

---

## 5. 数据库改动

### 5.1 messages 表 Migration

```sql
-- 新增推理和工具相关字段
ALTER TABLE messages ADD COLUMN reasoning_content TEXT;
ALTER TABLE messages ADD COLUMN tool_calls TEXT;       -- JSON array string
ALTER TABLE messages ADD COLUMN tool_call_id TEXT;
ALTER TABLE messages ADD COLUMN tool_name TEXT;

-- 放宽 role 字段的 CHECK 约束（新增 'tool'）
-- 注意: SQLite 不支持 ALTER CONSTRAINT，需要重建表
```

由于 SQLite 不支持修改 CHECK 约束，需要通过重建表的方式：

```sql
-- Migration: 重建 messages 表以支持 'tool' role + 新字段
CREATE TABLE IF NOT EXISTS messages_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL DEFAULT '',
    mode TEXT DEFAULT 'native' CHECK(mode IN ('native', 'fast', 'think', 'expert')),
    timestamp INTEGER NOT NULL,
    reasoning_content TEXT,
    tool_calls TEXT,
    tool_call_id TEXT,
    tool_name TEXT
);

INSERT OR IGNORE INTO messages_new
  (id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name)
SELECT id, session_id, role, content, mode, timestamp, NULL, NULL, NULL, NULL
FROM messages;

DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
```

### 5.2 search_providers 表（新增）

```sql
CREATE TABLE IF NOT EXISTS search_providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,              -- 'searxng' | 'tavily'
    base_url TEXT NOT NULL DEFAULT '',     -- SearXNG 服务器地址
    api_key TEXT DEFAULT '',                -- API Key（可选）
    api_key_masked TEXT DEFAULT '',         -- 遮罩显示
    api_key_set INTEGER DEFAULT 0,           -- 是否已设置
    max_results INTEGER NOT NULL DEFAULT 5,
    sort_order INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

### 5.3 app_settings 表扩展

```sql
-- 搜索相关设置（启动时检查并初始化默认值）
INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES
  ('search_enabled', 'false', strftime('%s', 'now')),
  ('search_provider_name', 'searxng', strftime('%s', 'now')),
  ('search_max_results', '5', strftime('%s', 'now')),
  ('max_tool_rounds', '5', strftime('%s', 'now'));
```

### 5.4 Rust Message struct 扩展（`src-tauri/src/db/models.rs`）

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub mode: String,
    pub timestamp: i64,
    // 新增字段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}
```

### 5.5 Rust save_message 命令扩展（`src-tauri/src/commands/session.rs`）

```rust
#[tauri::command]
pub fn save_message(
    state: State<'_, DbState>,
    session_id: String,
    role: String,
    content: String,
    mode: String,
    reasoning_content: Option<String>,  // 新增
    tool_calls: Option<String>,         // 新增
    tool_call_id: Option<String>,        // 新增
    tool_name: Option<String>,           // 新增
) -> Result<Message, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO messages (id, session_id, role, content, mode, timestamp,
         reasoning_content, tool_calls, tool_call_id, tool_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, session_id, role, content, mode, now,
                reasoning_content, tool_calls, tool_call_id, tool_name],
    )?;

    // ... 更新 session preview

    Ok(Message {
        id, session_id, role, content, mode, timestamp: now,
        reasoning_content, tool_calls, tool_call_id, tool_name,
    })
}
```

---

## 6. 设置页扩展

在 `SettingsPage.tsx` 中新增 **"联网搜索"** 配置区块（位于 API 提供商区块下方）：

### 6.1 UI 布局

```
┌──────────────────────────────────────────────┐
│ 🔍 联网搜索设置                                                                                         │
├──────────────────────────────────────────────┤
│                                                                                                                      │
│ 启用联网搜索          [●━━━━━━━] ON                                                           │
│                                                                                                                      │
│ 搜索引擎              [SearXNG      ▾]                                                               │
│                                                                                                                      │
│ ┌── SearXNG 配置 ─────────────────────────┐                  │
│ │ 服务器地址  [http://localhost:8888    ]                                    │                  │
│ │ API Key     [                      ]                                                    │                  │
│ └─────────────────────────────────────┘                  │
│                                                                                                                       │
│ 最大搜索结果          [━━●━━━━━] 5                                                                │
│ 最大工具轮数          [━━●━━━━━] 5                                                                │
│                                                                                                                       │
│ [测试连接]                                                                                                     │
│                                                                                                                      │
└──────────────────────────────────────────────┘
```

### 6.2 状态管理

搜索配置存储在 `app_settings` 表中，通过现有的 `getAppSetting` / `setAppSetting` Tauri 命令读写：

```typescript
interface SearchConfig {
  enabled: boolean;
  engineType: 'bing' | 'searxng' | 'tavily';
  // 必应（内置）无需配置
  // SearXNG 配置
  searxngBaseUrl: string;
  searxngApiKey: string;
  // Tavily 配置
  tavilyApiKey: string;
  // 通用配置
  maxResults: number;
  maxToolRounds: number;
}
```

配置变更时同步初始化 `toolExecutor`：

```typescript
function applySearchConfig(config: SearchConfig) {
  if (!config.enabled) return;

  toolExecutor.setEngineType(config.engineType);

  if (config.engineType === 'searxng') {
    toolExecutor.setProvider(new SearXNGProvider(
      config.searxngBaseUrl, config.searxngApiKey || undefined
    ));
  } else if (config.engineType === 'tavily') {
    toolExecutor.setProvider(new TavilyProvider(
      config.tavilyApiKey, config.maxResults
    ));
  }
  // engineType === 'bing' 时无需设置 provider，Rust 内置直接可用
}
```

### 6.3 连接测试

```typescript
async function testSearchConnection() {
  const config = getCurrentSearchConfig();

  if (config.engineType === 'bing') {
    // 必应内置：通过 Rust command 测试
    const results = await invoke<SearchResult[]>('web_search', { query: '测试', maxResults: 1 });
    showToast(`必应搜索连接成功，返回 ${results.length} 条结果`, 'success');
  } else {
    // 外部引擎：通过前端 HTTP 测试
    const provider = config.engineType === 'searxng'
      ? new SearXNGProvider(config.searxngBaseUrl)
      : new TavilyProvider(config.tavilyApiKey);
    const results = await provider.search('测试', 1);
    showToast(`${config.engineType} 连接成功，返回 ${results.length} 条结果`, 'success');
  }
}
```

---

## 7. 文件变更清单

| 文件                                         | 操作     | 变更内容                                                                                                |
| ------------------------------------------ | ------ | --------------------------------------------------------------------------------------------------- |
| `src/types/index.ts`                       | **修改** | Message 扩展 5 个可选字段 + role 新增 `'tool'`；新增 `ToolDefinition`、`ToolCall`、`ToolResult`、`SearchResult` 类型 |
| `src/utils/apiClient.ts`                   | **修改** | `buildBody` 新增 `tools` / `toolChoice` 参数；SSE 解析支持 `reasoning_content` 和 `tool_calls` 流式累积           |
| `src/utils/searchProvider.ts`              | **新建** | `SearchProvider` 接口 + `SearXNGProvider` + `TavilyProvider`（约 80 行，仅高级选项使用） |
| `src-tauri/src/commands/search.rs`          | **新建** | Rust 内置必应搜索 `web_search` command（`reqwest` + `scraper`，约 60 行） |
| `src/utils/toolExecutor.ts`                | **新建** | `ToolExecutor` 类 + `WEB_SEARCH_TOOL` 定义 + 全局单例（约 120 行）                                             |
| `src/hooks/useWebSocket.ts`                | **修改** | `WsHandlers` 新增 4 个回调；`sendApiChat` 改造为 while 工具循环；内部维护 `toolCallBuffer`                            |
| `src/stores/sessionStore.ts`               | **修改** | `addMessage` 的 `invoke('save_message')` 传递新字段                                                       |
| `src/components/message/MessageBubble.tsx` | **修改** | 推理内容折叠面板 + `reasoningOpen` 状态                                                                       |
| `src/components/message/MessageList.tsx`   | **修改** | 工具状态指示器（搜索中 / 分析中）                                                                                  |
| `src/components/layout/MainPanel.tsx`      | **修改** | `handleSend` 传递新增回调；新增 `reasoningContent` / `toolStatus` 状态                                         |
| `src/components/settings/SettingsPage.tsx` | **修改** | 新增"联网搜索"配置区块                                                                                        |
| `src/styles/globals.css`                   | **修改** | 推理面板样式 + 工具状态指示器样式                                                                                  |
| `src-tauri/src/db/models.rs`               | **修改** | Message struct 扩展 4 个 `Option<String>` 字段                                                           |
| `src-tauri/src/db/init.rs`                 | **修改** | Migration: messages 表重建（新字段 + tool role）；search_providers 新表；app_settings 默认值                       |
| `src-tauri/src/commands/session.rs`        | **修改** | `row_to_message` 读取新列；`save_message` 接受并存储新字段                                                       |

**新增文件**: 3 个（`searchProvider.ts`、`toolExecutor.ts`、`search.rs`）
**修改文件**: 12 个
**新增 Rust 代码量**: 约 60 行（`search.rs`）
**新增 TypeScript 代码量**: 约 400 行

---

## 8. 实施路线图

### Phase 1: 推理可视化（3-4h）

**目标**: 解析并渲染 `reasoning_content` 字段

| 步骤   | 文件                  | 内容                                                                  |
| ---- | ------------------- | ------------------------------------------------------------------- |
| 1.1  | `types/index.ts`    | Message 新增 `reasoningContent` 字段                                    |
| 1.2  | `models.rs`         | Message struct 新增 `reasoning_content: Option<String>`               |
| 1.3  | `init.rs`           | Migration: messages 表新增 `reasoning_content TEXT` 列                  |
| 1.4  | `session.rs`        | `row_to_message` + `save_message` 支持 `reasoning_content`            |
| 1.4b | `search.rs`         | Rust 内置必应搜索 `web_search` command（可独立于 Phase 2 提前验证） |
| 1.5  | `sessionStore.ts`   | `addMessage` 传递 `reasoningContent`                                  |
| 1.6  | `apiClient.ts`      | SSE 解析提取 `delta.reasoning_content`                                  |
| 1.7  | `useWebSocket.ts`   | `sendApiChat` OpenAI 分支解析 `reasoning_content` + 新增 `onReasoning` 回调 |
| 1.8  | `MessageBubble.tsx` | 推理内容折叠面板 UI                                                         |
| 1.9  | `globals.css`       | `.reasoning-panel` 样式                                               |
| 1.10 | `MainPanel.tsx`     | 传递 `onReasoning` 回调，维护 `reasoningContent` 状态                        |

**验收标准**: 发送消息到支持 reasoning 的模型（如 DeepSeek）时，消息气泡上方出现可折叠的"思考过程"面板。

### Phase 2: 搜索基础架构（4-5h）

**目标**: 创建搜索和工具执行模块

| 步骤  | 文件                  | 内容                                                             |
| --- | ------------------- | -------------------------------------------------------------- |
| 2.1 | `types/index.ts`    | 新增 `ToolDefinition`、`ToolCall`、`ToolResult`、`SearchResult`     |
| 2.2 | `search.rs`         | Rust 内置必应搜索 `web_search` command（`reqwest` + `scraper`） |
| 2.3 | `searchProvider.ts` | 前端搜索适配层（SearXNG + Tavily，高级选项）                      |
| 2.4 | `toolExecutor.ts`   | `ToolExecutor` 类（默认走 Rust command，备选走前端 HTTP） |
| 2.5 | `apiClient.ts`      | `buildBody` 支持 `tools` / `toolChoice`；SSE 解析 `tool_calls` 流式累积 |

**验收标准**: `toolExecutor.execute({ id: '1', type: 'function', function: { name: 'web_search', arguments: '{"query":"test"}' } })` 返回搜索结果。

### Phase 3: 工具循环集成（5-6h）

**目标**: `sendApiChat` 改造为 while 循环，完整的工具调用链路

| 步骤  | 文件                | 内容                                                                 |
| --- | ----------------- | ------------------------------------------------------------------ |
| 3.1 | `useWebSocket.ts` | `sendApiChat` 改造为 while 循环 + toolCallBuffer 累积 + 回调通知              |
| 3.2 | `MainPanel.tsx`   | 传递 `onToolCallStart` / `onToolResult` / `onToolRound` 回调           |
| 3.3 | `MessageList.tsx` | 工具状态指示器 UI（搜索中 / 分析中 / 轮次）                                         |
| 3.4 | `types/index.ts`  | Message role 新增 `'tool'` + `toolCalls` / `toolCallId` / `toolName` |

**验收标准**: 对支持 Function Calling 的模型提问时效性问题，模型自动调用 `web_search`，前端显示搜索过程，最终整合结果回复。

### Phase 4: 设置与数据库（3-4h）

**目标**: 设置页搜索配置 + DB 完整支持

| 步骤  | 文件                 | 内容                                                                              |
| --- | ------------------ | ------------------------------------------------------------------------------- |
| 4.1 | `init.rs`          | Migration: messages 表重建（tool role + 所有新字段）；app_settings 默认值 |
| 4.2 | `session.rs`       | `save_message` 完整支持所有新字段；`row_to_message` 读取新列                                  |
| 4.3 | `models.rs`        | Message struct 完整扩展                                                             |
| 4.4 | `search.rs`         | 验证必应搜索 command 稳定性（如 Phase 2 未提前验证） |
| 4.5 | `SettingsPage.tsx` | "联网搜索"配置区块（开关、引擎选择[必应/SearXNG/Tavily]、参数、测试连接） |
| 4.6 | `MainPanel.tsx`    | 启动时从设置加载 searchConfig 并初始化 `toolExecutor` |

**验收标准**: 设置页可配置搜索开关、引擎、参数；保存后发起新对话时生效。

### Phase 5: 测试与优化（3-4h）

**目标**: 端到端测试 + 边界处理

| 步骤  | 内容                              |
| --- | ------------------------------- |
| 5.1 | DeepSeek reasoning_content 解析测试 |
| 5.2 | 必应搜索集成测试（Rust `web_search` command 端到端验证） |
| 5.3 | 必应 HTML 解析健壮性测试（不同 query、广告过滤、空结果处理） |
| 5.4 | 工具循环 5 轮上限测试 |
| 5.5 | 搜索失败回退测试（引擎不可用 → 模型收到错误信息继续对话） |
| 5.6 | 不支持 tools 的模型兼容性测试（退化为普通对话） |
| 5.7 | 多轮工具调用测试（模型需要搜索两次才能回答的场景） |
| 5.8 | 推理内容 + 工具调用并存的场景测试 |

---

## 9. 风险与应对

| 风险                        | 影响           | 概率  | 应对措施                                                                     |
| ------------------------- | ------------ | --- | ------------------------------------------------------------------------ |
| 部分模型不支持 `tools` 参数        | 工具调用失败或被忽略   | 中   | `tools` 为可选参数，不传时退化为普通对话；UI 中可标注模型兼容性 |
| 必应 HTML 结构变更 | 解析失败或结果为空 | 低 | CSS 选择器解耦、异常捕获、错误信息回传模型继续对话；定期验证选择器有效性 |
| 搜索引擎服务不可用（SearXNG/Tavily） | 高级选项搜索失败 | 中 | `try-catch` 包装 + 10s 超时 + 错误信息回传模型继续对话 |
| 工具循环无限死循环                 | API 调用次数失控   | 低   | 硬限制最大轮数（默认 5）+ 单次会话总调用计数限制                                               |
| `reasoning_content` 格式不统一 | 解析失败或显示异常 | 中 | 同时处理 `delta.reasoning_content`（文本）和 `delta.reasoning`（JSON）两种格式；缺失时静默不显示 |
| SQLite CHECK 约束不支持 ALTER  | Migration 失败 | 低   | 通过重建表方式处理（已在 init.rs 中实现类似模式）                                            |
| React StrictMode 双重调用     | 工具循环被执行两次    | 中   | 已有 `apiDoneFiredRef` 守卫机制，工具循环中复用同一守卫逻辑                                  |

---

## 10. 设计决策记录

### DDR-1: 采用标准 Function Calling 协议

**决策**: 使用 OpenAI 兼容的 `tools` 协议实现联网搜索。

**理由**:

- OpenAI 兼容的 tools 协议已成为事实标准，DeepSeek、Qwen、Claude 等主流模型均已支持
- 模型自主判断搜索时机的准确率远高于预设规则
- `tools` 是可选参数，不传时完全退化为普通对话（零成本向后兼容）
- 改动集中在 `sendApiChat` 和 `apiClient.ts`，不引入新进程依赖
- 可扩展为通用工具系统（未来添加文件操作、代码执行等）

**替代方案**: 方案 B（预检索注入）适合简单场景但会产生大量冗余搜索；方案 C（Sidecar 代理）破坏 API 直连优势。

### DDR-2: 默认搜索引擎改为必应国内版（v1.1 更新）

**决策**: 默认搜索引擎从 SearXNG 改为必应国内版（`cn.bing.com` HTML 抓取），实现在 Rust 后端。

**理由**:
- DuckDuckGo 在国内无法访问，SearXNG 需自部署或依赖在线服务
- 必应国内版可直接访问、零配置零注册、开箱即用
- HTML 结构清晰（`li.b_algo`），`scraper` crate 解析简单稳定
- Rust 端实现不增加前端 bundle 大小，不引入运行时服务依赖
- 中文搜索质量优秀，满足国内用户需求

**替代方案**: SearXNG（自部署用户）和 Tavily（有 API Key 的用户）作为高级选项保留。详细方案参见 [内置搜索方案-必应国内版.md](./PilotDesk-内置搜索方案-必应国内版.md)。

### DDR-3: 工具循环在前端而非 Sidecar

**决策**: 工具调用循环在 `useWebSocket.ts`（前端）中执行，不经过 Sidecar。

**理由**:

- 保持 API 直连模式是 PilotDesk 的核心架构优势
- 工具调用仅需 HTTP 请求搜索引擎，不需要 Sidecar 中转
- 前端可以直接控制 UI 反馈（搜索中状态、分析中状态、轮次显示）
- 减少架构复杂度，避免引入新的进程间通信

**替代方案**: 如果未来需要在 Sidecar 中执行需要本地资源的工具（文件系统操作、代码执行），可在 Sidecar 中新增工具代理端点。

### DDR-4: 工具系统可扩展设计

**决策**: `toolExecutor` 采用注册机制，内置 `web_search` 工具，预留扩展接口。

**理由**:

- 当前仅需 `web_search` 一个工具，但架构设计上预留 `registerTool` 方法
- `ToolDefinition` 使用标准 JSON Schema 描述参数，任何符合 OpenAI 协议的工具都可注册
- 未来可轻松扩展：文件操作（`read_file`、`write_file`）、代码执行（`run_code`）、系统信息（`get_system_info`）

### DDR-5: 推理内容持久化到数据库

**决策**: `reasoning_content` 存入 messages 表而非仅内存中。

**理由**:

- 确保刷新页面后仍可查看历史推理过程
- 为后续功能（推理质量评估、推理过程对比）提供数据基础
- 存储成本可控（推理内容通常为几百到几千字符，TEXT 字段无额外开销）
- `skip_serializing_if = "Option::is_none"` 确保空值不影响 JSON 传输性能

---

> PilotDesk Function Calling 与联网搜索技术实现方案 v1.1 | 2026-06-02
