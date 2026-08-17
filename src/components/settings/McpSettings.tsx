import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Plus, Trash2, Save, Loader2, Boxes } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';

// 与后端 api_agent/mcp_client.rs 的 McpServerConfig 对齐
interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
}

const EMPTY_SERVER: McpServerConfig = {
  name: '',
  command: '',
  args: [],
};

function argsToString(args: string[]): string {
  return (args ?? []).join(' ');
}

function stringToArgs(text: string): string[] {
  // 简单按空格拆分；含空格的参数暂不支持转义
  return text.trim() ? text.trim().split(/\s+/) : [];
}

export function McpSettings() {
  const [servers, setServers] = useState<McpServerConfig[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const list = await invoke<McpServerConfig[]>('get_mcp_servers');
        setServers(list ?? []);
      } catch (e) {
        console.error('加载 MCP 服务器失败:', e);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleAdd = useCallback(() => {
    setServers((prev) => [...prev, { ...EMPTY_SERVER }]);
  }, []);

  const handleRemove = useCallback((idx: number) => {
    setServers((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  const handleChange = useCallback((idx: number, patch: Partial<McpServerConfig>) => {
    setServers((prev) => prev.map((s, i) => (i === idx ? { ...s, ...patch } : s)));
  }, []);

  const handleSave = useCallback(async () => {
    const cleaned = servers
      .map((s) => ({ ...s, name: s.name.trim(), command: s.command.trim() }))
      .filter((s) => s.name && s.command);
    setSaving(true);
    try {
      await invoke('set_mcp_servers', { servers: cleaned });
      setServers(cleaned);
    } catch (e) {
      console.error('保存 MCP 服务器失败:', e);
    } finally {
      setSaving(false);
    }
  }, [servers]);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 size={20} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
            MCP 服务器
          </h3>
          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            通过 stdio 启动 MCP 服务器，其工具会以 mcp_&lt;服务器&gt;_&lt;工具&gt; 的形式暴露给 API Agent
          </p>
        </div>
        <button
          onClick={handleAdd}
          className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          <Plus size={12} />
          添加服务器
        </button>
      </div>

      {servers.length === 0 ? (
        <SettingsSection title="已配置的服务器">
          <div className="text-center py-6">
            <Boxes size={20} className="mx-auto mb-2" style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              暂无 MCP 服务器，点击上方按钮添加
            </p>
          </div>
        </SettingsSection>
      ) : (
        <div className="space-y-2">
          {servers.map((s, idx) => (
            <div
              key={idx}
              className="rounded-lg p-3"
              style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            >
              <div className="flex items-center gap-2 mb-2">
                <input
                  type="text"
                  value={s.name}
                  onChange={(e) => handleChange(idx, { name: e.target.value })}
                  placeholder="名称（如 filesystem）"
                  className="flex-1 px-2 py-1 rounded text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
                <button
                  onClick={() => handleRemove(idx)}
                  className="pd-btn p-1 rounded transition-colors"
                  style={{ color: 'var(--danger)' }}
                  title="删除"
                >
                  <Trash2 size={13} />
                </button>
              </div>
              <div className="space-y-1.5">
                <input
                  type="text"
                  value={s.command}
                  onChange={(e) => handleChange(idx, { command: e.target.value })}
                  placeholder="命令（如 npx / node / uvx）"
                  className="w-full px-2 py-1 rounded text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
                <input
                  type="text"
                  value={argsToString(s.args)}
                  onChange={(e) => handleChange(idx, { args: stringToArgs(e.target.value) })}
                  placeholder="参数（空格分隔，如 -y @modelcontextprotocol/server-filesystem /path）"
                  className="w-full px-2 py-1 rounded text-xs outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-center justify-end">
        <SettingsButton onClick={handleSave} icon={saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />} disabled={saving}>
          保存
        </SettingsButton>
      </div>

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        保存后将在下一次 API Agent 会话时生效（每轮对话会重新连接并列出工具）。
      </p>
    </div>
  );
}
