import { useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { Settings, HelpCircle, Workflow as WorkflowIcon } from 'lucide-react';
import type { WorkflowDefinition, TriggerConfig } from '../../types/workflow';
import type { JsonValue } from '../../types/plugin';
import { Select } from '../common/Select';
import { ProjectSelect } from '../project/ProjectPicker';
import { WorkCreateDialogShell } from '../project/WorkCreateDialogShell';
import { useProjectStore } from '../../stores/projectStore';
import { errorMessage } from '../../utils/errorMessage';
import { projectLinkedDirectories, projectPrimaryDirectory } from '../../utils/project';

/** 定时任务数量的技术保护上限（与后端 `commands::workflow::MAX_SCHEDULES` 对齐） */
const MAX_SCHEDULES = 1000;

/** 入参字段规格（与 types/workflow.ts 的 inputSchema 一致；default 可能来自用户 JSON，形态未知） */
type InputSchemaField = { type: string; description?: string; required?: boolean; default?: JsonValue };
/** 产出字段规格（与 types/workflow.ts 的 outputSchema 一致） */
type OutputSchemaField = { type: string; description?: string };

interface Props {
  mode: 'create' | 'edit';
  initial?: Partial<WorkflowDefinition>;
  onConfirm: (data: {
    name: string;
    description: string;
    version: string;
    trigger: TriggerConfig;
    enabled: boolean;
    icon?: string | null;
    projectId?: string;
    inputSchema?: Record<string, InputSchemaField>;
    outputSchema?: Record<string, OutputSchemaField>;
  }) => void | Promise<void>;
  onClose: () => void;
}

// ── 预设 emoji 图标 ──
//
// 每组固定 5 个：外层是 `grid grid-cols-2`，每组占半行，加到 6 个会撑破行宽。
// 换图标时**按组 1:1 替换**，别只往上加。
// 本轮替换（把低频项换成更高频的内容/业务场景）：
//   ✏️ → 📰   内容/创作：✏️ 与同组 📝 语义重复（都是"写"）
//   🔍 → 🧾   数据/报表：检索由页面筛选承担；🧾 补单据/账单这类结构化凭证
//   🛡️ → 📡   自动化/运维：安全风控在建工作流时很少用来当标识；📡 表示采集与监控
//   🐛 → 📱   开发/技术：调试很少作为工作流主题；📱 表移动端场景
//   ⭐ → 🎟️   通用/其他：收藏/评分已由界面本身承担，不该占图标位；🎟️ 表票券/兑换码
const EMOJI_GROUPS: Array<{ label: string; emojis: string[] }> = [
  { label: 'AI/Agent', emojis: ['🤖', '🧠', '💬', '✨', '⚡'] },
  { label: '数据/报表', emojis: ['📊', '📈', '📋', '🧾', '📁'] },
  { label: '自动化/运维', emojis: ['⚙️', '🔔', '🚀', '🔄', '📡'] },
  { label: '内容/创作', emojis: ['📰', '📝', '🎨', '📚', '🎬'] },
  { label: '开发/技术', emojis: ['💻', '🧪', '📱', '🔧', '🧩'] },
  { label: '通用/其他', emojis: ['🌐', '📌', '🎟️', '💡', '🔗'] },
];

export function WorkflowPropertyDialog({ mode, initial, onConfirm, onClose }: Props) {
  const [name, setName] = useState(initial?.name || '');
  const [description, setDescription] = useState(initial?.description || '');
  const [version, setVersion] = useState(initial?.version || '1.0.0');
  const [icon, setIcon] = useState<string | undefined>(initial?.icon ?? undefined);
  const [projectId, setProjectId] = useState(initial?.projectId ?? '');
  const projects = useProjectStore((state) => state.projects);
  const selectedProject = projects.find((project) => project.id === projectId);
  const selectedProjectDirectory = projectPrimaryDirectory(selectedProject);
  const [triggerType, setTriggerType] = useState<'manual' | 'cron' | 'event'>(initial?.trigger?.triggerType || 'manual');
  const [cronExpr, setCronExpr] = useState(initial?.trigger?.cron || '');
  const [eventName, setEventName] = useState(initial?.trigger?.eventName || '');
  const [events, setEvents] = useState<Array<{ name: string; label: string }>>([]);
  /** 清单加载失败（例如运行中的应用还是旧版本、没有 list_workflow_events 命令）：
   *  此时不能一直显示"加载中…"，那会让人以为在等数据。 */
  const [eventsFailed, setEventsFailed] = useState(false);
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [inputSchemaText, setInputSchemaText] = useState(initial?.inputSchema ? JSON.stringify(initial.inputSchema, null, 2) : '');
  const [outputSchemaText, setOutputSchemaText] = useState(initial?.outputSchema ? JSON.stringify(initial.outputSchema, null, 2) : '');
  const [error, setError] = useState<string | null>(null);
  const [showCronHelp, setShowCronHelp] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(Boolean(
    initial?.inputSchema || initial?.outputSchema,
  ));
  const [showIconPicker, setShowIconPicker] = useState(false);
  const [iconPickerStyle, setIconPickerStyle] = useState<CSSProperties>({});
  const iconPickerTriggerRef = useRef<HTMLButtonElement>(null);
  const iconPickerPanelRef = useRef<HTMLDivElement>(null);

  const openIconPicker = () => {
    if (showIconPicker) {
      setShowIconPicker(false);
      return;
    }

    const trigger = iconPickerTriggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(430, window.innerWidth - 24);
    const left = Math.min(Math.max(12, rect.left), window.innerWidth - width - 12);
    const spaceBelow = window.innerHeight - rect.bottom - 12;
    const spaceAbove = rect.top - 12;
    const openUp = spaceBelow < 320 && spaceAbove > spaceBelow;
    const maxHeight = Math.max(180, Math.min(420, openUp ? spaceAbove : spaceBelow));

    setIconPickerStyle({
      position: 'fixed',
      zIndex: 100,
      left,
      width,
      maxHeight,
      ...(openUp
        ? { bottom: window.innerHeight - rect.top + 6 }
        : { top: rect.bottom + 6 }),
    });
    setShowIconPicker(true);
  };

  useEffect(() => {
    if (!showIconPicker) return;

    const closeOnOutsidePointer = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        iconPickerTriggerRef.current?.contains(target)
        || iconPickerPanelRef.current?.contains(target)
      ) return;
      setShowIconPicker(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      setShowIconPicker(false);
    };
    const closeOnViewportChange = (event: Event) => {
      if (event.type === 'scroll' && iconPickerPanelRef.current?.contains(event.target as Node)) return;
      setShowIconPicker(false);
    };

    document.addEventListener('mousedown', closeOnOutsidePointer);
    document.addEventListener('keydown', closeOnEscape, true);
    document.addEventListener('scroll', closeOnViewportChange, true);
    window.addEventListener('resize', closeOnViewportChange);
    return () => {
      document.removeEventListener('mousedown', closeOnOutsidePointer);
      document.removeEventListener('keydown', closeOnEscape, true);
      document.removeEventListener('scroll', closeOnViewportChange, true);
      window.removeEventListener('resize', closeOnViewportChange);
    };
  }, [showIconPicker]);

  // 事件名清单来自后端内置事件表（只有列在这里的事件才会被真正派发，
  // 因此不给自由输入——否则用户会订阅到一个永远不会响的名字）
  useEffect(() => {
    let cancelled = false;
    invoke<Array<{ name: string; label: string }>>('list_workflow_events')
      .then((list) => { if (!cancelled) setEvents(list); })
      .catch((e) => {
        if (cancelled) return;
        setEventsFailed(true);
        console.warn('[WorkflowPropertyDialog] 加载内置事件清单失败:', e);
      });
    return () => { cancelled = true; };
  }, []);

  // 定时任务数量只受技术保护上限约束（不再按会员档位配额）。在设置「定时触发」时提前告知已用/上限，
  // 免得用户填完 Cron 保存时才被后端拒绝。
  const scheduleLimit = MAX_SCHEDULES;
  const [scheduleCount, setScheduleCount] = useState<number | null>(null);
  useEffect(() => {
    let cancelled = false;
    invoke<unknown[]>('list_schedules')
      .then((rows) => { if (!cancelled) setScheduleCount(rows.length); })
      .catch(() => { /* 读取失败则不显示用量提示，不打断编辑 */ });
    return () => { cancelled = true; };
  }, []);

  const CRON_PRESETS = [
    { label: '每日9点', expr: '0 0 9 * * *' },
    { label: '工作日9点', expr: '0 0 9 * * 1-5' },
    { label: '每30分', expr: '0 */30 * * * *' },
    { label: '每月1日9点', expr: '0 0 9 1 * *' },
    { label: '周一9点', expr: '0 0 9 * * 1' },
  ];

  /** Cron 的"几点几分"按本机时区解释（后端调度器同一口径），显示出来免得口径不明 */
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || '本机时区';

  /**
   * Cron 帮助面板定位。
   *
   * 弹窗正文是滚动容器（overflow-y-auto），内联的 absolute 面板会被它裁掉，所以这里用
   * position: fixed 定位，按需要**向上展开**（贴在图标的上面），并把可用空间写进 maxHeight。
   * 关闭用 120ms 延时——图标与面板之间有 6px 间隙，鼠标穿过时不该立刻消失。
   */
  const cronHelpAnchorRef = useRef<HTMLDivElement>(null);
  const [cronHelpStyle, setCronHelpStyle] = useState<CSSProperties>({});
  const cronHelpTimer = useRef<number | null>(null);

  const openCronHelp = () => {
    if (cronHelpTimer.current !== null) {
      window.clearTimeout(cronHelpTimer.current);
      cronHelpTimer.current = null;
    }
    const rect = cronHelpAnchorRef.current?.getBoundingClientRect();
    if (rect) {
      const width = Math.min(420, window.innerWidth - 16);
      const spaceAbove = rect.top - 12;
      const spaceBelow = window.innerHeight - rect.bottom - 12;
      const upward = spaceAbove >= 200 || spaceAbove >= spaceBelow;
      const maxHeight = Math.max(160, Math.min(upward ? spaceAbove - 6 : spaceBelow - 6, window.innerHeight * 0.7));
      setCronHelpStyle({
        position: 'fixed',
        right: Math.max(8, window.innerWidth - rect.right),
        ...(upward
          ? { bottom: window.innerHeight - rect.top + 6 }
          : { top: rect.bottom + 6 }),
        width,
        maxHeight,
      });
    }
    setShowCronHelp(true);
  };

  const scheduleCloseCronHelp = () => {
    if (cronHelpTimer.current !== null) window.clearTimeout(cronHelpTimer.current);
    cronHelpTimer.current = window.setTimeout(() => setShowCronHelp(false), 120);
  };

  // 卸载时清掉待触发的关闭定时器，避免弹窗已关仍在改状态
  useEffect(() => () => {
    if (cronHelpTimer.current !== null) window.clearTimeout(cronHelpTimer.current);
  }, []);

  const handleConfirm = async () => {
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError('请输入工作流名称');
      return;
    }
    if (triggerType === 'cron' && !cronExpr.trim()) {
      setError('定时触发器需要填写 Cron 表达式');
      return;
    }
    if (triggerType === 'event' && !eventName) {
      setError('事件触发器需要选择事件名');
      return;
    }
    setError(null);

    // 解析 input/output schema JSON
    let inputSchema: Record<string, InputSchemaField> | undefined;
    let outputSchema: Record<string, OutputSchemaField> | undefined;
    if (inputSchemaText.trim()) {
      try { inputSchema = JSON.parse(inputSchemaText); }
      catch { setError('输入 Schema JSON 格式错误'); return; }
    }
    if (outputSchemaText.trim()) {
      try { outputSchema = JSON.parse(outputSchemaText); }
      catch { setError('输出 Schema JSON 格式错误'); return; }
    }

    setSubmitting(true);
    try {
      await onConfirm({
        name: trimmedName,
        description: description.trim(),
        version: version.trim() || '1.0.0',
        icon: icon ?? null,
        projectId: projectId || undefined,
        trigger: triggerType === 'manual'
          ? { triggerType: 'manual' }
          : triggerType === 'cron'
            ? { triggerType: 'cron', cron: cronExpr.trim() }
            : { triggerType: 'event', eventName },
        inputSchema,
        outputSchema,
        enabled,
      });
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  };

  const insertCronPreset = (expr: string) => {
    setCronExpr(expr);
    setError(null);
  };

  return (
    <>
      <WorkCreateDialogShell
        icon={mode === 'create'
          ? <WorkflowIcon size={14} />
          : <Settings size={14} />}
        kicker="工作流"
        title={mode === 'create' ? '新建工作流' : '编辑工作流属性'}
        project={selectedProject ? {
          name: selectedProject.name,
          icon: selectedProject.icon,
          primaryDirectory: selectedProjectDirectory,
          linkedDirectoryCount: projectLinkedDirectories(selectedProject).length,
        } : null}
        submitting={submitting}
        onClose={onClose}
        onSubmit={(event) => {
          event.preventDefault();
          void handleConfirm();
        }}
        submitLabel={mode === 'create' ? '创建' : '保存'}
        submittingLabel={mode === 'create' ? '创建中…' : '保存中…'}
        submitIcon={mode === 'create'
          ? <WorkflowIcon size={12} />
          : <Settings size={12} />}
        headerAction={(
          <div className="work-create-header-toggle">
            <span>{enabled ? '已启用' : '已禁用'}</span>
            <button
              type="button"
              role="switch"
              aria-checked={enabled}
              aria-label={enabled ? '禁用工作流' : '启用工作流'}
              onClick={() => setEnabled((value) => !value)}
              className="work-create-switch"
              data-active={enabled}
              disabled={submitting}
            >
              <span />
            </button>
          </div>
        )}
        error={error}
      >
        <>
        <label className="projects-field">
          <span>项目归属 <small>可选</small></span>
          <ProjectSelect value={projectId} onChange={setProjectId} />
        </label>

        <div className="work-create-section">
          <div className="work-create-section-head">
            <strong>基础信息</strong>
            <span>名称、版本与图标会显示在工作流列表中</span>
          </div>
          {/* 图标紧邻名称输入框，版本号继续并行以节省纵向空间 */}
          <div className="work-create-title-row">
            <button
              ref={iconPickerTriggerRef}
              type="button"
              className="work-create-title-icon-button"
              data-open={showIconPicker}
              aria-label={icon ? `当前图标 ${icon}，点击更换` : '选择工作流图标'}
              aria-haspopup="listbox"
              aria-expanded={showIconPicker}
              onClick={openIconPicker}
            >
              {icon || <WorkflowIcon size={15} />}
            </button>
            <div className="min-w-0">
              <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                工作流名称 <span style={{ color: '#EF4444' }}>*</span>
              </label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="w-full px-3 py-2 text-xs rounded-lg outline-none transition-colors"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-primary)',
                  border: error && !name.trim() ? '1px solid #EF4444' : '1px solid var(--border)',
                }}
                placeholder="输入工作流名称"
                autoFocus
              />
            </div>
            <div className="min-w-0">
              <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                版本号
              </label>
              <input
                value={version}
                onChange={(e) => setVersion(e.target.value)}
                className="w-full px-3 py-2 text-xs rounded-lg outline-none"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-primary)',
                  border: '1px solid var(--border)',
                }}
                placeholder="1.0.0"
              />
            </div>
          </div>

          {/* 描述 */}
          <div>
            <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
              描述
            </label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              className="w-full px-3 py-2 text-xs rounded-lg outline-none resize-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
                minHeight: 60,
              }}
              placeholder="工作流描述（可选）"
            />
          </div>

        </div>

        <div className="work-create-section">
          <div className="work-create-section-head">
            <strong>触发方式</strong>
            <span>手动运行、按 Cron 定时运行，或响应平台事件</span>
          </div>
          <div>
            <div className="flex gap-2">
              {([
                { value: 'manual', label: '手动触发' },
                { value: 'cron', label: '定时触发' },
                { value: 'event', label: '事件触发' },
              ] as const).map((opt) => (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => setTriggerType(opt.value)}
                  className="flex-1 px-3 py-2 text-xs rounded-lg transition-colors"
                  style={{
                    backgroundColor: triggerType === opt.value ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                    color: triggerType === opt.value ? 'var(--accent)' : 'var(--text-secondary)',
                    border: triggerType === opt.value ? '1px solid var(--accent)' : '1px solid var(--border)',
                  }}
                >
                  {opt.label}
                </button>
              ))}
            </div>
            {triggerType === 'cron' && (
              <p
                className="mt-1.5 text-[11px]"
                style={{
                  color:
                    scheduleCount !== null && scheduleCount >= scheduleLimit
                      ? 'var(--status-warning)'
                      : 'var(--text-tertiary)',
                }}
              >
                定时任务：已用 {scheduleCount ?? '—'} / 上限 {scheduleLimit}
                {scheduleCount !== null && scheduleCount >= scheduleLimit ? '（系统保护上限，请先清理不再使用的定时任务）' : ''}
              </p>
            )}
            {triggerType === 'event' && (
              <div className="mt-2">
                <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                  事件名 <span style={{ color: '#EF4444' }}>*</span>
                </label>
                <Select
                  value={eventName}
                  onChange={setEventName}
                  options={events.map(e => ({ value: e.name, label: `${e.label}（${e.name}）` }))}
                  placeholder={
                    events.length > 0
                      ? '请选择要订阅的事件'
                      : eventsFailed
                        ? '内置事件加载失败，请重启应用后重试'
                        : '加载内置事件中…'
                  }
                  disabled={events.length === 0}
                  className="w-full"
                  title="事件触发：平台在内置事件发生时启动本工作流"
                />
                <div className="mt-1 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                  仅支持平台内置事件；事件负载会作为本工作流的输入数据，
                  模板中可用 <span className="font-mono">{'{{sessionId}}'}</span>、
                  <span className="font-mono">{'{{definitionId}}'}</span>、
                  <span className="font-mono">{'{{executionId}}'}</span> 等字段。
                  工作流自身完成的事件不会触发它自己。
                </div>
              </div>
            )}
            {triggerType === 'cron' && (
              <div className="mt-2">
                {/* 标题行：标签 + 快捷插入 + 帮助图标 */}
                <div className="flex items-center gap-2 mb-1.5">
                  <label className="text-xs font-medium shrink-0" style={{ color: 'var(--text-secondary)' }}>
                    Cron 表达式 <span style={{ color: '#EF4444' }}>*</span>
                  </label>
                  <div className="flex-1 flex items-center gap-1 overflow-x-auto">
                    {CRON_PRESETS.map((preset) => (
                      <button
                        key={preset.expr}
                        type="button"
                        onClick={() => insertCronPreset(preset.expr)}
                        className="shrink-0 px-1.5 py-0.5 rounded text-[10px] transition-colors"
                        style={{
                          border: '1px solid var(--border)',
                          background: 'var(--bg-tertiary)',
                          color: 'var(--text-tertiary)',
                        }}
                        title={preset.expr}
                      >
                        {preset.label}
                      </button>
                    ))}
                  </div>
                  {/* 帮助图标：面板向上展开（见 openCronHelp） */}
                  <div className="relative shrink-0" ref={cronHelpAnchorRef}>
                    <button
                      type="button"
                      onMouseEnter={openCronHelp}
                      onMouseLeave={scheduleCloseCronHelp}
                      className="pd-btn p-0.5 rounded hover:opacity-80"
                      style={{ color: showCronHelp ? 'var(--accent)' : 'var(--text-tertiary)' }}
                    >
                      <HelpCircle size={14} />
                    </button>
                    {showCronHelp && (
                      <div
                        className="z-[300] overflow-y-auto rounded-lg shadow-xl p-4 text-[11px] leading-relaxed"
                        style={{
                          backgroundColor: 'var(--bg-primary)',
                          border: '1px solid var(--border)',
                          color: 'var(--text-primary)',
                          boxShadow: '0 12px 32px rgba(0,0,0,0.28)',
                          ...cronHelpStyle,
                        }}
                        onMouseEnter={openCronHelp}
                        onMouseLeave={scheduleCloseCronHelp}
                      >
                        <div className="font-medium mb-2">Cron 表达式详解</div>
                        <div className="space-y-2">
                          <div>
                            <div className="font-medium text-[10px]" style={{ color: 'var(--accent)' }}>标准格式（6字段）</div>
                            <pre className="text-[10px] mt-0.5 p-1.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
{`秒 分 时 日 月 星期
0  0  9  *  *  1-5`}
                            </pre>
                          </div>
                          <div>
                            <div className="font-medium text-[10px]" style={{ color: 'var(--accent)' }}>字段说明</div>
                            <table className="w-full text-[10px] mt-0.5" style={{ borderCollapse: 'collapse' }}>
                              <thead>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}>
                                  <th className="text-left pr-2 py-0.5" style={{ color: 'var(--text-tertiary)' }}>字段</th>
                                  <th className="text-left pr-2 py-0.5" style={{ color: 'var(--text-tertiary)' }}>范围</th>
                                  <th className="text-left py-0.5" style={{ color: 'var(--text-tertiary)' }}>特殊字符</th>
                                </tr>
                              </thead>
                              <tbody>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">秒</td><td className="pr-2 py-0.5">0-59</td><td className="py-0.5">, - * /</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">分钟</td><td className="pr-2 py-0.5">0-59</td><td className="py-0.5">, - * /</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">小时</td><td className="pr-2 py-0.5">0-23</td><td className="py-0.5">, - * /</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">日</td><td className="pr-2 py-0.5">1-31</td><td className="py-0.5">, - * / ? L W</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">月</td><td className="pr-2 py-0.5">1-12</td><td className="py-0.5">, - * /</td></tr>
                                <tr><td className="pr-2 py-0.5">星期</td><td className="pr-2 py-0.5">0-7</td><td className="py-0.5">, - * / ? L #</td></tr>
                              </tbody>
                            </table>
                          </div>
                          <div>
                            <div className="font-medium text-[10px]" style={{ color: 'var(--accent)' }}>特殊字符</div>
                            <table className="w-full text-[10px] mt-0.5" style={{ borderCollapse: 'collapse' }}>
                              <thead>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}>
                                  <th className="text-left pr-2 py-0.5" style={{ color: 'var(--text-tertiary)' }}>字符</th>
                                  <th className="text-left py-0.5" style={{ color: 'var(--text-tertiary)' }}>含义</th>
                                </tr>
                              </thead>
                              <tbody>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">*</td><td className="py-0.5">任意值（每）</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">,</td><td className="py-0.5">枚举多个值</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">-</td><td className="py-0.5">范围</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">/</td><td className="py-0.5">步进间隔</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">?</td><td className="py-0.5">不指定（日和星期互斥）</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">L</td><td className="py-0.5">最后一天/最后一个</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5">W</td><td className="py-0.5">最近的工作日</td></tr>
                                <tr><td className="pr-2 py-0.5">#</td><td className="py-0.5">第几个（如 3#2=第二个周二）</td></tr>
                              </tbody>
                            </table>
                          </div>
                          <div>
                            <div className="font-medium text-[10px]" style={{ color: 'var(--accent)' }}>常用示例</div>
                            <table className="w-full text-[10px] mt-0.5" style={{ borderCollapse: 'collapse' }}>
                              <thead>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}>
                                  <th className="text-left pr-2 py-0.5" style={{ color: 'var(--text-tertiary)' }}>表达式</th>
                                  <th className="text-left py-0.5" style={{ color: 'var(--text-tertiary)' }}>含义</th>
                                </tr>
                              </thead>
                              <tbody>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 9 * * *</td><td className="py-0.5">每天早上 9:00</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 18 * * *</td><td className="py-0.5">每天下午 6:00</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 9 * * 1-5</td><td className="py-0.5">工作日早上 9:00</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 */30 * * * *</td><td className="py-0.5">每 30 分钟</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 9 1 * *</td><td className="py-0.5">每月 1 号 9:00</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 9 * * 1</td><td className="py-0.5">每周一 9:00</td></tr>
                                <tr style={{ borderBottom: '1px solid var(--border)' }}><td className="pr-2 py-0.5 font-mono">0 0 0 L * ?</td><td className="py-0.5">每月最后一天午夜</td></tr>
                                <tr><td className="pr-2 py-0.5 font-mono">0 0 9 1,15 * *</td><td className="py-0.5">每月 1 日和 15 日 9:00</td></tr>
                              </tbody>
                            </table>
                          </div>
                          <div className="pt-1" style={{ borderTop: '1px solid var(--border)' }}>
                            <div className="font-medium text-[10px]" style={{ color: 'var(--accent)' }}>注意事项</div>
                            <ul className="text-[10px] mt-0.5 space-y-0.5" style={{ color: 'var(--text-tertiary)', paddingLeft: 12 }}>
                              <li>日和星期同时设置时是"或"关系，互斥时用 ?</li>
                              <li>*/10 从 0 开始：0,10,20,...；3/10 从 3 开始：3,13,23,...</li>
                              <li>夏令时切换日：春季少一小时（任务跳过），秋季多一小时（任务可能执行两次）</li>
                            </ul>
                          </div>
                        </div>
                      </div>
                    )}
                  </div>
                </div>
                <input
                  value={cronExpr}
                  onChange={(e) => setCronExpr(e.target.value)}
                  className="w-full px-3 py-2 text-xs rounded-lg outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: error && !cronExpr.trim() ? '1px solid #EF4444' : '1px solid var(--border)',
                  }}
                  placeholder="例如: 0 0 9 * * 1-5（工作日早9点）"
                />
                <p className="mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  格式: 秒 分 时 日 月 星期（时区: {timeZone}，可写 5 字段「分 时 日 月 星期」）
                </p>
              </div>
            )}
          </div>
        </div>

        <div className="work-create-section">
          <div className="work-create-section-head">
            <strong>高级配置</strong>
            <span>输入输出 Schema 用于约束运行参数，通常可留空</span>
          </div>

          <details
            className="work-create-advanced"
            open={advancedOpen}
            onToggle={(event) => setAdvancedOpen(event.currentTarget.open)}
          >
            <summary>Schema 配置（JSON，均可不填）</summary>
            <div className="work-create-advanced-body">
              <label className="projects-field">
                <span>
                输入 Schema
                <small>
                  入参字段：手动运行时弹表单填写（预填 default）；作为子工作流被调用时按 required / type 强制校验
                </small>
                </span>
                <textarea
                  value={inputSchemaText}
                  onChange={(e) => setInputSchemaText(e.target.value)}
                  placeholder='{"topic": {"type": "string", "required": true, "default": "人工智能"}}'
                  style={{ minHeight: 54, fontFamily: 'var(--font-mono)' }}
                />
              </label>

              <label className="projects-field">
                <span>
                输出 Schema
                <small>
                  产出字段：只保留列出的键（缺的补 null / default），供下游以{' '}
                  <span className="font-mono">{'{{节点.字段}}'}</span> 引用
                </small>
                </span>
                <textarea
                  value={outputSchemaText}
                  onChange={(e) => setOutputSchemaText(e.target.value)}
                  placeholder='{"summary": {"type": "string", "description": "最终摘要"}}'
                  style={{ minHeight: 54, fontFamily: 'var(--font-mono)' }}
                />
              </label>
            </div>
          </details>
        </div>

        </>
      </WorkCreateDialogShell>

      {showIconPicker && createPortal(
        <div
          ref={iconPickerPanelRef}
          id="workflow-icon-popover"
          className="work-create-icon-popover"
          style={iconPickerStyle}
          role="listbox"
          aria-label="选择工作流图标"
        >
          <button
            type="button"
            role="option"
            aria-selected={!icon}
            className="work-create-icon-default"
            data-active={!icon}
            onClick={() => {
              setIcon(undefined);
              setShowIconPicker(false);
            }}
          >
            <span className="work-create-icon-default-preview" aria-hidden="true">
              <WorkflowIcon size={14} />
            </span>
            <span>
              <strong>默认图标</strong>
              <small>未选择个性图标时使用</small>
            </span>
          </button>
          <div className="work-create-icon-grid">
            {EMOJI_GROUPS.map((group) => (
              <div key={group.label} className="work-create-icon-group">
                <span>{group.label}</span>
                <div>
                  {group.emojis.map((emoji) => (
                    <button
                      key={emoji}
                      type="button"
                      role="option"
                      aria-selected={icon === emoji}
                      onClick={() => {
                        setIcon(emoji);
                        setShowIconPicker(false);
                      }}
                      className="work-create-icon-option"
                      data-active={icon === emoji}
                      aria-label={`选择图标 ${emoji}`}
                    >
                      {emoji}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
