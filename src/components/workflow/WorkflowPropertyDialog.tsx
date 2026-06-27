import { useState } from 'react';
import { X, Settings, Plus, HelpCircle } from 'lucide-react';
import type { WorkflowDefinition, TriggerConfig } from '../types/workflow';

interface Props {
  mode: 'create' | 'edit';
  initial?: Partial<WorkflowDefinition>;
  onConfirm: (data: {
    name: string;
    description: string;
    version: string;
    trigger: TriggerConfig;
    enabled: boolean;
  }) => void;
  onClose: () => void;
}

export function WorkflowPropertyDialog({ mode, initial, onConfirm, onClose }: Props) {
  const [name, setName] = useState(initial?.name || '');
  const [description, setDescription] = useState(initial?.description || '');
  const [version, setVersion] = useState(initial?.version || '1.0.0');
  const [triggerType, setTriggerType] = useState<'manual' | 'cron' | 'event'>(initial?.trigger?.triggerType || 'manual');
  const [cronExpr, setCronExpr] = useState(initial?.trigger?.cron || '');
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [error, setError] = useState<string | null>(null);
  const [showCronHelp, setShowCronHelp] = useState(false);

  const CRON_PRESETS = [
    { label: '每日9点', expr: '0 0 9 * * *' },
    { label: '工作日9点', expr: '0 0 9 * * 1-5' },
    { label: '每30分', expr: '0 */30 * * * *' },
    { label: '每月1日9点', expr: '0 0 9 1 * *' },
    { label: '周一9点', expr: '0 0 9 * * 1' },
  ];

  const handleConfirm = () => {
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError('请输入工作流名称');
      return;
    }
    if (triggerType === 'cron' && !cronExpr.trim()) {
      setError('定时触发器需要填写 Cron 表达式');
      return;
    }
    setError(null);
    onConfirm({
      name: trimmedName,
      description: description.trim(),
      version: version.trim() || '1.0.0',
      trigger: triggerType === 'manual'
        ? { triggerType: 'manual' }
        : triggerType === 'cron'
          ? { triggerType: 'cron', cron: cronExpr.trim() }
          : { triggerType: 'event' },
      enabled,
    });
  };

  const insertCronPreset = (expr: string) => {
    setCronExpr(expr);
    setError(null);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[480px] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="flex items-center gap-2">
            <Settings size={16} style={{ color: 'var(--accent)' }} />
            <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
              {mode === 'create' ? '新建工作流' : '编辑工作流属性'}
            </span>
          </div>
          <button onClick={onClose} className="pd-btn p-1 rounded hover:opacity-80" style={{ color: 'var(--text-tertiary)' }}>
            <X size={14} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4">
          {/* 名称 */}
          <div>
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

          {/* 版本 */}
          <div>
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

          {/* 触发器 */}
          <div>
            <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
              触发器类型
            </label>
            <div className="flex gap-2">
              {([
                { value: 'manual', label: '手动触发' },
                { value: 'cron', label: '定时触发' },
                { value: 'event', label: '事件触发' },
              ] as const).map((opt) => (
                <button
                  key={opt.value}
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
                  {/* 帮助图标 */}
                  <div className="relative shrink-0">
                    <button
                      type="button"
                      onMouseEnter={() => setShowCronHelp(true)}
                      onMouseLeave={() => setShowCronHelp(false)}
                      className="pd-btn p-0.5 rounded hover:opacity-80"
                      style={{ color: 'var(--text-tertiary)' }}
                    >
                      <HelpCircle size={14} />
                    </button>
                    {showCronHelp && (
                      <div
                        className="absolute right-0 top-full mt-1 z-50 w-[420px] max-h-[60vh] overflow-y-auto rounded-lg shadow-xl p-4 text-[11px] leading-relaxed"
                        style={{
                          backgroundColor: 'var(--bg-primary)',
                          border: '1px solid var(--border)',
                          color: 'var(--text-primary)',
                        }}
                        onMouseEnter={() => setShowCronHelp(true)}
                        onMouseLeave={() => setShowCronHelp(false)}
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
                  格式: 秒 分 时 日 月 星期
                </p>
              </div>
            )}
          </div>

          {/* 启用状态 */}
          <div className="flex items-center gap-3">
            <label className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
              启用状态
            </label>
            <button
              onClick={() => setEnabled(!enabled)}
              className="relative w-9 h-5 rounded-full transition-colors"
              style={{
                backgroundColor: enabled ? 'var(--accent)' : 'var(--border)',
              }}
            >
              <div
                className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform shadow-sm"
                style={{ left: enabled ? '18px' : '2px' }}
              />
            </button>
            <span className="text-xs" style={{ color: enabled ? '#22c55e' : 'var(--text-tertiary)' }}>
              {enabled ? '已启用' : '已禁用'}
            </span>
          </div>

          {/* 错误提示 */}
          {error && (
            <div className="p-2 rounded text-xs" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
              {error}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-5 py-3 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
          <button
            onClick={onClose}
            className="pd-btn px-4 py-1.5 text-xs rounded"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            取消
          </button>
          <button
            onClick={handleConfirm}
            className="pd-btn px-4 py-1.5 text-xs rounded flex items-center gap-1.5"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            {mode === 'create' ? <Plus size={14} /> : <Settings size={14} />}
            {mode === 'create' ? '创建' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
