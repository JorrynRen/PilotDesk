import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ShieldCheck, History, Undo2, Loader2, Save, RefreshCw,
} from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';

// ============================================================
// Types（与后端 agent_loop.rs 的 PermissionRules / file_history.rs 对齐）
// ============================================================
interface PermissionRules {
  allowPaths: string[];
  denyPaths: string[];
  allowCommands: string[];
  denyCommands: string[];
}

interface FileHistoryEntry {
  id: number;
  sessionId: string;
  filePath: string;
  fileExisted: boolean;
  createdAt: number;
}

const EMPTY_RULES: PermissionRules = {
  allowPaths: [],
  denyPaths: [],
  allowCommands: [],
  denyCommands: [],
};

function rulesToString(list: string[]): string {
  return (list ?? []).join('\n');
}

function stringToRules(text: string): string[] {
  return text
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);
}

function formatTime(secs: number): string {
  if (!secs) return '-';
  return new Date(secs * 1000).toLocaleString();
}

// ============================================================
// 权限规则配置
// ============================================================
function RuleEditor({
  title,
  description,
  placeholder,
  value,
  onChange,
}: {
  title: string;
  description: string;
  placeholder: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div>
      <div className="flex items-center gap-1.5 mb-1">
        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
          {title}
        </span>
      </div>
      <p className="text-[10px] mb-1.5" style={{ color: 'var(--text-tertiary)' }}>
        {description}
      </p>
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        rows={4}
        spellCheck={false}
        className="w-full px-3 py-2 rounded-lg text-xs outline-none resize-y font-mono"
        style={{
          backgroundColor: 'var(--bg-tertiary)',
          color: 'var(--text-primary)',
          border: '1px solid var(--border)',
        }}
      />
    </div>
  );
}

function PermissionRulesEditor() {
  const [rules, setRules] = useState<PermissionRules>(EMPTY_RULES);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const r = await invoke<PermissionRules>('get_permission_rules');
      setRules(r ?? EMPTY_RULES);
    } catch {
      setRules(EMPTY_RULES);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const save = async () => {
    setSaving(true);
    try {
      const payload: PermissionRules = { ...rules };
      await invoke('set_permission_rules', { rules: payload });
      setRules(payload);
      setSavedAt(new Date().toLocaleTimeString());
    } catch {
      // ignore
    } finally {
      setSaving(false);
    }
  };

  const setField = (key: keyof PermissionRules, text: string) => {
    setRules((prev) => ({ ...prev, [key]: stringToRules(text) }));
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-10">
        <Loader2 size={18} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <SettingsSection
        title="权限规则"
        description="Agent 在执行工具调用前的确定性拦截/放行规则。允许列表（allow）命中即放行，拒绝列表（deny）命中即拦截，均命中时拒绝优先。路径支持 * 与 ** 通配符。"
        actions={
          <SettingsButton
            onClick={save}
            variant="primary"
            disabled={saving}
            icon={saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
          >
            保存
          </SettingsButton>
        }
      >
        <div className="space-y-4">
          <RuleEditor
            title="允许访问的路径"
            description="仅允许 Agent 读取/写入这些路径，一行一条。留空表示不额外放行。"
            placeholder={'例如：\ne:\\WorkSpace_LingXi\\PilotDesk\\**\nC:\\Users\\Administrator\\Desktop\\**'}
            value={rulesToString(rules.allowPaths)}
            onChange={(v) => setField('allowPaths', v)}
          />
          <RuleEditor
            title="拒绝访问的路径"
            description="禁止 Agent 读取/写入这些路径，优先级高于允许列表。"
            placeholder={'例如：\nC:\\Windows\\**\n**\\*.env'}
            value={rulesToString(rules.denyPaths)}
            onChange={(v) => setField('denyPaths', v)}
          />
          <RuleEditor
            title="允许执行的命令前缀"
            description="仅允许 Agent 执行以这些前缀开头的命令，一行一条。留空表示不额外放行。"
            placeholder={'例如：\ngit \npython \nnpm '}
            value={rulesToString(rules.allowCommands)}
            onChange={(v) => setField('allowCommands', v)}
          />
          <RuleEditor
            title="拒绝执行的命令"
            description="禁止 Agent 执行包含这些关键词/前缀的命令，优先级高于允许列表。"
            placeholder={'例如：\nshutdown\nformat\ndel /f /s'}
            value={rulesToString(rules.denyCommands)}
            onChange={(v) => setField('denyCommands', v)}
          />
        </div>
        {savedAt && (
          <p className="text-[10px] mt-2" style={{ color: 'var(--success)' }}>
            已保存于 {savedAt}
          </p>
        )}
      </SettingsSection>
    </div>
  );
}

// ============================================================
// 文件修改历史与撤销
// ============================================================
function FileHistoryPanel() {
  const [entries, setEntries] = useState<FileHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [undoingId, setUndoingId] = useState<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<FileHistoryEntry[]>('list_file_history', {
        sessionId: null,
        limit: 200,
      });
      setEntries(list ?? []);
    } catch {
      setEntries([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const undo = async (id: number) => {
    setUndoingId(id);
    try {
      await invoke<string>('undo_file_history', { historyId: id });
      setEntries((prev) => prev.filter((e) => e.id !== id));
    } catch {
      // ignore
    } finally {
      setUndoingId(null);
    }
  };

  return (
    <SettingsSection
      title="文件修改历史"
      description="Agent 通过 write_file / edit_file 修改文件前的快照。可撤销到上一个版本。"
      actions={
        <SettingsButton
          onClick={load}
          variant="secondary"
          icon={<RefreshCw size={12} />}
        >
          刷新
        </SettingsButton>
      }
    >
      {loading ? (
        <div className="flex items-center justify-center py-8">
          <Loader2 size={18} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
        </div>
      ) : entries.length === 0 ? (
        <div className="text-center py-8">
          <History size={24} className="mx-auto mb-2" style={{ color: 'var(--text-tertiary)' }} />
          <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            暂无文件修改记录
          </p>
        </div>
      ) : (
        <div className="space-y-1.5">
          {entries.map((e) => (
            <div
              key={e.id}
              className="flex items-center gap-2 px-3 py-2 rounded-lg"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs truncate font-mono" style={{ color: 'var(--text-primary)' }} title={e.filePath}>
                    {e.filePath}
                  </span>
                  {!e.fileExisted && (
                    <span
                      className="text-[9px] px-1 py-0.5 rounded shrink-0"
                      style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-tertiary)' }}
                    >
                      新建
                    </span>
                  )}
                </div>
                <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                  {formatTime(e.createdAt)}
                  {e.sessionId && <span className="ml-2">会话 {e.sessionId.slice(0, 8)}</span>}
                </p>
              </div>
              <SettingsButton
                onClick={() => undo(e.id)}
                variant="secondary"
                disabled={undoingId === e.id}
                icon={undoingId === e.id ? <Loader2 size={12} className="animate-spin" /> : <Undo2 size={12} />}
              >
                撤销
              </SettingsButton>
            </div>
          ))}
        </div>
      )}
    </SettingsSection>
  );
}

// ============================================================
// 组合导出
// ============================================================
export function AgentPermissionSettings() {
  return (
    <div className="space-y-8">
      <div className="flex items-center gap-2">
        <ShieldCheck size={14} style={{ color: 'var(--accent)' }} />
        <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          Agent 权限与文件历史
        </h2>
      </div>
      <PermissionRulesEditor />
      <FileHistoryPanel />
    </div>
  );
}
