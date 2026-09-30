import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, Loader2, Save } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

// ============================================================
// 权限清单（独立设置页）
// 清单只做分类（高风险/风险/安全），不定义具体行为；
// 行为由会话窗口 / 群聊窗口选择的「会话安全模式」
// （严格 / 标准 / 宽松 / 无限制，随消息/波次传参、不持久化）按子策略矩阵决定。
// 文件修改历史已拆分为独立 tab：FileHistorySettings。
// ============================================================

interface PermissionRules {
  /** 安全路径 */
  allowPaths: string[];
  /** 风险路径 */
  denyPaths: string[];
  /** 安全命令 */
  allowCommands: string[];
  /** 高风险命令 */
  denyCommands: string[];
  /** 风险命令 */
  riskyCommands: string[];
}

const EMPTY_RULES: PermissionRules = {
  allowPaths: [],
  denyPaths: [],
  allowCommands: [],
  denyCommands: [],
  riskyCommands: [],
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

function RuleEditor({
  title,
  description,
  placeholder,
  value,
  onChange,
  rows = 4,
}: {
  title: string;
  description: string;
  placeholder: string;
  value: string;
  onChange: (v: string) => void;
  rows?: number;
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
        rows={rows}
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

export function PermissionSettings() {
  const [rules, setRules] = useState<PermissionRules>(EMPTY_RULES);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  /** 保存按钮上的短暂反馈（显示「已保存」两秒后复位） */
  const [justSaved, setJustSaved] = useState(false);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 卸载时清掉未触发的复位定时器，避免对已卸载组件 setState
  useEffect(() => () => {
    if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const r = await invoke<PermissionRules>('get_permission_rules');
      setRules({ ...EMPTY_RULES, ...(r ?? {}) });
    } catch {
      setRules(EMPTY_RULES);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // effect 体内不允许同步 setState（react-hooks/set-state-in-effect）：把首次加载推迟一个微任务，
    // 仍在同一帧内执行，观感与原先一致
    queueMicrotask(() => { void load(); });
  }, [load]);

  const save = async () => {
    setSaving(true);
    try {
      const payload: PermissionRules = { ...rules };
      await invoke('set_permission_rules', { rules: payload });
      setRules(payload);
      // 按钮本身给反馈：成功后短暂变成「已保存」（原先只在页面底部有一行小字，容易被忽略）
      setJustSaved(true);
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
      savedTimerRef.current = setTimeout(() => setJustSaved(false), 2000);
    } catch (err) {
      // 这里原来是静默 ignore —— 保存失败时用户得不到任何提示，只会以为"点了没反应"
      showToast(`保存失败: ${errorMessage(err)}`, 'error');
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
        title="权限清单"
        description="清单只对命令/路径分类（高风险 / 风险 / 安全），不直接决定行为。实际拦截/审批/放行由会话窗口与群聊窗口选择的「会话安全模式」（严格 / 标准 / 宽松 / 无限制）按子策略矩阵决定。"
        actions={
          <SettingsButton
            onClick={save}
            variant="primary"
            disabled={saving}
            icon={saving ? <Loader2 size={12} className="animate-spin" /> : justSaved ? <Check size={12} /> : <Save size={12} />}
          >
            {saving ? '保存中…' : justSaved ? '已保存' : '保存'}
          </SettingsButton>
        }
      >
        <div className="space-y-4">
          <RuleEditor
            title="高风险命令"
            description="确定性破坏性命令/短语，一行一条。命中后按会话安全模式处理（严格/标准/宽松下禁止，无限制下放行）。"
            placeholder={'例如：\nshutdown\ndiskpart\nformat c:\ninvoke-expression\ncertutil -urlcache'}
            value={rulesToString(rules.denyCommands)}
            onChange={(v) => setField('denyCommands', v)}
          />
          <RuleEditor
            title="风险命令"
            description="可能性安全的命令/短语，一行一条。命中后按会话安全模式处理（严格/标准下审批，宽松/无限制下放行）。"
            placeholder={'例如：\ndel\ncopy\nmove\nmkdir\nnpm install\ngit push\n>'}
            value={rulesToString(rules.riskyCommands)}
            onChange={(v) => setField('riskyCommands', v)}
          />
          <RuleEditor
            title="安全命令"
            description="整体安全的「命令 + 首子命令」前缀，一行一条（如 git status；裸 git 需写 git status 形式）。严格模式下仍审批，其余模式放行。"
            placeholder={'例如：\ngit status\ngit log\nipconfig\npowershell get-process'}
            value={rulesToString(rules.allowCommands)}
            onChange={(v) => setField('allowCommands', v)}
          />
          <RuleEditor
            title="安全路径"
            description="命中这些路径的读写操作按会话安全模式处理（严格下审批，其余放行）。工作区目录与用户显式提供的路径默认归入安全路径；常用目录（桌面/文档/下载等）已默认包含，留空表示不额外添加。"
            placeholder={'例如：\nD:\\Data\\**'}
            value={rulesToString(rules.allowPaths)}
            onChange={(v) => setField('allowPaths', v)}
          />
          <RuleEditor
            title="风险路径"
            description="禁止 Agent 读取/写入这些路径，一行一条（默认含系统目录）。按会话安全模式处理（严格/标准/宽松下禁止，无限制下放行）。"
            placeholder={'例如：\nC:\\Windows\\**\n**\\*.env'}
            value={rulesToString(rules.denyPaths)}
            onChange={(v) => setField('denyPaths', v)}
          />
        </div>
      </SettingsSection>
    </div>
  );
}
