# PilotDesk Agent 统一管理平台设计

> **版本**: v1.0 | **日期**: 2026-06-18 | **状态**: 草案

---

## 一、背景与目标

### 1.1 现状问题

当前 PilotDesk 对 Agent 的支持采用**硬编码**方式，Agent 信息散落在 **4 个 Rust 文件 + 18 个前端文件** 中：

| 层面 | 硬编码点 | 涉及文件 |
|------|---------|---------|
| Rust 后端 | `AGENTS` 配置表 | `commands/env.rs` |
| Rust 后端 | 独立安装命令（`install_claude_code` 等 3 个） | `commands/env.rs` |
| Rust 后端 | 命令注册 | `lib.rs` |
| Rust 后端 | 数据库 CHECK 约束 | `db/init.rs` |
| Rust 后端 | `AgentType` 枚举 + `build_args` + `parse_output_line` | `agent/mod.rs` |
| 前端类型 | `AGENT_THEMES` 对象（5 个条目） | `types/index.ts` |
| 前端组件 | `EnvManager` 每个 agent 的安装/检测/更新逻辑 | `components/env/EnvManager.tsx` |
| 前端组件 | `SessionList` 新建会话下拉 | `components/layout/SessionList.tsx` |
| 前端组件 | `useAgentEvent` agent 类型分发 | `hooks/useAgentEvent.ts` |
| 前端组件 | 各组件中 `agentType === 'claude'` 等条件判断 | ~20 处 |

新增或移除一个 Agent 需要修改约 **20+ 处代码**，作为 Agent 统一管理平台的定位，极不科学。

### 1.2 设计目标

1. **用户级管理**：用户可通过界面新增、移除、启用/禁用 Agent，无需修改代码
2. **元信息驱动**：后端和前端通过统一的元数据查询接口驱动运行
3. **渐进式迁移**：分阶段实施，每阶段可独立交付
4. **向后兼容**：现有数据库和用户数据无损迁移

### 1.3 设计边界

> PilotDesk 的假设前提是 Agent 已正确安装和配置。Agent 自身的认证（API Key 等）、模型选择、对话能力属于 Agent 端管理范畴，不在本软件考虑范围内。

---

## 二、Agent 元信息模型

### 2.1 核心元信息定义

所有 Agent 元信息存储在数据库 `agents` 表中，预置数据作为种子，用户可自定义新增。

```sql
CREATE TABLE IF NOT EXISTS agents (
    -- 标识
    agent_id        TEXT PRIMARY KEY,          -- 唯一标识符，如 "claude", "hermes", "custom-agent"
    name            TEXT NOT NULL,             -- 显示名称，如 "Claude Code", "Hermes Agent"
    description     TEXT NOT NULL DEFAULT '',  -- 简短描述

    -- 生命周期命令（直接存储可执行的 shell 命令，后端零推导）
    install_cmd         TEXT NOT NULL DEFAULT '',  -- 安装命令，如 "npm install -g @anthropic-ai/claude-code"
    uninstall_cmd       TEXT NOT NULL DEFAULT '',  -- 卸载命令，如 "npm uninstall -g @anthropic-ai/claude-code"
    update_cmd          TEXT NOT NULL DEFAULT '',  -- 更新命令，如 "npm update -g @anthropic-ai/claude-code"
    version_cmd         TEXT NOT NULL DEFAULT '',  -- 版本检测命令，如 "claude --version"
    latest_version_cmd  TEXT NOT NULL DEFAULT '',  -- 最新版本查询命令，如 "npm view @anthropic-ai/claude-code version"

    -- 进程交互
    run_cmd_template    TEXT NOT NULL DEFAULT '',  -- 启动命令模板，{message} 占位符替换
    output_parser       TEXT NOT NULL DEFAULT 'raw-text'  -- 输出解析器类型
                        CHECK(output_parser IN ('json-stream', 'ansi-text', 'raw-text')),
    output_filter_regex TEXT NOT NULL DEFAULT '',  -- 噪声行过滤正则
    version_pattern     TEXT NOT NULL DEFAULT 'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)',  -- 版本号提取正则

    -- 显示
    color       TEXT NOT NULL DEFAULT '#6366F1',
    icon        TEXT NOT NULL DEFAULT '🤖',
    sort_order  INTEGER NOT NULL DEFAULT 0,

    -- 状态
    enabled     INTEGER NOT NULL DEFAULT 1,
    is_builtin  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
```

### 2.2 字段说明

| 字段 | 必要性 | 说明 |
|------|--------|------|
| `agent_id` | 核心 | 唯一标识，预置 Agent 使用固定 ID（`claude`/`hermes`/`codex`），自定义 Agent 由用户指定 |
| `name` | 核心 | 用户可见的显示名称 |
| `description` | 建议 | 帮助用户了解 Agent 用途 |
| `install_cmd` | 核心 | 安装命令，直接可执行的 shell 命令 |
| `uninstall_cmd` | 核心 | 卸载命令 |
| `update_cmd` | 核心 | 更新命令 |
| `version_cmd` | 核心 | 版本检测命令 |
| `latest_version_cmd` | 核心 | 最新版本查询命令 |
| `run_cmd_template` | 核心 | 完整启动命令模板（含 CLI 命令名），`{message}` 占位符在运行时替换为用户消息 |
| `output_parser` | 核心 | 输出解析器类型：`json-stream` / `ansi-text` / `raw-text` |
| `output_filter_regex` | 建议 | 过滤噪声行的正则（匹配的行跳过不显示） |
| `version_pattern` | 核心 | 从 `version_cmd` 输出中提取版本号的正则 |
| `color` | 建议 | UI 主题色，用于 AgentBadge、消息气泡等 |
| `icon` | 建议 | UI 图标 |
| `sort_order` | 建议 | 排序优先级，决定 UI 中的展示顺序 |
| `enabled` | 核心 | 用户可禁用不需要的 Agent，禁用的 Agent 不参与环境检测和会话创建 |
| `is_builtin` | 核心 | 预置 Agent 不可删除，但可禁用 |
| `created_at`/`updated_at` | 建议 | 审计 |

### 2.3 不纳入元信息的字段

以下字段**不属于** Agent 元信息：

| 字段 | 原因 |
|------|------|
| `installed_version` | 运行时状态，由 `version_cmd` 检测获取，可内存缓存 |
| `latest_version` | 运行时状态，由 `latest_version_cmd` 查询获取，可内存缓存 |
| `auth_type` / `auth_config` | Agent 自身管理范畴 |
| `supports_model_selection` / `model_list_url` | Agent 自身管理范畴 |
| `supports_tools` / `supports_skills` | Agent 自身能力范畴 |
| `max_context_length` | Agent 自身配置范畴 |

---

## 三、当前三个 Agent 的元信息

### 3.1 Claude Code

| 字段 | 值 |
|------|-----|
| `agent_id` | `claude` |
| `name` | Claude Code |
| `description` | Anthropic 官方 AI 编程助手，支持代码生成、调试、重构 |
| `install_cmd` | `npm install -g @anthropic-ai/claude-code` |
| `uninstall_cmd` | `claude uninstall` |
| `update_cmd` | `claude update` |
| `version_cmd` | `claude --version` |
| `latest_version_cmd` | `npm view @anthropic-ai/claude-code version` |
| `run_cmd_template` | `claude -p --verbose --output-format stream-json --dangerously-skip-permissions {message}` |
| `output_parser` | `json-stream` |
| `output_filter_regex` | `` |
| `version_pattern` | `v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)` |
| `color` | `#3B82F6`（蓝色） |
| `icon` | `🤖` |
| `sort_order` | `1` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`claude -p --verbose --output-format stream-json --dangerously-skip-permissions {message}`
- 输出解析：JSON stream 格式，解析 `{"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}]}}`
- 安装命令：`npm install -g @anthropic-ai/claude-code`
- 版本查询：`npm view @anthropic-ai/claude-code version`

### 3.2 Hermes Agent

| 字段 | 值 |
|------|-----|
| `agent_id` | `hermes` |
| `name` | Hermes Agent |
| `description` | 开源 AI 编程助手，支持多模型后端 |
| `install_cmd` | `pip install hermes-agent` |
| `uninstall_cmd` | `hermes uninstall` |
| `update_cmd` | `hermes update` |
| `version_cmd` | `hermes --version` |
| `latest_version_cmd` | `pip index versions hermes-agent` |
| `run_cmd_template` | `hermes chat -q {message} -Q` |
| `output_parser` | `ansi-text` |
| `output_filter_regex` | `^(Initializing agent|Resume this session|hermes --resume|Session:|Duration:|Messages:|Query:)` |
| `version_pattern` | `v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)` |
| `color` | `#8B5CF6`（紫色） |
| `icon` | `🤖` |
| `sort_order` | `2` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`hermes chat -q {message} -Q`
- 输出解析：ANSI 文本流，需 strip ANSI 转义码后过滤非内容行
- 安装命令：`pip install hermes-agent`
- 版本查询：`pip index versions hermes-agent`

### 3.3 codeX

| 字段 | 值 |
|------|-----|
| `agent_id` | `codex` |
| `name` | codeX |
| `description` | OpenAI 官方 AI 编程助手 |
| `install_cmd` | `npm install -g @openai/codex` |
| `uninstall_cmd` | `codex uninstall` |
| `update_cmd` | `codex update` |
| `version_cmd` | `codex --version` |
| `latest_version_cmd` | `npm view @openai/codex version` |
| `run_cmd_template` | `codex exec {message}` |
| `output_parser` | `json-stream` |
| `output_filter_regex` | `` |
| `version_pattern` | `v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)` |
| `color` | `#10B981`（绿色） |
| `icon` | `🤖` |
| `sort_order` | `3` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`codex exec {message}`
- 输出解析：JSON stream 格式
- 安装命令：`npm install -g @openai/codex`
- 版本查询：`npm view @openai/codex version`

---

## 四、架构设计

### 4.1 数据流

```
+-----------------------------------------------------------+
|                      前端 (React)                           |
|                                                           |
|  useAgentRegistry() <--- invoke('list_agents')            |
|       |                                                    |
|       +--- SessionList (新建会话下拉)                       |
|       +--- EnvManager (安装/检测/更新)                     |
|       +--- AgentBadge (主题色/图标)                        |
|       +--- MessageBubble (主题色)                          |
|       +--- StatusBar (版本显示)                            |
+--------------------------+--------------------------------+
                           | Tauri IPC
+--------------------------v--------------------------------+
|                    后端 (Rust)                              |
|                                                           |
|  list_agents() ---> agents 表 ---> 内存缓存                |
|  get_agent(id)                                            |
|  add_agent() ---> 写入 agents 表 ---> 失效缓存             |
|  update_agent() ---> 写入 agents 表 ---> 失效缓存          |
|  delete_agent() ---> 删除 agents 表 ---> 失效缓存          |
|                                                           |
|  detect_env() ---> 循环 enabled agents ---> 检测 CLI       |
|  install_agent(id) ---> 从元信息读取包管理器/包名 -> 安装   |
|  check_update(id) ---> 从元信息读取 registry -> 查最新版本 |
|                                                           |
|  send_message() ---> 从元信息读取 cli_command/build_args   |
+--------------------------+--------------------------------+
                           | rusqlite
+--------------------------v--------------------------------+
|                    SQLite                                  |
|                                                           |
|  agents 表 (元信息)                                        |
|  sessions 表 (会话) <- agent_type 改为 TEXT (移除 CHECK)   |
|  messages 表 (消息)                                        |
|  inspirations 表 (灵感) <- source_agent 改为 TEXT (移除 CHECK)|
+-----------------------------------------------------------+
```

### 4.2 缓存策略

```rust
// 懒加载 + 写穿透缓存
static AGENT_CACHE: Lazy<RwLock<Option<Vec<AgentConfig>>>> = Lazy::new(|| RwLock::new(None));

fn get_agents(conn: &Connection) -> Vec<AgentConfig> {
    if let Some(cache) = AGENT_CACHE.read().unwrap().as_ref() {
        return cache.clone();
    }
    let agents = load_from_db(conn);
    *AGENT_CACHE.write().unwrap() = Some(agents.clone());
    agents
}

// 写操作时失效
fn update_agent(conn: &Connection, id: &str, ...) {
    conn.execute(...);
    *AGENT_CACHE.write().unwrap() = None;
}

// 后端执行逻辑：读命令 → 替换 {message} → 执行 → 返回结果
// 无需任何包管理器推导逻辑
fn exec_agent_cmd(cmd: &str) -> Result<String, AppError> {
    let parts = shlex::split(cmd).ok_or(AppError::InvalidCommand)?;
    let (program, args) = parts.split_first().unwrap();
    let output = Command::new(program).args(args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
```

### 4.3 数据库迁移

```sql
-- 1. 创建 agents 表
CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    install_cmd TEXT NOT NULL DEFAULT '',
    uninstall_cmd TEXT NOT NULL DEFAULT '',
    update_cmd TEXT NOT NULL DEFAULT '',
    version_cmd TEXT NOT NULL DEFAULT '',
    latest_version_cmd TEXT NOT NULL DEFAULT '',
    run_cmd_template TEXT NOT NULL DEFAULT '',
    output_parser TEXT NOT NULL DEFAULT 'raw-text' CHECK(output_parser IN ('json-stream', 'ansi-text', 'raw-text')),
    output_filter_regex TEXT NOT NULL DEFAULT '',
    version_pattern TEXT NOT NULL DEFAULT 'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)',
    color TEXT NOT NULL DEFAULT '#6366F1',
    icon TEXT NOT NULL DEFAULT '🤖',
    sort_order INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    is_builtin INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 2. 插入预置 Agent 种子数据
INSERT INTO agents (agent_id, name, description,
    install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd,
    run_cmd_template, output_parser, output_filter_regex,
    color, sort_order, is_builtin, created_at, updated_at)
VALUES
    ('claude', 'Claude Code', 'Anthropic 官方 AI 编程助手',
     'npm install -g @anthropic-ai/claude-code',
     'claude uninstall',
     'claude update',
     'claude --version',
     'npm view @anthropic-ai/claude-code version',
     'claude -p --verbose --output-format stream-json --dangerously-skip-permissions {message}',
     'json-stream', '',
     '#3B82F6', 1, 1, 1718640000000, 1718640000000),

    ('hermes', 'Hermes Agent', '开源 AI 编程助手，支持多模型后端',
     'pip install hermes-agent',
     'hermes uninstall',
     'hermes update',
     'hermes --version',
     'pip index versions hermes-agent',
     'hermes chat -q {message} -Q',
     'ansi-text',
     '^(Initializing agent|Resume this session|hermes --resume|Session:|Duration:|Messages:|Query:)',
     '#8B5CF6', 2, 1, 1718640000000, 1718640000000),

    ('codex', 'codeX', 'OpenAI 官方 AI 编程助手',
     'npm install -g @openai/codex',
     'codex uninstall',
     'codex update',
     'codex --version',
     'npm view @openai/codex version',
     'codex exec {message}',
     'json-stream', '',
     '#10B981', 3, 1, 1718640000000, 1718640000000);

-- 3. 移除 sessions 表的 CHECK 约束（SQLite 需重建表）
-- 4. 移除 inspirations 表的 CHECK 约束
```

---

## 五、分阶段实施路径

### Phase 1 — 元信息存储与查询（纯后端）

**目标**：创建 `agents` 表，迁移 `AGENTS` 常量到数据库，新增查询命令。

**工作量**：约 8-10 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 创建 `agents` 表 + 种子数据 | `db/init.rs` | 新增迁移版本 |
| 新增 `list_agents` / `get_agent` 命令 | `commands/agents.rs`（新建） | 含缓存逻辑 |
| 新增 `add_agent` / `update_agent` / `delete_agent` 命令 | `commands/agents.rs` | 含缓存失效 |
| 注册命令到 `lib.rs` | `lib.rs` | |
| 移除 `AGENTS` 常量 | `commands/env.rs` | 改为从数据库读取 |
| 移除 CHECK 约束 | `db/init.rs` | 重建 sessions/inspirations 表 |

**交付物**：
- `agents` 表创建成功，种子数据可查询
- `list_agents` 返回预置三条记录
- 现有功能不受影响（后端仍使用常量，逐步切换）

### Phase 2 — 安装/检测/更新泛化（后端重构）

**目标**：将三个独立安装命令合并为一个泛化命令，环境检测和版本查询由元信息驱动。

**工作量**：约 10-12 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 泛化 `install_agent(agent_id)` 命令 | `commands/env.rs` | 直接执行 `install_cmd` 字段 |
| 泛化 `uninstall_agent(agent_id)` 命令 | `commands/env.rs` | 直接执行 `uninstall_cmd` 字段 |
| 泛化 `check_version(agent_id)` 命令 | `commands/env.rs` | 直接执行 `version_cmd` 字段 |
| 泛化 `check_update(agent_id)` 命令 | `commands/update.rs` | 直接执行 `latest_version_cmd` 字段 |
| 移除 `install_claude_code` / `install_hermes` / `install_codex` | `commands/env.rs` + `lib.rs` | 合并为 1 个泛化命令 |
| 移除 `check_single_npm` / `check_single_pypi` | `commands/update.rs` | 合并为 1 个泛化命令 |
| `detect_env` 改为循环 `enabled` agents | `commands/env.rs` | 执行各 agent 的 `version_cmd` |
| `AgentType` 枚举改为从元信息动态构建 | `agent/mod.rs` | `build_args` 从 `run_cmd_template` 读取 |

**交付物**：
- `install_agent('claude')` 等效于原 `install_claude_code`
- `detect_env` 自动检测所有 `enabled` 的 Agent
- 新增 Agent 无需修改 Rust 代码即可安装和检测

### Phase 3 — 前端元信息驱动（前端重构）

**目标**：前端从后端动态获取 Agent 元信息，消除所有硬编码条件判断。

**工作量**：约 12-15 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 新增 `useAgentRegistry` hook | `hooks/useAgentRegistry.ts`（新建） | 封装 `list_agents` 调用 |
| `AGENT_THEMES` 替换为动态数据 | `types/index.ts` | 保留兼容层 |
| `EnvManager` 改为循环渲染 | `components/env/EnvManager.tsx` | 安装/检测/更新由元信息驱动 |
| `SessionList` 新建会话下拉动态生成 | `components/layout/SessionList.tsx` | |
| 消除所有 `agentType === 'claude'` 条件判断 | 各组件 ~20 处 | 改为元信息字段判断 |

**交付物**：
- 前端无硬编码 Agent 类型字符串
- 新增 Agent 自动出现在新建会话下拉和 EnvManager 中
- 现有 UI 行为完全一致

### Phase 4 — 用户管理界面

**目标**：提供用户可操作的 Agent 管理界面。

**工作量**：约 8-10 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 设置页新增 "Agent 管理" tab | `pages/SettingsPage.tsx` | |
| Agent 列表（启用/禁用、排序） | 新增组件 | |
| 添加自定义 Agent 表单 | 新增组件 | 填写元信息 |
| 编辑 Agent 配置 | 新增组件 | 修改名称/颜色/图标等 |
| 删除自定义 Agent | 新增组件 | 预置 Agent 不可删除 |
| 导入/导出 Agent 配置（JSON） | 新增组件 | |

**交付物**：
- 用户可在设置页管理 Agent
- 新增 Agent 无需任何代码修改

### Phase 5 — 进程协议抽象（可选）

**目标**：支持更多 CLI 交互模式，降低新增 Agent 的进程管理门槛。

**工作量**：约 8-10 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 定义 `ProcessHandler` trait | `agent/handler.rs`（新建） | stdio/http/sse 三种实现 |
| `build_args` 从元信息读取 | `agent/mod.rs` | 可配置参数模板 |
| `parse_output_line` 从元信息读取 | `agent/mod.rs` | 可配置解析器 |
| Agent 市场：从远程源下载配置 | 新增 | 可选功能 |

---

## 六、性能影响分析

### 6.1 查询路径对比

| 场景 | 当前 | 改造后 | 影响 |
|------|------|--------|------|
| Agent 元信息读取 | 编译期常量，O(1) | 首次 SQLite 查询 ~1ms，后续缓存命中 0ms | **可忽略** |
| 环境检测 | 循环 3 次，硬编码 | 从 DB 读 enabled agents -> 循环 N 次（N <= 10） | **可忽略** |
| 新建会话下拉 | 硬编码 4 项 | 从缓存读取 | **无差异** |
| 消息气泡主题色 | 编译期 switch | 从缓存 Map 查找 | **无差异** |
| 安装 Agent | 3 个独立命令 | 1 个泛化命令 + DB 查询元信息 ~0.5ms | **可忽略** |
| 发送消息 | 编译期枚举匹配 | 从缓存读取 cli_command/build_args | **无差异** |
| 内存占用 | 0 | 缓存 ~2KB（10 条配置） | **可忽略** |
| 启动耗时 | 0 | 首次查询 agents 表 ~1-2ms | **可忽略** |

### 6.2 瓶颈不在元信息查询

PilotDesk 当前的性能瓶颈在于：
1. **Agent 子进程管理**（启动/通信/销毁）—— 毫秒到秒级
2. **消息流式渲染**（Markdown 解析 + Virtuoso 虚拟滚动）
3. **SQLite 写入**（消息持久化）

Agent 元信息读取的开销（~1ms 首次，0ms 缓存命中）完全在噪声范围内。

### 6.3 代码量变化预估

| 指标 | 当前 | 改造后 | 变化 |
|------|------|--------|------|
| Rust 后端 | ~200 行硬编码 + 推导逻辑 | ~100 行命令执行器 | -100 行 |
| 前端组件 | ~500 行条件判断 | ~200 行循环渲染 | -300 行 |
| 新增 Agent | 修改 20+ 处 | 填写表单或导入 JSON | **大幅简化** |

**净效果**：代码量减少，可维护性显著提升。

---

## 七、风险与缓解措施

| 风险 | 等级 | 缓解措施 |
|------|------|---------|
| 现有数据库已有 CHECK 约束 | 中 | 迁移脚本重建表，数据无损 |
| 不同 Agent 的进程管理差异大 | 高 | Phase 1-3 仅覆盖 stdio 模式，http/sse 留到 Phase 5 |
| 用户误配置导致 Agent 不可用 | 中 | 输入校验 + 预置 Agent 不可删除 + 恢复默认功能 |
| 动态类型导致 TypeScript 类型安全降低 | 中 | 运行时校验 + Zod schema 验证 |
| 升级时预置 Agent 配置覆盖用户修改 | 低 | upsert 策略：新增字段写入，已有字段保留用户值 |

---

## 八、附录

### 8.1 当前硬编码引用清单

| 文件 | 硬编码内容 | 改造方式 |
|------|-----------|---------|
| `src-tauri/src/commands/env.rs` | `AGENTS` 常量 + 安装/检测硬编码 | 改为从 DB 加载命令并执行 |
| `src-tauri/src/commands/env.rs` | `install_claude_code`/`install_hermes`/`install_codex` | 合并为 `install_agent(id)` |
| `src-tauri/src/commands/update.rs` | `check_single_npm`/`check_single_pypi` | 合并为 `check_update(id)` |
| `src-tauri/src/agent/mod.rs` | `AgentType` 枚举 + `build_args` + `parse_output_line` | 保留枚举，`build_args` 从 `run_cmd_template` 读取 |
| `src-tauri/src/db/init.rs` | CHECK 约束 | 移除 |
| `src-tauri/src/lib.rs` | 3 个安装命令 + 2 个版本查询命令注册 | 替换为 2-3 个泛化命令 |
| `src/types/index.ts` | `AGENT_THEMES` 对象 | 从后端动态获取 |
| `src/components/env/EnvManager.tsx` | 每个 agent 独立安装/检测/更新逻辑 | 循环渲染 |
| `src/components/layout/SessionList.tsx` | 硬编码下拉选项 | 动态生成 |
| `src/hooks/useAgentEvent.ts` | agent 类型分发 | 元信息驱动 |
| 各组件 ~20 处 | `agentType === 'claude'` 等 | 改为元信息字段判断 |

### 8.2 术语表

| 术语 | 说明 |
|------|------|
| Agent | AI 编程助手，如 Claude Code、Hermes Agent、codeX |
| 预置 Agent | PilotDesk 内置的 Agent，不可删除但可禁用 |
| 自定义 Agent | 用户自行添加的 Agent |
| 元信息 | Agent 的静态描述性信息，存储在 `agents` 表中 |
| 运行时状态 | Agent 的版本信息，由检测逻辑实时获取 |
| stdio 模式 | 通过 stdin/stdout 与子进程交互的通信模式 |
| 命令模板 | 存储在数据库中的 shell 命令字符串，`run_cmd_template` 支持 `{message}` 占位符 |
| 输出解析器 | 预定义的输出解析策略（`json-stream` / `ansi-text` / `raw-text`），用于将 CLI 输出转换为结构化消息 |
