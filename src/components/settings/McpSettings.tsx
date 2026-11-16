import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Plus, Trash2, Loader2, Plug, FileJson } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';

// 与后端 tools/mcp.rs 的 McpServerConfig 对齐
interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
  cwd?: string;
  timeoutSecs?: number;
  transport?: string;
  url?: string;
  headers?: Record<string, string>;
}

// 编辑态：args/env/headers 以原始文本保存，避免"边输入边解析"导致光标跳动或内容丢失
interface ServerDraft {
  name: string;
  transport: 'stdio' | 'http';
  command: string;
  argsText: string;
  envText: string;
  cwd: string;
  timeoutText: string;
  url: string;
  headersText: string;
}

const EMPTY_SERVER: ServerDraft = {
  name: '',
  transport: 'stdio',
  command: '',
  argsText: '',
  envText: '',
  cwd: '',
  timeoutText: '',
  url: '',
  headersText: '',
};

/** 按 shell 习惯拆分参数：支持单/双引号包裹含空格的参数，反斜杠转义引号 */
function parseArgs(text: string): string[] {
  const out: string[] = [];
  let cur = '';
  let quote: string | null = null;
  let pending = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === quote) {
        quote = null;
      } else if (ch === '\\' && i + 1 < text.length) {
        cur += text[++i];
      } else {
        cur += ch;
      }
      pending = true;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      pending = true;
    } else if (/\s/.test(ch)) {
      if (pending) {
        out.push(cur);
        cur = '';
        pending = false;
      }
    } else {
      cur += ch;
      pending = true;
    }
  }
  if (pending) out.push(cur);
  return out;
}

/** 回显参数：含空白的参数自动加双引号，保证 parseArgs 可还原 */
function formatArgs(args: string[]): string {
  return (args ?? [])
    .map((a) =>
      /\s|"/.test(a) ? `"${a.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"` : a,
    )
    .join(' ');
}

/** 每行一条 KEY=VALUE；空行与 # 开头的行忽略；无 = 的行忽略 */
function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const idx = line.indexOf('=');
    if (idx <= 0) continue;
    const key = line.slice(0, idx).trim();
    if (!key) continue;
    env[key] = line.slice(idx + 1);
  }
  return env;
}

function formatEnv(env?: Record<string, string>): string {
  return Object.entries(env ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');
}

function toDraft(cfg: McpServerConfig): ServerDraft {
  // 传输类型：显式 transport 优先，否则按 url 有无推断（与后端 is_http 一致）
  const transport: 'stdio' | 'http' =
    cfg.transport === 'http' || (!cfg.transport && !!cfg.url) ? 'http' : 'stdio';
  return {
    name: cfg.name ?? '',
    transport,
    command: cfg.command ?? '',
    argsText: formatArgs(cfg.args),
    envText: formatEnv(cfg.env),
    cwd: cfg.cwd ?? '',
    timeoutText: cfg.timeoutSecs ? String(cfg.timeoutSecs) : '',
    url: cfg.url ?? '',
    headersText: formatEnv(cfg.headers),
  };
}

function fromDraft(d: ServerDraft): McpServerConfig {
  const timeout = Number.parseInt(d.timeoutText.trim(), 10);
  const cfg: McpServerConfig = {
    name: d.name.trim(),
    transport: d.transport,
    // 按类型裁剪字段：http 不带 stdio 的进程配置，stdio 不带 http 的网络配置
    command: d.transport === 'stdio' ? d.command.trim() : '',
    args: d.transport === 'stdio' ? parseArgs(d.argsText) : [],
  };
  if (Number.isFinite(timeout) && timeout > 0) cfg.timeoutSecs = timeout;
  if (d.transport === 'stdio') {
    const env = parseEnv(d.envText);
    if (Object.keys(env).length > 0) cfg.env = env;
    const cwd = d.cwd.trim();
    if (cwd) cfg.cwd = cwd;
  } else {
    cfg.url = d.url.trim();
    const headers = parseEnv(d.headersText);
    if (Object.keys(headers).length > 0) cfg.headers = headers;
  }
  return cfg;
}

// ── 从 JSON 导入 ──────────────────────────────────────────────────────────────
// 只做「粘贴 → 解析成表单条目」，JSON 不作权威存储：导入后仍是表单可编辑。
// 兼容常见外壳：{ mcpServers }、{ servers }、{ mcp: { servers } }、裸 name→config 映射、配置数组。

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v);
}

/** 单个服务器配置对象 → 编辑态草稿；不合法时记录错误并返回 null */
function draftFromJson(name: string, raw: unknown, errors: string[]): ServerDraft | null {
  if (!isPlainObject(raw)) {
    errors.push(`${name}：配置必须是对象`);
    return null;
  }
  // 明确禁用的条目直接忽略，不当作错误
  if (raw.disabled === true) return null;

  const typeRaw = String(raw.type ?? raw.transport ?? '').trim().toLowerCase();
  if (typeRaw === 'sse') {
    errors.push(`${name}：不支持旧版 SSE 传输，请改用 Streamable HTTP 地址`);
    return null;
  }
  const url = typeof raw.url === 'string' ? raw.url : typeof raw.serverUrl === 'string' ? raw.serverUrl : '';
  const isHttp =
    typeRaw ? ['http', 'streamable-http', 'streamablehttp', 'streamable'].includes(typeRaw) : !!url;
  const command = typeof raw.command === 'string' ? raw.command : '';

  if (isHttp && !url) {
    errors.push(`${name}：HTTP 传输缺少 url`);
    return null;
  }
  if (!isHttp && !command) {
    errors.push(`${name}：stdio 传输缺少 command`);
    return null;
  }

  // args 允许字符串（按 shell 习惯拆分）或字符串数组
  const args = Array.isArray(raw.args)
    ? (raw.args as unknown[]).map((a) => String(a))
    : typeof raw.args === 'string'
      ? parseArgs(raw.args)
      : [];

  const kv = (v: unknown) =>
    isPlainObject(v)
      ? Object.entries(v)
          .map(([k, val]) => `${k}=${String(val)}`)
          .join('\n')
      : '';

  const timeoutRaw = raw.timeoutSecs ?? raw.timeout;
  const timeout = Number(timeoutRaw);

  return {
    name: name.trim(),
    transport: isHttp ? 'http' : 'stdio',
    command,
    argsText: formatArgs(args),
    envText: kv(raw.env),
    cwd: typeof raw.cwd === 'string' ? raw.cwd : '',
    timeoutText: Number.isFinite(timeout) && timeout > 0 ? String(Math.trunc(timeout)) : '',
    url,
    headersText: kv(raw.headers),
  };
}

interface ImportResult {
  drafts: ServerDraft[];
  errors: string[];
}

/** 解析粘贴的 JSON 文本（外壳格式自动识别） */
function parseImportJson(text: string): ImportResult {
  let root: unknown;
  try {
    root = JSON.parse(text);
  } catch (e) {
    return { drafts: [], errors: [`JSON 解析失败：${String(e)}`] };
  }

  const errors: string[] = [];
  const blocks: Array<[string, unknown]> = [];

  if (Array.isArray(root)) {
    // 数组形式：[{ name, command|url, ... }]
    root.forEach((item, i) => {
      const nm = isPlainObject(item) && typeof item.name === 'string' ? item.name : `server${i + 1}`;
      blocks.push([nm, item]);
    });
  } else if (isPlainObject(root)) {
    const nested =
      root.mcpServers ??
      (isPlainObject(root.mcp) ? root.mcp.servers : undefined) ??
      root.servers;
    if (isPlainObject(nested)) {
      // 标准的 name → config 映射
      Object.entries(nested).forEach(([k, v]) => blocks.push([k, v]));
    } else if (typeof root.command === 'string' || typeof root.url === 'string') {
      // 单个服务器对象
      blocks.push([typeof root.name === 'string' ? root.name : '', root]);
    } else {
      // 裸映射 { name: config, ... }
      Object.entries(root).forEach(([k, v]) => blocks.push([k, v]));
    }
  } else {
    return { drafts: [], errors: ['无法识别的 JSON 结构（应为对象或数组）'] };
  }

  const drafts: ServerDraft[] = [];
  for (const [name, raw] of blocks) {
    if (!name.trim()) {
      errors.push('存在缺少名称的服务器条目');
      continue;
    }
    const draft = draftFromJson(name, raw, errors);
    if (draft) drafts.push(draft);
  }
  return { drafts, errors };
}

/** 卡片：编辑缓冲 + 上次成功落库的值，是「提交 / 取消」的语义基础 */
interface Card {
  key: string;
  /** 本地编辑缓冲（输入框直接改它，尚未写库） */
  draft: ServerDraft;
  /** 上次成功落库的值（取消时回滚到这里） */
  committed: ServerDraft;
  /** 是否已落库（false = 新增但未提交） */
  persisted: boolean;
}

let cardSeq = 0;
const nextKey = () => `mcp-card-${++cardSeq}`;

/** 单条配置是否具备保存条件（stdio 需 command，http 需 url） */
function isComplete(d: ServerDraft): boolean {
  return !!d.name.trim() && (d.transport === 'http' ? !!d.url.trim() : !!d.command.trim());
}

export function McpSettings() {
  const [cards, setCards] = useState<Card[]>([]);
  const [loading, setLoading] = useState(true);
  // 写库请求在途（提交 / 删除 / 导入共用，统一禁用相关按钮）
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 操作结果提示（中性色，与红色错误区分）
  const [notice, setNotice] = useState<string | null>(null);
  // JSON 导入面板（只导入，不做权威存储）
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState('');

  useEffect(() => {
    (async () => {
      try {
        const drafts = (await invoke<McpServerConfig[]>('get_mcp_servers') ?? []).map(toDraft);
        setCards(
          drafts.map((d) => ({
            key: nextKey(),
            draft: d,
            committed: d,
            persisted: true,
          })),
        );
      } catch (e) {
        console.error('加载 MCP 服务器失败:', e);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  /**
   * 统一落库：以各卡片的 committed 值写入（未提交的编辑缓冲不会落库）。
   * 成功则替换卡片列表并给出提示；失败保留现场。
   */
  const persist = useCallback(async (next: Card[], success: string): Promise<boolean> => {
    const configs = next.map((c) => fromDraft(c.committed));
    const names = new Set<string>();
    for (const cfg of configs) {
      if (names.has(cfg.name)) {
        setError(`服务器名称重复：${cfg.name}（名称用于工具前缀，必须唯一）`);
        return false;
      }
      names.add(cfg.name);
    }
    setSaving(true);
    setError(null);
    try {
      await invoke('set_mcp_servers', { servers: configs });
      setCards(next);
      setNotice(success);
      return true;
    } catch (e) {
      console.error('保存 MCP 服务器失败:', e);
      setError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  }, []);

  const handleAdd = useCallback(() => {
    setCards((prev) => [
      ...prev,
      {
        key: nextKey(),
        draft: { ...EMPTY_SERVER },
        committed: { ...EMPTY_SERVER },
        persisted: false,
      },
    ]);
    setNotice(null);
    setError(null);
  }, []);

  const handleChange = useCallback((key: string, patch: Partial<ServerDraft>) => {
    setCards((prev) => prev.map((c) => (c.key === key ? { ...c, draft: { ...c.draft, ...patch } } : c)));
    setError(null);
    setNotice(null);
  }, []);

  /** 提交单条：校验 → 归一化（参数/环境变量格式统一）→ 落库 → 该条转为已提交 */
  const handleSubmit = useCallback(
    async (key: string) => {
      const card = cards.find((c) => c.key === key);
      if (!card) return;
      if (!isComplete(card.draft)) {
        setError(
          card.draft.transport === 'http'
            ? '请填写服务器名称与 url'
            : '请填写服务器名称与启动命令',
        );
        return;
      }
      const cfg = fromDraft(card.draft);
      const dup = cards.some(
        (c) => c.key !== key && c.persisted && fromDraft(c.committed).name === cfg.name,
      );
      if (dup) {
        setError(`服务器名称重复：${cfg.name}（名称用于工具前缀，必须唯一）`);
        return;
      }
      const normalized = toDraft(cfg);
      await persist(
        cards.map((c) =>
          c.key === key
            ? { ...c, draft: normalized, committed: normalized, persisted: true }
            : c,
        ),
        `已保存 ${cfg.name}`,
      );
    },
    [cards, persist],
  );

  /** 取消单条：未提交的新增卡片直接移除，其余回滚到已落库的值 */
  const handleCancel = useCallback((key: string) => {
    setCards((prev) => {
      const card = prev.find((c) => c.key === key);
      if (!card) return prev;
      if (!card.persisted) return prev.filter((c) => c.key !== key);
      return prev.map((c) => (c.key === key ? { ...c, draft: c.committed } : c));
    });
    setError(null);
    setNotice(null);
  }, []);

  /** 删除：已落库的直接写库移除，未提交的直接丢弃 */
  const handleDelete = useCallback(
    async (key: string) => {
      const card = cards.find((c) => c.key === key);
      if (!card) return;
      if (!card.persisted) {
        setCards((prev) => prev.filter((c) => c.key !== key));
        setError(null);
        setNotice(null);
        return;
      }
      await persist(
        cards.filter((c) => c.key !== key),
        `已删除 ${fromDraft(card.committed).name}`,
      );
    },
    [cards, persist],
  );

  // 导入：解析成草稿后追加并直接落库（同名条目跳过，不覆盖已填写的配置）
  const handleImport = useCallback(async () => {
    const { drafts, errors } = parseImportJson(importText);
    if (drafts.length === 0) {
      setNotice(null);
      setError(errors.length > 0 ? errors.join('；') : '未解析出任何服务器配置');
      return;
    }
    const names = new Set(
      cards.filter((c) => c.persisted).map((c) => fromDraft(c.committed).name),
    );
    const accepted: Card[] = [];
    let skipped = 0;
    for (const d of drafts) {
      if (names.has(d.name)) {
        skipped += 1;
        continue;
      }
      names.add(d.name);
      accepted.push({ key: nextKey(), draft: d, committed: d, persisted: true });
    }
    setImportOpen(false);
    setImportText('');
    const notes = skipped > 0 ? [`跳过同名的 ${skipped} 项`] : [];
    setError(errors.length > 0 ? errors.join('；') : null);
    if (accepted.length > 0) {
      await persist(
        [...cards, ...accepted],
        notes.length > 0
          ? `已导入 ${accepted.length} 项（${notes.join('；')}）`
          : `已导入 ${accepted.length} 项`,
      );
    }
  }, [importText, cards, persist]);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 size={20} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
      </div>
    );
  }

  const inputClass = 'pd-settings-input w-full';
  const inputStyle = {
    backgroundColor: 'var(--bg-field)',
    color: 'var(--text-primary)',
    border: '1px solid var(--border)',
  } as const;

  return (
    <div className="settings-page-stack">
      <SettingsSection
        title="MCP 服务器"
        description="支持 stdio（本地命令）与 Streamable HTTP（远程服务）。工具以 mcp_<服务器>_<工具> 暴露给 API Agent；Windows 会自动解析 npx/uvx 等 .cmd 垫片。"
        bodyless={!importOpen}
        actions={
          <>
            <SettingsButton
              onClick={() => {
                setImportOpen((v) => !v);
                setError(null);
              }}
              icon={<FileJson size={12} />}
            >
              从 JSON 导入
            </SettingsButton>
            <SettingsButton
              variant="primary"
              onClick={handleAdd}
              icon={<Plus size={12} />}
            >
              添加服务器
            </SettingsButton>
          </>
        }
      >

      {importOpen && (
        <div
          className="space-y-2"
        >
          <p className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            粘贴 MCP 配置 JSON（兼容 mcpServers / servers / mcp.servers / 数组），导入后按表单逐条编辑
          </p>
          <textarea
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            rows={6}
            placeholder={'{\n  "mcpServers": {\n    "filesystem": {\n      "command": "npx",\n      "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:\\\\work"]\n    }\n  }\n}'}
            className="pd-settings-input w-full resize-y font-mono"
          />
          <div className="flex items-center justify-end gap-2">
            <SettingsButton
              variant="ghost"
              onClick={() => {
                setImportOpen(false);
                setImportText('');
              }}
            >
              取消
            </SettingsButton>
            <SettingsButton onClick={handleImport} disabled={!importText.trim()}>
              导入
            </SettingsButton>
          </div>
        </div>
      )}
      </SettingsSection>

      {cards.length === 0 ? (
        <SettingsSection title="已配置的服务器">
          <div
            className="pd-settings-empty"
          >
            <div
              className="mb-2.5 flex h-9 w-9 items-center justify-center rounded-full"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-tertiary)' }}
            >
              <Plug size={17} />
            </div>
            <p className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
              暂无 MCP 服务器
            </p>
            <p
              className="mt-1 max-w-[420px] text-[10px] leading-relaxed"
              style={{ color: 'var(--text-tertiary)', textWrap: 'pretty' }}
            >
              从上方添加服务器，或导入现有 JSON 配置。保存后会在下一次 API Agent 会话时连接并列出工具。
            </p>
          </div>
        </SettingsSection>
      ) : (
        <SettingsSection
          title="已配置的服务器"
          description={`共 ${cards.length} 项。每条配置独立提交，提交后立即保存。`}
        >
        <div className="space-y-2">
          {cards.map((card) => {
            const s = card.draft;
            // 本条是否有未提交的改动（新增卡片始终视为有改动）
            const changed =
              !card.persisted ||
              JSON.stringify(fromDraft(s)) !== JSON.stringify(fromDraft(card.committed));
            return (
            <div
              key={card.key}
              className="rounded-md p-3"
              style={{ backgroundColor: 'var(--bg-raised)', border: '1px solid var(--border)' }}
            >
              <div className="flex items-center gap-2 mb-2">
                <input
                  type="text"
                  value={s.name}
                  onChange={(e) => handleChange(card.key, { name: e.target.value })}
                  placeholder="名称（如 filesystem）"
                  className="pd-settings-input flex-1"
                />
                {/* 传输类型切换：stdio（本地命令）/ http（远程服务） */}
                <div
                  className="flex rounded overflow-hidden shrink-0"
                  style={{ border: '1px solid var(--border)' }}
                >
                  {(['stdio', 'http'] as const).map((t) => (
                    <button
                      key={t}
                      onClick={() => handleChange(card.key, { transport: t })}
                      className="pd-btn px-2 py-1 text-[10px] transition-colors"
                      style={{
                        backgroundColor: s.transport === t ? 'var(--accent)' : 'var(--bg-field)',
                        color: s.transport === t ? '#fff' : 'var(--text-secondary)',
                      }}
                    >
                      {t === 'stdio' ? 'stdio' : 'HTTP'}
                    </button>
                  ))}
                </div>
                <button
                  onClick={() => handleDelete(card.key)}
                  className="pd-btn p-1 rounded transition-colors"
                  style={{ color: 'var(--danger)' }}
                  title="删除"
                >
                  <Trash2 size={13} />
                </button>
              </div>
              <div className="space-y-1.5">
                {s.transport === 'stdio' ? (
                  <>
                    <input
                      type="text"
                      value={s.command}
                      onChange={(e) => handleChange(card.key, { command: e.target.value })}
                      placeholder="命令（如 npx / node / uvx）"
                      className={inputClass}
                      style={inputStyle}
                    />
                    <input
                      type="text"
                      value={s.argsText}
                      onChange={(e) => handleChange(card.key, { argsText: e.target.value })}
                      placeholder="参数（空格分隔，含空格的参数用双引号包裹，如 -y @modelcontextprotocol/server-filesystem &quot;C:\My Docs&quot;）"
                      className={inputClass}
                      style={inputStyle}
                    />
                    <textarea
                      value={s.envText}
                      onChange={(e) => handleChange(card.key, { envText: e.target.value })}
                      placeholder={'环境变量（可选，每行一条 KEY=VALUE）\n如 API_KEY=xxx'}
                      rows={2}
                      className={`${inputClass} resize-y font-mono`}
                      style={inputStyle}
                    />
                    <div className="flex items-center gap-2">
                      <input
                        type="text"
                        value={s.cwd}
                        onChange={(e) => handleChange(card.key, { cwd: e.target.value })}
                        placeholder="工作目录（可选，如 C:\\work）"
                        className={inputClass}
                        style={inputStyle}
                      />
                      <input
                        type="number"
                        min={1}
                        value={s.timeoutText}
                        onChange={(e) => handleChange(card.key, { timeoutText: e.target.value })}
                        placeholder="超时秒"
                        className="pd-settings-input w-24"
                      />
                    </div>
                  </>
                ) : (
                  <>
                    <input
                      type="text"
                      value={s.url}
                      onChange={(e) => handleChange(card.key, { url: e.target.value })}
                      placeholder="服务地址（如 https://example.com/mcp）"
                      className={inputClass}
                      style={inputStyle}
                    />
                    <textarea
                      value={s.headersText}
                      onChange={(e) => handleChange(card.key, { headersText: e.target.value })}
                      placeholder={'自定义请求头（可选，每行一条 KEY=VALUE）\n如 Authorization=Bearer xxx'}
                      rows={2}
                      className={`${inputClass} resize-y font-mono`}
                      style={inputStyle}
                    />
                    <div className="flex items-center gap-2">
                      <input
                        type="number"
                        min={1}
                        value={s.timeoutText}
                        onChange={(e) => handleChange(card.key, { timeoutText: e.target.value })}
                        placeholder="超时秒"
                        className="pd-settings-input w-24"
                      />
                    </div>
                  </>
                )}
              </div>
              {/* 每条独立提交：提交即落库；取消回滚到已保存的值（新增未提交则整条移除） */}
              <div className="flex items-center justify-end gap-2 mt-2">
                <SettingsButton onClick={() => handleCancel(card.key)} disabled={saving} variant="ghost">
                  取消
                </SettingsButton>
                <SettingsButton
                  onClick={() => handleSubmit(card.key)}
                  disabled={saving || !changed}
                >
                  提交
                </SettingsButton>
              </div>
            </div>
            );
          })}
        </div>
        </SettingsSection>
      )}

      {error && (
        <p className="pd-settings-note px-1" style={{ color: 'var(--danger)' }}>
          {error}
        </p>
      )}
      {notice && !error && (
        <p className="pd-settings-note px-1" style={{ color: 'var(--text-secondary)' }}>
          {notice}
        </p>
      )}

      {/* 每条卡片独立提交后即写库，因此不再需要底部统一保存 */}
      {cards.length > 0 && (
        <p className="pd-settings-note px-1">
          提交后立即保存，将在下一次 API Agent 会话时生效（每轮对话会重新连接并列出工具）。
        </p>
      )}
    </div>
  );
}
