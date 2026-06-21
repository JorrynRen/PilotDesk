# PilotDesk Agent 会话延续能力设计

> **版本**: v1.0 | **日期**: 2026-06-18 | **状态**: 草案

---

## 一、背景与问题

### 1.1 当前缺陷

当前 PilotDesk 的 Agent 会话存在根本性设计缺陷：**每一条消息对 agent 来说都是独立的、无上下文的单次调用**。

```
消息 1:  claude -p "..."            → 新进程 → 退出
消息 2:  claude -p "..."            → 新进程 → 退出（不记得消息 1）
消息 N:  claude -p "..."            → 新进程 → 退出（不记得之前任何消息）
```

Agent CLI 原生支持会话延续，但 PilotDesk 未利用此能力：

| Agent | 延续命令 | 当前使用 | 正确做法 |
|-------|---------|---------|---------|
| Claude Code | `claude -c -p "..."` 或 `claude -r <session_id> -p "..."` | `claude -p "..."`（无 `-c`） | 首次用 `-p`，后续用 `-c` 或 `-r` |
| Hermes | `hermes chat -q "..." -Q`（无原生延续） | `hermes chat -q "..." -Q` | 需通过 `cwd` 下的缓存文件延续 |
| codeX | `codex exec "..."`（无原生延续） | `codex exec "..."` | 需通过 `cwd` 下的缓存文件延续 |

### 1.2 设计目标

1. **工作目录注入**：首次发送消息时告知 agent 工作目录，工作产物保存在预期位置
2. **Agent 会话 ID 获取**：首次消息后获取 agent 生成的会话 ID，存入 `sessions` 表
3. **会话延续**：后续消息使用 agent 会话 ID 恢复上下文，实现多轮对话
4. **API 会话预留**：API 直连模式不传递工作目录，但生成会话 ID 以备未来上下文扩展

---

## 二、Agent 会话延续机制

### 2.1 各 Agent 的会话延续能力调研

#### Claude Code

Claude Code 原生支持会话延续：

```bash
# 首次对话：在当前目录下创建 .claude/sessions/ 缓存
claude -p "第一次的问题" --verbose --output-format stream-json --dangerously-skip-permissions

# 延续最近一次会话
claude -c -p "后续的问题" --verbose --output-format stream-json --dangerously-skip-permissions

# 恢复指定会话
claude -r <session_id> -p "后续的问题" --verbose --output-format stream-json --dangerously-skip-permissions
```

- 会话缓存存储在 `{cwd}/.claude/sessions/` 目录下
- `claude -c` 自动恢复当前目录下最近一次会话
- `claude -r <id>` 恢复指定 ID 的会话
- 可通过解析 `claude -c` 首次输出获取 session ID

#### Hermes Agent

Hermes 的 `chat` 模式每次是独立查询，但会在 `{cwd}/.hermes/` 下生成缓存。需要进一步调研是否支持 `--resume` 参数。

#### codeX

codeX 的 `exec` 模式每次是独立执行，无原生会话延续能力。

### 2.2 通用方案

对于不支持原生会话延续的 Agent（Hermes、codeX），采用**进程保活**方案：

```
消息 1: 启动 agent 子进程 → 保持进程存活 → 读取输出
消息 2: 向同一进程 stdin 写入 → 读取输出
...
关闭会话: 终止进程
```

但此方案复杂度高（进程保活、状态管理、超时处理），且与当前每次启动新进程的架构差异大。**建议先实现 Claude Code 的原生会话延续**，其他 Agent 后续跟进。

---

## 三、详细设计

### 3.1 数据库变更

`sessions` 表新增字段：

```sql
ALTER TABLE sessions ADD COLUMN agent_session_id TEXT DEFAULT '';
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `agent_session_id` | TEXT | Agent 侧的会话 ID，如 Claude Code 的 `.claude/sessions/` 中的文件名（不含扩展名） |

### 3.2 会话生命周期

```
┌────────────────────────────────────────────────────────────┐
│                     会话生命周期                              │
│                                                             │
│  create_session()                                            │
│    │                                                         │
│    ├── agent_type = 'api'                                    │
│    │   └── 生成 UUID，存入 sessions 表，agent_session_id = '' │
│    │       └── 后续：前端维护消息列表，API 上下文能力标记为待完成 │
│    │                                                         │
│    ├── agent_type = 'claude'                                 │
│    │   ├── 首次消息（agent_session_id == ''）                  │
│    │   │   ├── 启动 claude -p "..."（带 cwd）                  │
│    │   │   ├── 解析输出获取 session ID                        │
│    │   │   ├── 更新 sessions 表 agent_session_id              │
│    │   │   └── 返回消息内容                                   │
│    │   │                                                      │
│    │   └── 后续消息（agent_session_id != ''）                  │
│    │       ├── 启动 claude -r <agent_session_id> -p "..."     │
│    │       └── 返回消息内容                                   │
│    │                                                         │
│    ├── agent_type = 'hermes'（待实现）                        │
│    │   └── ...                                               │
│    │                                                         │
│    └── agent_type = 'codex'（待实现）                         │
│        └── ...                                               │
│                                                             │
│  close_session()                                             │
│    └── 清理资源                                              │
└────────────────────────────────────────────────────────────┘
```

### 3.3 Claude Code 会话延续实现

#### 首次消息

```rust
// 当前 build_args:
//   claude -p --verbose --output-format stream-json --dangerously-skip-permissions {message}

// 首次消息：无变化，但需要从输出中解析 session ID
// Claude Code 首次输出中会包含类似：
//   Session: abc123 (./.claude/sessions/abc123.json)
// 需要从 stdout 中提取 "abc123" 作为 agent_session_id
```

需要在 `parse_claude_output` 之外增加一个**元数据解析通道**，从 stdout 中提取 session ID 信息。Claude Code 的 JSON stream 输出中可能包含 `{"type": "session", "session_id": "..."}` 类型的事件。

#### 后续消息

```rust
fn build_args(&self, message: &str, mode: &str, system_prompt: Option<&str>, agent_session_id: Option<&str>) -> Vec<String> {
    match self {
        Self::Claude => {
            let mut args = vec![
                "--verbose".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--dangerously-skip-permissions".into(),
            ];
            if let Some(sid) = agent_session_id {
                args.push("-r".into());
                args.push(sid.into());
            } else {
                args.push("-p".into());
            }
            args.push(message.into());
            args
        }
        // ...
    }
}
```

#### 输出解析扩展

```rust
// 新增：从 Claude Code 输出中提取 session ID
fn extract_claude_session_id(line: &str) -> Option<String> {
    if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
        if event["type"] == "session_info" {
            return event["session_id"].as_str().map(|s| s.to_string());
        }
    }
    None
}
```

### 3.4 前端变更

#### 消息发送流程

```
MainPanel.sendMessage()
  │
  ├── 获取当前 session 的 agent_session_id
  │
  ├── agent 会话（claude/hermes/codex）
  │   └── invoke('agent_send_message', {
  │         sessionId,
  │         agentType,
  │         message,
  │         mode,
  │         cwd,
  │         agentSessionId,  // 新增：首次为空，后续为 agent 侧 ID
  │       })
  │
  └── API 会话
      └── sendApiChat()  // 不变，上下文能力标记为待完成
```

#### 接收消息流程

```
agent-done 事件
  │
  ├── 检查事件中是否包含 agent_session_id
  │   └── 有 → 调用 update_session_agent_id(sessionId, agentSessionId)
  │
  └── 正常消息渲染
```

### 3.5 Rust 后端变更

#### AgentManager

```rust
pub struct AgentManager {
    processes: HashMap<String, AgentProcess>,
}

pub struct AgentSession {
    agent_session_id: Option<String>,  // 新增：存储 agent 侧会话 ID
}

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
    // 从输出中提取 agent_session_id
    // 通过 agent-session 事件回传给前端
}
```

#### 新增 Tauri 命令

```rust
#[tauri::command]
pub async fn update_session_agent_id(
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

---

## 四、分阶段实施

### Phase 1 — Claude Code 会话延续

**工作量**：约 6-8 小时

| 任务 | 说明 |
|------|------|
| `sessions` 表新增 `agent_session_id` 字段 | 数据库迁移 |
| `build_args` 增加 `agent_session_id` 参数 | 首次无 ID 用 `-p`，后续有 ID 用 `-r <id>` |
| 输出解析增加 session ID 提取 | 从 Claude Code JSON stream 中解析 session_info 事件 |
| 新增 `update_session_agent_id` Tauri 命令 | 前端收到 agent_session_id 后更新数据库 |
| 前端 `sendChat` 增加 `agentSessionId` 参数 | 从 session 对象中读取并传递 |
| 前端 `agent-done` 处理中增加 agent_session_id 提取 | 收到后调用 `update_session_agent_id` |

### Phase 2 — Hermes / codeX 会话延续调研

**工作量**：约 4-6 小时

| 任务 | 说明 |
|------|------|
| 调研 Hermes 是否支持 `--resume` 参数 | 阅读 Hermes CLI 文档 |
| 调研 codeX 是否支持会话延续 | 阅读 codeX CLI 文档 |
| 如不支持，评估进程保活方案可行性 | 进程保活 vs 每次重建上下文 |

### Phase 3 — API 会话上下文能力

**工作量**：标记为待完成

| 任务 | 说明 |
|------|------|
| API 会话生成会话 ID | 已有（PilotDesk 的 session.id） |
| 前端维护消息历史 | 已有 |
| 发送消息时携带历史上下文 | **待实现**：将前端消息列表中的历史消息作为 `messages` 参数传入 API 请求 |

---

## 五、影响范围

### 5.1 需要修改的文件

| 文件 | 修改内容 |
|------|---------|
| `src-tauri/src/agent/mod.rs` | `build_args` 增加 `agent_session_id` 参数；新增 session ID 提取逻辑 |
| `src-tauri/src/commands/session.rs` | `create_session` 初始化 `agent_session_id`；新增 `update_session_agent_id` 命令 |
| `src-tauri/src/db/init.rs` | 新增迁移：`ALTER TABLE sessions ADD COLUMN agent_session_id TEXT DEFAULT ''` |
| `src-tauri/src/lib.rs` | 注册 `update_session_agent_id` 命令 |
| `src/types/index.ts` | `Session` 类型增加 `agentSessionId` 字段 |
| `src/hooks/useAgentEvent.ts` | `sendChat` 增加 `agentSessionId` 参数 |
| `src/stores/sessionStore.ts` | `createSession` / 消息处理中处理 `agentSessionId` |
| `src/components/layout/MainPanel.tsx` | 发送消息时传递 `agentSessionId` |

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

| 风险 | 等级 | 缓解措施 |
|------|------|---------|
| Claude Code `-r` 参数行为不确定 | 中 | Phase 1 前需实际测试 `claude -r <id> -p "..."` 的行为 |
| Claude Code session ID 提取方式不确定 | 中 | 需实际测试 JSON stream 输出中是否包含 session 信息；如不包含，可改为从文件系统扫描 `.claude/sessions/` 目录 |
| `-r` 指定的 session 不存在时 Claude Code 行为 | 低 | 回退为 `-p`（无延续），前端捕获错误后重新创建会话 |
| 多个会话共享同一 `cwd` 时 session 冲突 | 中 | Claude Code 的 `-r` 指定具体 ID 可避免冲突；`-c` 只能恢复最近一次，不适合多会话场景 |
| Hermes / codeX 不支持会话延续 | 中 | 进程保活方案复杂度高，可能需重新架构 |

---

## 七、附录

### 7.1 Claude Code CLI 相关命令参考

```bash
# 交互式启动
claude

# 非交互式单次查询
claude -p "问题"

# 延续最近会话
claude -c -p "后续问题"

# 恢复指定会话
claude -r <session_id> -p "后续问题"

# 会话存储位置
# {cwd}/.claude/sessions/<session_id>.json
```

### 7.2 术语表

| 术语 | 说明 |
|------|------|
| agent_session_id | Agent 侧生成的会话 ID，与 PilotDesk 的 session.id 不同 |
| 会话延续 | 后续消息能引用前文上下文的能力 |
| 进程保活 | 保持 agent 子进程运行，通过 stdin/stdout 持续交互 |
| 上下文能力 | API 会话中携带历史消息列表的能力 |
