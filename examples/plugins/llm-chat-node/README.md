# LLM Chat Node 插件

工作流插件节点：选择 API 供应商和模型，通过提示词与 LLM 对话。

## 功能概述

本插件为 PilotDesk 工作流贡献一个 LLM 对话节点类型。与 Claude/Hermes/Codex 等固定 Agent 类型不同，本插件复用 PilotDesk agent 模块中已有的 **API 会话**机制——即通过用户在「设置 - API 配置」中添加的供应商（API Provider）和该供应商支持的模型名称来发起对话。

支持 OpenAI 兼容格式和 Anthropic 格式的 API 端点。

## 执行链路

```
工作流编辑器 → 用户配置节点（选供应商、模型、提示词）
  → WorkflowEngine 执行 → PluginExecutor emit("workflow:plugin-execute")
    → 前端 commandDispatcher.execute("llm-chat.chat", params)
      → chatWithLLM(): 通过 api.data.invoke 获取供应商信息和 API Key
        → fetch() 调用 LLM API 端点
          → 解析响应 → 返回 { content, model, provider }
            → respond_plugin_execute 回传结果 → 工作流继续下一节点
```

## 文件结构

```
llm-chat-node/
├── manifest.json   # 插件清单（面板 + 命令 + 节点类型声明）
├── index.js        # 插件入口（面板组件 + 命令 handler + API 调用逻辑）
├── icon.png        # 插件图标（128x128）
└── README.md       # 本文件
```

## 节点配置项

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| API 供应商 | select | 是 | 选择已在「设置 - API 配置」中添加的供应商 |
| 模型名称 | select/input | 是 | 供应商有预置模型时为下拉选择，否则可手动输入 |
| 系统提示词 | textarea | 否 | 设定 AI 角色和行为 |
| 最大 Token 数 | number | 否 | 单次回复的最大 token 数（默认 4096） |
| 温度 | number | 否 | 控制回复随机性（0-2，默认 0.7） |

## 工作流命令参数

命令 `llm-chat.chat` 的输入参数：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| api_provider_id | string | 是 | API 供应商 ID |
| model | string | 是 | 模型名称 |
| prompt | string | 是 | 用户提示词 |
| system_prompt | string | 否 | 系统提示词 |
| max_tokens | integer | 否 | 最大生成 token 数 |
| temperature | number | 否 | 温度参数 |

输出字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| content | string | LLM 回复内容 |
| model | string | 实际使用的模型名称 |
| provider | string | 供应商名称 |

## 使用示例

### 面板预览

1. 插件加载后，在右侧面板中打开「LLM Chat」面板
2. 选择已配置的 API 供应商
3. 选择或输入模型名称
4. 输入提示词和可选的系统提示词
5. 点击「发送」进行对话测试

### 工作流中使用

1. 在工作流编辑器中拖入「插件调用」节点
2. 在节点配置中填写 `plugin_id: llm-chat-node`、`command_id: llm-chat.chat`
3. 在节点 `params` 中设置 API 供应商、模型、提示词等参数
4. 将上游节点的输出连接到本节点
5. 执行工作流时，PluginExecutor 将路由到本插件的命令 handler 完成对话

### 前置条件

- 需要在「设置 - API 配置」中预先添加至少一个 API 供应商并配置 API Key
- 供应商需配置正确的 API 端点地址

## 技术实现

- **API 格式支持**：自动根据端点 URL 推断格式（Anthropic / OpenAI 兼容）
- **供应商管理**：通过 `api.data.invoke('list_api_providers')` 读取已配置的供应商列表
- **密钥获取**：通过 `api.data.invoke('get_api_key', { id })` 安全获取 API Key
- **权限声明**：`ui:panel`（面板）、`ui:toast`（通知）、`data:invoke`（调用 Tauri 命令）
