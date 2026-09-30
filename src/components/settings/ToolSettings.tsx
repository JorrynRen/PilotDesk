import { useEffect, useMemo, useState, useCallback, useRef } from 'react';
import { Search, Lock, Save, RotateCcw, Wrench, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { Select } from '../common/Select';
import { errorMessage } from '../../utils/errorMessage';

// ============================================================
// 工具管理（后端打通版）
// - 目录：invoke('tool_catalog')，返回内建工具元数据 + 各场景默认状态/锁定
// - 覆盖：invoke('get_tool_overrides') / invoke('set_tool_overrides')
//   overrides 为「追加禁用」清单，新会话 / 新房间生效
// ============================================================

type Scene = 'session' | 'groupchat';
type Risk = 'Low' | 'Medium' | 'High';

interface ToolCatalogItem {
  name: string;
  description: string;
  /** low / medium / high */
  risk: string;
  /** ToolTag 变体名：Filesystem / Web / Exec / Image / Agent / Interaction / Read / Write / Execute / Network / HighCost / Interactive / Mcp */
  tags: string[];
  session_enabled: boolean;
  session_locked: boolean;
  groupchat_enabled: boolean;
  groupchat_locked: boolean;
}

interface ToolOverrides {
  session: string[];
  groupchat: string[];
}

const TAG_LABELS: Record<string, string> = {
  Filesystem: '文件系统',
  Web: '网络',
  Exec: '命令执行',
  Image: '图像',
  Audio: '语音',
  Mcp: 'MCP',
  Agent: '代理',
  Interaction: '人机交互',
  Read: '只读',
  Write: '写入',
  Execute: '执行',
  Network: '联网',
  HighCost: '高成本',
  Interactive: '交互',
};

const RISK_LABELS: Record<Risk, string> = { Low: '低', Medium: '中', High: '高' };
const RISKS: Risk[] = ['Low', 'Medium', 'High'];

/**
 * 工具卡片描述：默认折叠为 3 行，溢出时按钮「详细」直接拼接在内容末尾（文本流内联），
 * 不挤占水平显示区、不遮挡描述内容；展开后按钮「收起」跟随全文末尾。
 */
function ToolDescription({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  const [overflow, setOverflow] = useState(false);
  const [clamped, setClamped] = useState('');
  // 隐藏测量容器：与可见区同宽同字号，用于判断溢出并二分求 3 行内最大前缀。
  const measureRef = useRef<HTMLParagraphElement>(null);
  // 折叠按钮「详细」+ 间距预留宽度：14px（实测 10px 仍有换行），避免拼接后换行。
  const BUTTON_RESERVE = 14;

  useEffect(() => {
    const el = measureRef.current;
    if (!el) return;
    const check = () => {
      el.style.setProperty('-webkit-line-clamp', '3');
      // 预留折叠按钮宽度：截断文本与按钮拼接后仍落在 3 行内
      el.style.paddingRight = `${BUTTON_RESERVE}px`;
      el.textContent = text;
      void el.offsetHeight; // 强制 reflow 后再测量
      if (el.scrollHeight <= el.clientHeight + 1) {
        setOverflow(false);
        setClamped('');
        return;
      }
      setOverflow(true);
      // 二分查找：3 行内可完整容纳的最长前缀
      let lo = 0;
      let hi = text.length;
      let best = 0;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        el.textContent = text.slice(0, mid);
        void el.offsetHeight;
        if (el.scrollHeight <= el.clientHeight + 1) {
          best = mid;
          lo = mid + 1;
        } else {
          hi = mid - 1;
        }
      }
      setClamped(text.slice(0, best));
    };
    check();
    window.addEventListener('resize', check);
    return () => window.removeEventListener('resize', check);
  }, [text]);

  return (
    <div className="relative">
      {/* 测量容器：不可见、不占位，与可见描述同宽同排版 */}
      <p
        ref={measureRef}
        aria-hidden
        className="invisible absolute top-0 left-0 w-full text-[11px] leading-relaxed pointer-events-none"
        style={{
          display: '-webkit-box',
          WebkitLineClamp: 3,
          WebkitBoxOrient: 'vertical',
          overflow: 'hidden',
        }}
      />
      <p className="text-[11px] mt-1 leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
        {expanded || !overflow ? text : clamped}
        {overflow && (
          <button
            onClick={() => setExpanded((v) => !v)}
            className="inline-block ml-1 align-baseline text-[10px] hover:opacity-80"
            style={{ color: 'var(--accent)' }}
          >
            {expanded ? '收起' : '详细'}
          </button>
        )}
      </p>
    </div>
  );
}

const capRisk = (r: string): Risk => (r === 'high' ? 'High' : r === 'medium' ? 'Medium' : 'Low');

export function ToolSettings() {
  const [scene, setScene] = useState<Scene>('session');
  const [query, setQuery] = useState('');
  const [riskFilter, setRiskFilter] = useState<Risk | ''>('');
  const [tagFilter, setTagFilter] = useState('');
  const [catalog, setCatalog] = useState<ToolCatalogItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [saving, setSaving] = useState(false);
  const [savedTip, setSavedTip] = useState<'' | 'saved' | 'error'>('');
  // 用户「追加禁用」的工具名集合（持久化 overrides 驱动）
  const [sessionDisabled, setSessionDisabled] = useState<Set<string>>(new Set());
  const [groupchatDisabled, setGroupchatDisabled] = useState<Set<string>>(new Set());

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError('');
    try {
      const [cat, ov] = await Promise.all([
        invoke<ToolCatalogItem[]>('tool_catalog'),
        invoke<ToolOverrides>('get_tool_overrides'),
      ]);
      setCatalog(cat);
      setSessionDisabled(new Set(ov.session ?? []));
      setGroupchatDisabled(new Set(ov.groupchat ?? []));
    } catch (e) {
      setLoadError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // effect 体内不允许同步 setState（react-hooks/set-state-in-effect）：把首次加载推迟一个微任务，
    // 仍在同一帧内执行，观感与原先一致
    queueMicrotask(() => { void load(); });
  }, [load]);

  const sceneItems = useMemo(() => {
    return catalog
      .filter((t) => !query.trim() || t.name.includes(query.trim()) || t.description.includes(query.trim()))
      .filter((t) => !riskFilter || capRisk(t.risk) === riskFilter)
      .filter((t) => !tagFilter || t.tags.includes(tagFilter));
  }, [catalog, query, riskFilter, tagFilter]);

  const disabledSet = scene === 'session' ? sessionDisabled : groupchatDisabled;

  // 当前场景的目录状态（catalog 默认状态 + 用户追加禁用合并）
  const isLocked = (t: ToolCatalogItem) => (scene === 'session' ? t.session_locked : t.groupchat_locked);
  const defaultEnabled = (t: ToolCatalogItem) => (scene === 'session' ? t.session_enabled : t.groupchat_enabled);
  const isDisabled = (t: ToolCatalogItem) =>
    isLocked(t) || !defaultEnabled(t) || disabledSet.has(t.name);

  const toggle = (name: string) => {
    const setter = scene === 'session' ? setSessionDisabled : setGroupchatDisabled;
    setter((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
    setSavedTip('');
  };

  const allTags = useMemo(() => {
    const s = new Set<string>();
    catalog.forEach((t) => t.tags.forEach((x) => s.add(x)));
    return Array.from(s);
  }, [catalog]);

  const stats = useMemo(() => {
    const locked = catalog.filter((t) => isLocked(t)).length;
    const off = catalog.filter((t) => isDisabled(t)).length;
    return { total: catalog.length, locked, off, on: catalog.length - off };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [catalog, scene, sessionDisabled, groupchatDisabled]);

  const handleSave = async () => {
    setSaving(true);
    setSavedTip('');
    try {
      const overrides: ToolOverrides = {
        session: Array.from(sessionDisabled),
        groupchat: Array.from(groupchatDisabled),
      };
      await invoke('set_tool_overrides', { overrides });
      setSavedTip('saved');
    } catch (e) {
      console.error('保存工具覆盖失败', e);
      setSavedTip('error');
    } finally {
      setSaving(false);
      setTimeout(() => setSavedTip(''), 2000);
    }
  };

  const handleReset = async () => {
    setSaving(true);
    setSavedTip('');
    try {
      await invoke('set_tool_overrides', { overrides: { session: [], groupchat: [] } });
      setSessionDisabled(new Set());
      setGroupchatDisabled(new Set());
      setSavedTip('saved');
    } catch (e) {
      console.error('重置工具覆盖失败', e);
      setSavedTip('error');
    } finally {
      setSaving(false);
      setTimeout(() => setSavedTip(''), 2000);
    }
  };

  // 不透明色块背景（风险色本身，深色/浅色模式下均醒目协调）
  const riskBg = (r: string) =>
    r === 'high' ? '#dc2626' : r === 'medium' ? '#d97706' : '#16a34a';

  return (
    <div className="space-y-4">
      {/* 场景切换 */}
      <div className="flex items-center gap-1">
        {(['session', 'groupchat'] as Scene[]).map((s) => (
          <button
            key={s}
            onClick={() => setScene(s)}
            className={'pd-tab' + (scene === s ? ' pd-tab-active' : '')}
          >
            {s === 'session' ? '会话模式' : '群聊模式'}
          </button>
        ))}
        <span className="ml-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          共 {stats.total} 项 · 启用 {stats.on} · 禁用 {stats.off} · 锁定 {stats.locked}
        </span>
      </div>

      {/* 筛选工具栏 */}
      <div className="flex items-center gap-2 flex-wrap">
        <div className="pd-field flex-1 min-w-[160px] flex items-center gap-1.5 px-2 py-1.5 rounded-lg" style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}>
          <Search size={12} style={{ color: 'var(--text-tertiary)' }} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索工具名称或说明..."
            className="flex-1 bg-transparent outline-none text-xs"
            style={{ color: 'var(--text-primary)' }}
          />
        </div>
        <Select
          value={riskFilter}
          onChange={(v) => setRiskFilter(v as Risk | '')}
          options={[
            { value: '', label: '全部风险' },
            ...RISKS.map((r) => ({ value: r, label: `风险：${RISK_LABELS[r]}` })),
          ]}
          placeholder="全部风险"
          size="sm"
        />
        <Select
          value={tagFilter}
          onChange={(v) => setTagFilter(v)}
          options={[
            { value: '', label: '全部分类' },
            ...allTags.map((t) => ({ value: t, label: TAG_LABELS[t] ?? t })),
          ]}
          placeholder="全部分类"
          size="sm"
        />
      </div>

      {/* 操作按钮 */}
      <div className="flex items-center gap-2">
        <button
          onClick={handleSave}
          disabled={saving || loading}
          className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors disabled:opacity-50"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
          保存修改
        </button>
        <button
          onClick={handleReset}
          disabled={saving || loading}
          className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs transition-colors disabled:opacity-50"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
        >
          <RotateCcw size={12} />
          重置（恢复默认）
        </button>
        {savedTip === 'saved' && (
          <span className="text-xs" style={{ color: 'var(--success)' }}>已保存（新会话生效）</span>
        )}
        {savedTip === 'error' && (
          <span className="text-xs" style={{ color: 'var(--danger)' }}>保存失败，请重试</span>
        )}
      </div>

      {loadError && (
        <div className="px-3 py-2 rounded-lg text-xs" style={{ backgroundColor: 'rgba(239,68,68,0.08)', color: 'var(--danger)', border: '1px solid rgba(239,68,68,0.2)' }}>
          加载工具目录失败：{loadError}
        </div>
      )}

      {/* 工具列表（两列卡片，减少滚动量） */}
      {loading ? (
        <div className="flex items-center justify-center py-10">
          <Loader2 size={20} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-2">
          {sceneItems.map((t) => {
            const locked = isLocked(t);
            const off = isDisabled(t);
            return (
              <div
                key={t.name}
                className="px-3 py-2.5 rounded-lg transition-colors"
                style={{
                  backgroundColor: 'var(--bg-tertiary)', // 与 SettingsCard（环境检测等 tab）统一
                  border: '1px solid transparent',
                  opacity: off && !locked ? 0.7 : 1,
                }}
              >
                {/* 第一行：名称 + 风险 + 锁定 ｜ 开关在行尾（ml-auto），不占用下方行的宽度 */}
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium font-mono min-w-0 truncate" style={{ color: off ? 'var(--text-tertiary)' : 'var(--text-primary)' }}>
                    {t.name}
                  </span>
                  <span
                    className="shrink-0 px-1.5 py-0.5 rounded text-[10px]"
                    style={{ backgroundColor: riskBg(t.risk), color: '#fff' }}
                  >
                    {RISK_LABELS[capRisk(t.risk)]}风险
                  </span>
                  {locked && (
                    <span className="shrink-0 flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                      <Lock size={9} />
                      锁定
                    </span>
                  )}
                  <button
                    onClick={() => !locked && toggle(t.name)}
                    disabled={locked}
                    title={locked ? '架构性禁用，不可启用' : off ? '点击启用' : '点击禁用'}
                    className="ml-auto relative shrink-0 w-9 h-5 rounded-full transition-colors disabled:opacity-50"
                    style={{
                      backgroundColor: off ? 'var(--bg-tertiary)' : 'var(--accent)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    <span
                      className="absolute top-0.5 w-3.5 h-3.5 rounded-full transition-all"
                      style={{
                        backgroundColor: off ? 'var(--text-tertiary)' : '#fff',
                        left: off ? 3 : 18,
                      }}
                    />
                  </button>
                </div>
                {/* 描述：独立块级元素，占满整行；默认折叠 4 行，溢出时「详细 / 收起」切换 */}
                <ToolDescription text={t.description} />
                {/* 标签：同样占满整行 */}
                <div className="flex items-center gap-1 mt-1.5 flex-wrap">
                  {t.tags.map((tag) => (
                    <span key={tag} className="px-1.5 py-0.5 rounded text-[10px]" style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-tertiary)', border: '1px solid var(--border)' }}>
                      {TAG_LABELS[tag] ?? tag}
                    </span>
                  ))}
                </div>
              </div>
            );
          })}
          {sceneItems.length === 0 && !loading && (
            <div className="text-center py-8 col-span-2">
              <Wrench size={20} style={{ color: 'var(--text-tertiary)' }} className="mx-auto mb-2" />
              <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>没有匹配的工具</p>
            </div>
          )}
        </div>
      )}

      {/* 底部说明 */}
      <p className="text-[11px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
        - 工具开关仅在「{scene === 'session' ? '会话模式' : '群聊模式'}」生效；修改将在<b>新会话 / 新房间</b>启动时生效，当前会话不受影响。
        <br />
        - 锁定的工具为架构性禁用（如群聊中的子代理/记忆/任务等，与其场景语义冲突），不可启用。
        <br />
        - 「mcp:*」控制 MCP 整体启用：会话与群聊均默认开启（具体服务器在「MCP 服务器」设置管理，群聊为房间级共享连接池），可按场景整体禁用。
        <br />
        - 「file_history」为文件快照记录能力（write_file/edit_file 自动触发，非 Agent 可调用工具）。
        <br />
        - 技能（skills）由「技能目录」设置管理，不在本页开关范围。
      </p>
    </div>
  );
}
