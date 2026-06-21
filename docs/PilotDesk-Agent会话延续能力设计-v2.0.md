# PilotDesk Agent 会话延续能力设计

> **版本**: v2.1 | **日期**: 2026-06-21 | **状态**: 已实施（消息参数注入修复：-- 分隔符 / = 语法 / 原子参数三种模式通用处理）

---

## 一、背景与问题

### 1.1 当前缺陷

当前 PilotDesk 的 Agent 会话存在根本性设计缺陷：**每一条消息对 agent 来说都是独立的、无上下文的单次调用**。

```
消息 1:  claude --prompt="..."            → 新进程 → 退出
消息 2:  claude --prompt="..."            → 新进程 → 退出（不记得消息 1）
消息 N:  claude --prompt="..."            → 新进程 → 退出（不记得之前任何消息）
```

Agent CLI 原生支持会话延续，但 PilotDesk 未利用此能力。

### 1.2 设计目标

1. **工作目录注入**：首次发送消息时告知 agent 工作目录，工作产物保存在预期位置
2. **Agent 会话 ID 获取**：首次消息后获取 agent 生成的会话 ID，存入 `sessions` 表
3. **会话延续**：后续消息使用 agent 会话 ID 恢复上下文，实现多轮对话
4. **API 会话预留**：API 直连模式不传递工作目录，但生成会话 ID 以备未来上下文扩展

---

## 二、Agent 会话延续机制

### 2.1 各 Agent 的会话延续能力调研与验证

#### Claude Code

Claude Code 原生支持会话延续：

```bash
# 首次对话
claude --prompt="第一次的问题" --output-format stream-json --verbose --dangerously-skip-permissions

# 恢复指定会话（已验证通过）
claude --resume <session_id> --prompt="后续的问题" --output-format stream-json --verbose --dangerously-skip-permissions
```

**session_id 提取**：从 stdout JSON stream 的第一个事件中提取：
```json
{"type": "system", "subtype": "init", "session_id": "bb3097dd-8545-4c1b-862f-e00e309e30d6"}
```

> **注意**：`-- {message}` 使用 `--` 分隔符，yargs（Node.js CLI 框架）停止解析标志，后续内容均为位置参数。

**测试结果**：
- 首次消息："记住这个数字：42" → 回复"已记住"，提取 `session_id: 3fb2e1c3-9b33-4047-b8d8-863b2b5f4737`
- 恢复消息（`--resume <session_id>`）："我刚才让你记住的数字是什么？" → 回复"42"
- **结论：会话延续有效**

#### Hermes Agent

Hermes 支持 `--resume` 参数恢复会话：

```bash
# 首次对话
hermes chat --query="第一次的问题" -Q

# 恢复指定会话（已验证通过）
hermes chat --resume <session_id> --query="后续的问题" -Q
```

**session_id 提取**：从 stderr 文本行中提取：
```
session_id: 20260619_023802_571ee5
```

> **注意**：`--query={message}` 使用 `=` 语法，argparse 将 `=` 后的内容强制作为 `--query` 的值，即使以 `--` 开头也不会被解释为标志。

**测试结果**：
- 首次消息："记住这个数字：42" → 回复"已记住"，提取 `session_id: 20260619_023802_571ee5`
- 恢复消息（`--resume <session_id>`）："我刚才让你记住的数字是什么？" → 回复"42"
- stderr 还输出确认信息：`↻ Resumed session 20260619_023802_571ee5 (1 user message, 2 total messages)`
- **结论：会话延续有效**

#### Codex CLI

Codex 支持 `exec resume` 子命令恢复会话：

```bash
# 首次对话
codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- "第一次的问题"

# 恢复指定会话（已验证通过）
codex exec resume --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox <session_id> -- "后续的问题"
```

**session_id 提取**：从 stdout JSONL 的 `thread.started` 事件中提取：
```json
{"type":"thread.started","thread_id":"019edc15-1535-7951-8b86-dc6042445b66"}
```

> **注意**：`-- {message}` 使用 `--` 分隔符，告诉 argparse 停止解析标志，后续内容均为位置参数。

**文本提取**：从 `item.completed` 事件中提取：
```json
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"已记住数字 42。"}}
```

**测试结果**：
- 首次消息："记住这个数字：42" → 回复"已记住数字42。"，提取 `thread_id: 019edc15-b316-7262-9aaf-40236e627ee6`
- 恢复消息（`exec resume <session_id>`）："我刚才让你记住的数字是什么？" → 回复"42。"
- **结论：会话延续有效**

### 2.2 方案演进历程

会话延续的实现经历了多次方案迭代：

| 方案 | 尝试 | 结果 |
|------|------|------|
| `--session-id <uuid>` | 主动分配 UUID 给 agent | 失败：需配合 `--resume/--continue + --fork-session` |
| `--resume <uuid> --fork-session` | 分叉一个不存在的会话 | 失败：`No conversation found with session ID` |
| `--continue` | 恢复最近会话 | 失败：同目录多会话冲突 |
| 纯 `-p`（放弃会话延续） | 每次都新建会话 | 临时方案，放弃上下文 |
| **从 JSON stream 提取 session_id + `--resume`** | 首次从输出提取，后续恢复 | **成功（最终方案）** |

### 2.3 三种 Agent 的差异对比

| 维度 | Claude Code | Hermes Agent | Codex CLI |
|------|------------|-------------|-----------|
| session_id 来源 | stdout JSON stream | stderr 文本行 | stdout JSONL |
| 提取事件 | `type=system,subtype=init` | `session_id: xxxxx` | `type=thread.started` |
| session_id 字段 | `event["session_id"]` | `line.strip_prefix("session_id: ")` | `event["thread_id"]` |
| 恢复参数 | `--resume <uuid>`（插入到 `--prompt=` 之前） | `--resume <id>`（插入到 `--query=` 之前） | `exec resume <uuid>`（插入到 `--` 之后） |
| 输出格式 | `stream-json` | `-Q` 安静模式 | `--json` JSONL |
| 文本提取 | `type=assistant -> message.content[].text` | strip ANSI + 过滤非内容行 | `type=item.completed -> item.text` |
| 额外标志 | `--verbose`（stream-json 必需） | 无 | `--skip-git-repo-check` `--dangerously-bypass-approvals-and-sandbox` |

---

## 三、详细设计

### 3.1 工作目录体系

#### 三层兜底逻辑

工作目录采用三层兜底策略，确保 Agent 始终在有效目录下运行：

```
创建会话时（SessionList.validateAndEnsureDir）
  +-- 用户输入了路径 -> 校验 -> ensure_dir（不存在则自动创建）
  +-- 用户未输入（空） -> 读取全局设置 pilotdesk-workspace
  |     +-- 有设置 -> ensure_dir
  |     +-- 无设置 -> 返回 null（回退到 Rust 端 current_dir()）
  +-- 校验失败 -> 提示错误，阻止创建

发送消息时（MainPanel.handleSend）
  +-- currentSession.cwd || undefined -> Rust 端
        +-- 有值 -> child.current_dir(cwd)
        +-- 无值 -> child.current_dir(current_dir())
```

#### 全局默认工作区

- **默认值**：`~\AppData\Roaming\PilotDesk`（`~` 展开为用户主目录）
- **存储位置**：`app_settings` 表，key = `pilotdesk-workspace`
- **种子数据**：首次启动时自动写入
- **用户可修改**：设置页 - 工作区目录

#### 路径校验规则（ensure_dir）

| 规则 | 说明 |
|------|------|
| 空值 | 拒绝，返回"路径不能为空" |
| `~` 前缀 | 展开为用户主目录（如 `C:\Users\Administrator`） |
| 非法字符 | 拒绝 `< > " \| ? *` |
| 裸盘符 | 拒绝 `C:`、`D:` 等不完整路径 |
| 无效盘符 | 拒绝非 A-Z 的盘符 |
| 已存在路径 | 必须是目录，不能是文件 |
| 不存在路径 | 自动创建（含父目录） |

#### 前端校验（仅浏览选择）

- 工作区目录为**只读展示**，用户无法手动输入
- 仅通过"浏览"按钮调用系统文件夹选择器（`open({ directory: true })`）选择目录
- 选择后直接保存，路径由系统保证合法，无需额外校验
- 创建会话对话框中的工作目录同样为只读展示，仅可通过浏览按钮选择

### 3.2 数据库变更

`sessions` 表新增字段（migration v7）：

```sql
ALTER TABLE sessions ADD COLUMN agent_session_id TEXT DEFAULT NULL;
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `agent_session_id` | TEXT | Agent 侧的会话 ID，如 Claude Code 的 UUID、Hermes 的时间戳 ID、Codex 的 thread_id |

### 3.3 会话生命周期

```
+--------------------------------------------------------------------------------+
|                     会话生命周期（已实施）                                         |
|                                                                                 |
|  create_session()                                                                |
|    |                                                                            |
|    +-- agent_type = 'api'                                                       |
|    |   +-- 生成 UUID，存入 sessions 表，agent_session_id = null                  |
|    |       +-- 后续：前端维护消息列表，API 上下文能力标记为待完成                   |
|    |                                                                            |
|    +-- agent_type = 'claude' / 'hermes' / 'codex'                               |
|        +-- 首次消息（agent_session_id == null）                                   |
|        |   +-- build_args 不带 --resume，agent 创建新会话                         |
|        |   +-- stdout/stderr 解析提取 session_id                                 |
|        |   +-- 发射 agent-session 事件到前端                                     |
|        |   +-- 前端调用 update_session_agent_id 存入数据库                        |
|        |   +-- 消息正常渲染                                                      |
|        |                                                                        |
|        +-- 后续消息（agent_session_id != null）                                   |
|            +-- build_args 附加 --resume/exec resume <session_id>                 |
|            +-- agent 恢复上下文，返回有记忆的响应                                   |
|            +-- 消息正常渲染                                                      |
|                                                                                 |
|  close_session()                                                                 |
|    +-- 清理进程资源                                                              |
+---------------------------------------------------------------------------------+
```

### 3.4 Rust 后端实现

#### build_args（agent/mod.rs）

```rust
// 当前实现已迁移至 handler.rs 的 ProcessHandler trait
// 通过 run_cmd_template 配置驱动，无需硬编码 build_args
//
// 各 Agent 模板（定义在 init.rs 种子数据 + migration v17/v18）：
//
// Claude:  claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}
// Hermes:  hermes chat --query={message} -Q
// Codex:   codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- {message}
//
// handler.rs 的 build_command() 方法通用处理逻辑：
// 1. splitn(2, "{message}") 拆分为前缀和后缀
// 2. 检测前缀最后一个参数是否以 = 结尾
//    - 是（Claude/Hermes）：= 语法，消息与 = 参数拼接为 "--key=value"
//    - 否（Codex）：消息作为独立参数 push
// 3. resume 参数插入到 = 参数之前，确保 --query=--help 的原子性
```

#### session_id 提取（stdout 循环）

```rust
// 统一在 stdout 读取循环中解析 JSON 事件
if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
    // Claude Code: system/init event
    if agent_type_name == "claude" && event["type"] == "system" && event["subtype"] == "init" {
        if let Some(sid_agent) = event["session_id"].as_str() {
            let _ = app_clone.emit("agent-session", serde_json::json!({
                "sessionId": sid,
                "agentSessionId": sid_agent,
            }));
        }
    }
    // Codex: thread.started event
    if agent_type_name == "codex" && event["type"] == "thread.started" {
        if let Some(sid_agent) = event["thread_id"].as_str() {
            let _ = app_clone.emit("agent-session", serde_json::json!({
                "sessionId": sid,
                "agentSessionId": sid_agent,
            }));
        }
    }
}
```

#### session_id 提取（stderr 循环 — Hermes）

```rust
// 后台收集 stderr 的任务中提取 Hermes session_id
if agent_type_name_for_stderr == "hermes" {
    if let Some(sid_agent) = line.strip_prefix("session_id: ") {
        let _ = app_clone_for_stderr.emit("agent-session", serde_json::json!({
            "sessionId": sid_for_stderr,
            "agentSessionId": sid_agent.trim(),
        }));
    }
}
```

#### send_message 参数传递

```rust
pub async fn send_message(
    &mut self,
    app_handle: tauri::AppHandle,
    session_id: String,
    agent_type: String,
    message: String,
    mode: String,
    cwd: Option<String>,
    system_prompt: Option<String>,
    agent_session_id: Option<String>,  // 新增参数
) -> Result<(), String> {
    // ...
    let args = agent.build_args(&message, &mode, system_prompt.as_deref(), agent_session_id.as_deref());
    // ...
}
```

#### 新增 Tauri 命令

```rust
#[tauri::command]
pub fn update_session_agent_id(
    state: State<'_, DbState>,
    session_id: String,
    agent_session_id: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    conn.execute(
        "UPDATE sessions SET agent_session_id = ?1 WHERE id = ?2",
        params![agent_session_id, session_id],
    )?;
    Ok(())
}
```

### 3.5 前端实现

#### useAgentEvent.ts

```typescript
// AgentEventHandlers 新增 onSession 回调
export interface AgentEventHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSession?: (sessionId: string, agentSessionId: string) => void;  // 新增
  onSkills?: (agentType: string, skills: Array<{...}>) => void;
}

// 注册 agent-session 事件监听
const sessionUnlisten = await listen<{ sessionId: string; agentSessionId: string }>(
  'agent-session', (event) => {
    handlersRef.current?.onSession?.(event.payload.sessionId, event.payload.agentSessionId);
  }
);

// sendChat 增加 agentSessionId 参数
const sendChat = useCallback(
  async (sessionId, message, mode, agentType, cwd, systemPrompt, agentSessionId?) => {
    await invoke('agent_send_message', {
      sessionId, agentType, message, mode, cwd, systemPrompt,
      agentSessionId: agentSessionId || null,
    });
  },
  [],
);
```

#### MainPanel.tsx

```typescript
// onSession 回调：保存 agent_session_id 到数据库
const onSession = useCallback((sessionId: string, agentSessionId: string) => {
  invoke('update_session_agent_id', { sessionId, agentSessionId }).catch(console.error);
  useSessionStore.getState().fetchSessions();
}, []);

// handleSend 传递 agentSessionId
const agentSessionId = currentSession.agentSessionId || undefined;
sendChat(sid, message, mode, currentSession.agentType, undefined, systemPrompt, agentSessionId);
```

---

## 四、分阶段实施

### Phase 1 — Claude Code 会话延续（已完成）

| 任务 | 说明 | 状态 |
|------|------|------|
| `sessions` 表新增 `agent_session_id` 字段 | migration v7 | ✅ 已完成 |
| `build_args` 增加 `agent_session_id` 参数 | 首次无 ID 用 `-p`，后续有 ID 用 `--resume <id>` | ✅ 已完成 |
| stdout 解析增加 session ID 提取 | 从 `system/init` 事件提取 | ✅ 已完成 |
| 新增 `update_session_agent_id` Tauri 命令 | 前端收到 agent_session_id 后更新数据库 | ✅ 已完成 |
| 前端 `sendChat` 增加 `agentSessionId` 参数 | 从 session 对象中读取并传递 | ✅ 已完成 |
| 前端 `agent-session` 事件处理 | 收到后调用 `update_session_agent_id` | ✅ 已完成 |

### Phase 2 — Hermes / Codex 会话延续（已完成）

| 任务 | 说明 | 状态 |
|------|------|------|
| 调研 Hermes 是否支持 `--resume` 参数 | 测试验证通过 | ✅ 已完成 |
| 调研 Codex 是否支持会话延续 | `exec resume` 测试验证通过 | ✅ 已完成 |
| Hermes build_args 增加 `--resume` 支持 | stderr 提取 session_id | ✅ 已完成 |
| Codex build_args 改为 `--json` 模式 + `exec resume` | stdout JSONL 提取 thread_id | ✅ 已完成 |
| parse_codex_output 改为解析 JSONL `item.completed` | 从 JSON 中提取文本 | ✅ 已完成 |

### Phase 3 — API 会话上下文能力（待实现）

| 任务 | 说明 | 状态 |
|------|------|------|
| API 会话生成会话 ID | 已有（PilotDesk 的 session.id） | ✅ 已有 |
| 前端维护消息历史 | 已有 | ✅ 已有 |
| 发送消息时携带历史上下文 | 将前端消息列表中的历史消息作为 `messages` 参数传入 API 请求 | ⏳ 待实现 |

---

## 五、影响范围

### 5.1 实际修改的文件

| 文件 | 修改内容 |
|------|---------|
| `src-tauri/src/agent/mod.rs` | `build_args` 增加 `agent_session_id` 参数；Claude stdout 提取 session_id；Codex stdout JSONL 提取 thread_id；Hermes stderr 提取 session_id；`parse_codex_output` 改为 JSONL 解析；`send_message` 增加 `agent_session_id` 参数 |
| `src-tauri/src/commands/session.rs` | 所有查询包含 `agent_session_id` 列；`create_session` 返回 `agent_session_id: None`；新增 `update_session_agent_id` 命令 |
| `src-tauri/src/db/init.rs` | migration v7：`ALTER TABLE sessions ADD COLUMN agent_session_id` |
| `src-tauri/src/db/models.rs` | `Session` 结构体新增 `agent_session_id: Option<String>` |
| `src-tauri/src/lib.rs` | `agent_send_message` 传递 `agent_session_id`；注册 `update_session_agent_id` |
| `src/types/index.ts` | `Session` 接口新增 `agentSessionId?: string` |
| `src/hooks/useAgentEvent.ts` | 新增 `onSession` 处理器、`agent-session` 事件监听、`sendChat` 支持 `agentSessionId` 参数 |
| `src/components/layout/MainPanel.tsx` | 新增 `onSession` 回调；`handleSend` 传递 `agentSessionId` |
| `src/styles/globals.css` | 新增 `@keyframes pd-breathe` 呼吸脉冲动画 |
| `src/components/common/AgentBadge.tsx` | 等待动画改为呼吸脉冲（无发光效果） |
| `src/pages/SettingsPage.tsx` | 工作区目录改为只读展示 + 仅浏览选择；移除 `handleWorkspaceChange` 输入校验逻辑；清理未使用的 `SettingsInput` 导入 |
| `src/components/layout/SessionList.tsx` | 创建会话对话框工作目录改为只读展示 + 仅浏览选择；移除 `handleCwdInputChange` 输入校验逻辑 |

### 5.2 不受影响的模块

| 模块 | 原因 |
|------|------|
| API 会话（sendApiChat） | 上下文能力标记为待完成，当前行为不变 |
| 环境检测 | 无关联 |
| 插件系统 | 无关联 |
| 灵感市集 | 无关联 |
| 技能系统 | 无关联 |

---

## 六、风险与注意事项

| 风险 | 等级 | 状态 | 缓解措施 |
|------|------|------|---------|
| Claude Code `--resume` 参数行为不确定 | 中 | ✅ 已解决 | 实际测试验证通过 |
| Claude Code session ID 提取方式不确定 | 中 | ✅ 已解决 | JSON stream 第一个事件包含 `session_id` |
| `--resume` 指定的 session 不存在时行为 | 低 | ✅ 已解决 | 首次消息无 ID 用 `-p`，后续有 ID 才用 `--resume` |
| 多个会话共享同一 `cwd` 时 session 冲突 | 中 | ✅ 已解决 | `--resume` 指定具体 ID 可避免冲突 |
| Hermes 不支持会话延续 | 中 | ✅ 已解决 | Hermes 支持 `--resume`，stderr 输出 session_id |
| Codex 不支持会话延续 | 中 | ✅ 已解决 | Codex 支持 `exec resume`，JSONL 输出 thread_id |
| Codex 需要 `--skip-git-repo-check` 标志 | 低 | ✅ 已解决 | 已加入 build_args |
| Codex 需要 `--dangerously-bypass-approvals-and-sandbox` 标志 | 低 | ✅ 已解决 | 已加入 build_args |

---

## 七、附录

### 7.1 各 Agent CLI 命令参考

#### Claude Code

```bash
# 首次对话（非交互式 + JSON stream）
claude --prompt="问题" --output-format stream-json --verbose --dangerously-skip-permissions

# 恢复指定会话
claude --resume <session_id> --prompt="问题" --output-format stream-json --verbose --dangerously-skip-permissions
```

#### Hermes Agent

```bash
# 首次对话（安静模式）
hermes chat --query="问题" -Q

# 恢复指定会话
hermes chat --resume <session_id> --query="问题" -Q
```

#### Codex CLI

```bash
# 首次对话（JSONL 模式 + 非交互式）
codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- "问题"

# 恢复指定会话
codex exec resume --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox <session_id> -- "问题"
```

### 7.2 测试验证脚本

测试脚本位于 `_test-session-continuity.js`，验证流程：
1. 发送第一条消息"记住这个数字：42"
2. 从输出中提取 session_id
3. 发送第二条消息"我刚才让你记住的数字是什么？"并传入 `--resume`/`exec resume`
4. 验证第二条响应包含"42"

### 7.3 术语表

| 术语 | 说明 |
|------|------|
| agent_session_id | Agent 侧生成的会话 ID，与 PilotDesk 的 session.id 不同 |
| 会话延续 | 后续消息能引用前文上下文的能力 |
| JSON stream | Claude Code 的流式 JSON 输出格式 |
| JSONL | Codex 的 JSON 行格式，每行一个 JSON 对象 |
| 安静模式 | Hermes 的 `-Q` 标志，抑制横幅和旋转器 |
| 上下文能力 | API 会话中携带历史消息列表的能力 |


### 7.4 消息参数注入修复探索记录

修复 `--help` 崩溃问题经历了多轮探索验证：

| 轮次 | 方案 | 验证结果 |
|------|------|---------|
| 1 | 双引号包裹 `"{message}"` | `simple_split` 剥离引号，`--help` 仍是裸参数 ❌ |
| 2 | `splitn` 拆分 + 原子参数 `push` | argparse 仍拦截 `--help` ❌ |
| 3 | `--query={message}` 长选项 `=` 语法 | Hermes argparse 通过 ✅ |
| 4 | `-p={message}` 短选项 `=` 语法 | yargs 不支持，报错 `-=--help` ❌ |
| 5 | `--prompt={message}` 长选项 `=` 语法 | yargs 也不支持 ❌ |
| 6 | `-- {message}` 分隔符方案 | yargs 和 argparse 均通过 ✅ |

**最终模板**：

| Agent | 模板 | 安全机制 |
|-------|------|---------|
| Claude | `-p ... --dangerously-skip-permissions -- {message}` | `--` 分隔符 |
| Hermes | `--query={message} -Q` | `=` 语法 |
| Codex | `--json ... --dangerously-bypass-approvals-and-sandbox -- {message}` | `--` 分隔符 |

**handler.rs 通用逻辑**：检测 `--` 分隔符 → resume 插入到 `--` 之前；检测 `=` 结尾 → resume 插入到 `=` 之前。