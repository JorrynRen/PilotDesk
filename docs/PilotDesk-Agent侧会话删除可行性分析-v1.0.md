# PilotDesk Agent 侧会话删除可行性分析 v1.0

> 分析日期：2026-06-21
> 状态：可行性分析（不涉及代码调整）
> 核心问题：删除 PilotDesk 会话时，仅删除了本地 SQLite 记录，未清理 Agent CLI 侧的会话数据文件

---

## 1. 问题定义

### 1.1 当前行为

```
用户删除会话
  → delete_session (session.rs)
    → DELETE FROM sessions WHERE id = ? (CASCADE 删除 messages)
    → AgentManager.close_session() 终止进程
  → Agent CLI 侧的会话文件未被清理（~/.claude/sessions/xxx 等）
```

### 1.2 影响

| 维度 | 说明 |
|------|------|
| 磁盘占用 | Agent 侧会话文件持续累积 |
| 隐私 | 删除 PilotDesk 会话后，Agent 侧仍保留完整对话历史 |
| 一致性 | 用户期望"删除"是彻底的 |

---

## 2. 方案分析

### 2.1 方案 A：CLI 命令删除（不可行）

**思路**：通过 Agent CLI 提供的删除命令清理会话。

**问题**：部分 Agent 不提供 shell 删除命令。

| Agent | 提供删除命令？ | 依据 |
|-------|--------------|------|
| Claude Code | `claude session delete <id>` | 有明确 CLI 命令 |
| Hermes | 不确定 | 文档未明确提及 shell 删除命令 |
| CodeX | 不确定 | 文档未明确提及 shell 删除命令 |

**结论**：不可行。依赖 Agent CLI 是否提供删除命令，无法统一处理。

### 2.2 方案 B：文件系统直接删除（推荐）

**思路**：所有 Agent 的会话数据都存储在本地文件系统中，直接通过 Rust 的文件操作 API 删除对应文件/目录，不依赖 Agent CLI。

**核心逻辑**：

```
delete_session
  → 读取 sessions.agent_session_id
  → 查找 AgentConfig.sessions_dir（会话存储目录路径模板）
  → 拼接完整路径，删除对应文件/目录
  → 删除本地 SQLite 记录
  → AgentManager.close_session()
```

**优点**：
- 不依赖 Agent CLI 是否提供删除命令
- 统一处理所有 Agent
- 删除操作可靠（文件系统操作 vs CLI 命令执行）
- 实现简单，无需 spawn 子进程

**缺点**：
- 需要知道各 Agent 的会话存储目录结构和文件命名规则

---

## 3. 各 Agent 会话存储结构

### 3.1 Claude Code

| 维度 | 说明 |
|------|------|
| 存储目录 | `~/.claude/sessions/` |
| 存储结构 | 每个会话一个独立子目录，目录名 = session_id |
| 示例 | `~/.claude/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/` |
| 目录内容 | `messages.jsonl`、`tool_output/` 等 |
| 删除方式 | `std::fs::remove_dir_all()` 删除整个目录 |

### 3.2 Hermes Agent

| 维度 | 说明 |
|------|------|
| 存储目录 | `~/.hermes/sessions/` |
| 存储结构 | 每个会话一个独立文件或目录（待确认） |
| 示例 | `~/.hermes/sessions/{session_id}.json` 或 `~/.hermes/sessions/{session_id}/` |
| 删除方式 | 根据实际结构选择 `remove_file()` 或 `remove_dir_all()` |

### 3.3 CodeX

| 维度 | 说明 |
|------|------|
| 存储目录 | `~/.codex/sessions/`（推测） |
| 存储结构 | `rollout-*.jsonl` 文件（待确认） |
| 删除方式 | 根据实际结构选择 |

---

## 4. 技术方案

### 4.1 AgentConfig 扩展

```rust
// AgentConfig 新增字段
pub sessions_dir: String,       // 会话存储目录模板，支持 ~ 和 {agent_type} 占位符
pub session_file_pattern: String, // 会话文件/目录命名模式，{session_id} 占位符
```

**各 Agent 配置值**：

| Agent | sessions_dir | session_file_pattern |
|-------|-------------|---------------------|
| Claude Code | `~/.claude/sessions/` | `{session_id}`（子目录） |
| Hermes | `~/.hermes/sessions/` | `{session_id}`（待确认） |
| CodeX | `~/.codex/sessions/` | `{session_id}`（待确认） |

### 4.2 核心实现

```rust
/// 删除 Agent 侧会话数据（文件系统直接删除）
fn delete_agent_session_files(
    agent_type: &str,
    agent_session_id: &str,
    agents: &[AgentConfig],
) -> Result<(), String> {
    let agent = agents.iter()
        .find(|a| a.agent_type == agent_type)
        .ok_or_else(|| format!("Agent 配置未找到: {}", agent_type))?;

    if agent.sessions_dir.is_empty() || agent.session_file_pattern.is_empty() {
        return Err("sessions_dir 或 session_file_pattern 未配置".into());
    }

    // 展开 ~ 为用户 home 目录
    let dir = if agent.sessions_dir.starts_with("~/") {
        if let Some(home) = home_dir() {
            home.join(&agent.sessions_dir[2..])
        } else {
            return Err("无法获取用户 home 目录".into());
        }
    } else {
        std::path::PathBuf::from(&agent.sessions_dir)
    };

    // 替换 {session_id} 占位符
    let file_name = agent.session_file_pattern.replace("{session_id}", agent_session_id);
    let target = dir.join(&file_name);

    if !target.exists() {
        log::warn!("[Agent/{}] 会话文件不存在，可能已被清理: {:?}", agent_type, target);
        return Ok(()); // 不存在不算错误
    }

    // 删除（支持文件和目录两种形式）
    if target.is_dir() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("删除会话目录失败: {} - {:?}", e, target))?;
    } else {
        std::fs::remove_file(&target)
            .map_err(|e| format!("删除会话文件失败: {} - {:?}", e, target))?;
    }

    log::info!("[Agent/{}] 会话数据已删除: {:?}", agent_type, target);
    Ok(())
}
```

### 4.3 修改 delete_session 命令

```rust
#[tauri::command]
pub fn delete_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;

    // 1. 读取 agent_session_id 和 agent_type
    let (agent_type, agent_session_id): (String, Option<String>) = conn.query_row(
        "SELECT agent_type, agent_session_id FROM sessions WHERE id = ?1",
        params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // 2. 如果有 agent_session_id，尝试删除 Agent 侧会话文件
    if let Some(ref sid) = agent_session_id {
        if !sid.is_empty() {
            let agents = crate::commands::agents::list_agents_inner(&conn)?;
            if let Err(e) = delete_agent_session_files(&agent_type, sid, &agents) {
                // 非关键路径，失败仅记录 warn，不影响本地删除
                log::warn!("[Agent/{}] 删除会话文件失败: {}", agent_type, e);
            }
        }
    }

    // 3. 删除本地数据库记录
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;

    Ok(())
}
```

### 4.4 需要修改的文件

| 文件 | 变更 | 说明 |
|------|------|------|
| `commands/agents.rs` | `AgentConfig` 新增 `sessions_dir`、`session_file_pattern` 字段 | 会话存储路径配置 |
| `db/init.rs` | migration v7：新增字段 + 更新内置 Agent 种子数据 | 数据库迁移 |
| `commands/session.rs` | `delete_session` 新增文件删除逻辑 | 核心改动 |
| 前端 | 无需改动 | 后端处理，用户无感知 |

**数据库 Migration**：

```sql
-- migration v7
ALTER TABLE agents ADD COLUMN sessions_dir TEXT NOT NULL DEFAULT '';
ALTER TABLE agents ADD COLUMN session_file_pattern TEXT NOT NULL DEFAULT '';

UPDATE agents SET sessions_dir = '~/.claude/sessions/', session_file_pattern = '{session_id}' WHERE agent_type = 'claude';
UPDATE agents SET sessions_dir = '~/.hermes/sessions/', session_file_pattern = '{session_id}' WHERE agent_type = 'hermes';
UPDATE agents SET sessions_dir = '~/.codex/sessions/', session_file_pattern = '{session_id}' WHERE agent_type = 'codex';
```

---

## 5. 边界情况与风险

### 5.1 边界情况

| 场景 | 处理方式 |
|------|---------|
| `agent_session_id` 为空 | 跳过文件删除，仅删除本地 |
| 会话文件/目录不存在 | 记录 warn，不报错（可能已被 Agent 自身清理） |
| `sessions_dir` 未配置 | 跳过，兼容旧数据 |
| API 直连模式（`agent_type = "api"`）| 无对应 Agent 配置，跳过 |
| 权限不足无法删除 | 记录 warn，不影响本地删除 |
| 批量删除 | 每个会话独立处理 |

### 5.2 风险

| 风险 | 等级 | 说明 | 缓解措施 |
|------|------|------|---------|
| 路径遍历攻击 | **低** | `agent_session_id` 含 `../` 等恶意路径 | 路径规范化 + 禁止 `..` |
| 误删非会话文件 | **低** | `sessions_dir` 配置错误 | 仅删除 `sessions_dir` 下的内容 |
| 文件锁定 | **低** | Agent 正在使用该会话文件 | Windows 上文件被占用时删除失败，记录 warn |

### 5.3 路径安全

```rust
fn delete_agent_session_files(...) -> Result<(), String> {
    // ... 展开路径 ...

    // 安全校验：禁止路径遍历
    let canonical = target.canonicalize()
        .map_err(|e| format!("路径解析失败: {}", e))?;

    let base = dir.canonicalize()
        .map_err(|e| format!("基础路径解析失败: {}", e))?;

    // 确保目标路径在 sessions_dir 下
    if !canonical.starts_with(&base) {
        return Err(format!("路径越权: {:?} 不在 {:?} 下", canonical, base));
    }

    // ... 执行删除 ...
}
```

---

## 6. 总结

### 6.1 可行性结论

**结论：完全可行，且比 CLI 命令方案更可靠。**

| 维度 | CLI 命令方案 | 文件系统方案（推荐） |
|------|-------------|-------------------|
| Agent 依赖 | 依赖 CLI 提供删除命令 | 不依赖，通用 |
| 实现方式 | spawn 子进程 | 直接文件操作 |
| 可靠性 | 依赖 CLI 退出码 | 直接操作系统 API |
| 统一性 | 各 Agent 命令格式不同 | 统一处理 |
| **推荐度** | ❌ 不可行 | ✅ 推荐 |

### 6.2 核心逻辑

```
delete_session
  → 读取 agent_session_id（有值且非空）
    → 查找 AgentConfig.sessions_dir + session_file_pattern
    → 拼接完整路径
    → 路径安全检查（防止遍历攻击）
    → std::fs::remove_dir_all() 或 remove_file()
    → 失败仅记录 warn
  → 删除本地 SQLite 记录
```

### 6.3 工时估算

| 步骤 | 预估工时 |
|------|---------|
| AgentConfig 扩展 + migration | 1-2h |
| delete_agent_session_files 函数 | 1-2h |
| delete_session 逻辑修改 | 1h |
| 测试验证 | 1-2h |
| **合计** | **4-7h** |

---

## 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v1.0 | 2026-06-21 | Agent 侧会话删除可行性分析（文件系统方案） | `{{GIT_HASH}}` |
