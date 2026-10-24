/**
 * Select — 全应用统一下拉选择器（替代原生 <select>）。
 *
 * 为什么自研：原生 <select> 的弹出层由浏览器绘制，样式无法统一（列表是直角、配色跟系统走），
 * 也不能指定展开方向 —— 在弹窗底部会因空间不足被裁掉选项。这里改为 portal 渲染的自绘面板：
 * - 面板圆角/边框/阴影与选择器本体一致，配色全部走主题变量；
 * - 依据可用空间自动向上/向下展开，空间不足时面板内滚动；
 * - 键盘支持（Esc 关闭、↑/↓ 移动、Enter 选中）+ 点击外部关闭；
 * - portal 到 body，避免被祖先的 overflow: hidden / transform 裁掉；
 * - 可选内置搜索（`searchable`）：选项多到无法靠滚动找（如知识库文件关系），
 *   打开即聚焦输入框，按 label 子串过滤；只渲染前 200 项，避免上千个按钮卡顿。
 *
 * 用法：把 `<select value={v} onChange={(e) => setV(e.target.value)}><option value="a">A</option></select>`
 * 换成 `<Select value={v} onChange={setV} options={[{ value: 'a', label: 'A' }]} />`。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, Search } from 'lucide-react';

export interface SelectOption {
  value: string;
  label: string;
  /** 选项前置图标（如门户标签图标）；只影响面板内的选项渲染，触发器仍只显示 label 文本 */
  icon?: ReactNode;
  disabled?: boolean;
}

/** 分组：组头只作分隔与说明，不可选（用于 CLI / API 这类二级选择） */
export interface SelectGroup {
  label: string;
  options: SelectOption[];
}

export interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  /** 平铺选项（与 groups 二选一） */
  options?: SelectOption[];
  /** 分组选项（与 options 二选一）；组头不可选，键盘上下键会跳过组头 */
  groups?: SelectGroup[];
  /** value 为空串时按钮上显示的占位文案 */
  placeholder?: string;
  disabled?: boolean;
  /** 附加到触发按钮（用于宽度等布局类名；外观由组件统一控制） */
  className?: string;
  /** 附加到触发按钮的行内样式（布局用） */
  style?: CSSProperties;
  /** 触发按钮尺寸：md（默认，与表单一致）/ sm（紧凑，工具栏与内联用）/ xs（与工具栏图标按钮齐高，h=24） */
  size?: 'md' | 'sm' | 'xs';
  title?: string;
  /** 展开方向偏好；默认 auto：下方空间不足且上方更宽裕时自动向上展开 */
  prefer?: 'auto' | 'up' | 'down';
  /** 面板最小宽度（px），默认与触发按钮同宽 */
  panelMinWidth?: number;
  /** 面板内显示搜索框（按 label 子串过滤）；选项很多时必开 */
  searchable?: boolean;
  /** 搜索框占位文案 */
  searchPlaceholder?: string;
}

const PANEL_MAX_H = 280;
const PANEL_MIN_H = 96;
const GAP = 4;
const VIEWPORT_PAD = 8;
const ITEM_H = 30;
/** 分组头行高（估算面板高度用） */
const GROUP_H = 22;
/** 可搜索时最多渲染的选项数：搜到的仍可继续输入缩小范围，避免上千个按钮卡顿 */
const MAX_RENDER = 200;

export function Select({
  value,
  onChange,
  options,
  groups,
  placeholder = '请选择',
  disabled,
  className = '',
  style,
  size = 'md',
  title,
  prefer = 'auto',
  panelMinWidth,
  searchable = false,
  searchPlaceholder = '搜索…',
}: SelectProps) {
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [query, setQuery] = useState('');
  const [rect, setRect] = useState<{ left: number; top: number; width: number; up: boolean; maxH: number } | null>(null);

  // 分组与平铺统一成一条扁平列表：键盘导航、active 下标、value 查找都基于它，
  // 渲染时再按组切分（分组头只是视觉分隔，不参与选中）。
  const flat = useMemo<SelectOption[]>(
    () => (groups && groups.length > 0 ? groups.flatMap((g) => g.options) : options ?? []),
    [groups, options],
  );
  const groupCount = groups?.length ?? 0;

  /** 搜索过滤后的选项（不搜索时就是 flat）；大小写不敏感的子串匹配 */
  const visible = useMemo(() => {
    if (!searchable) return flat;
    const q = query.trim().toLowerCase();
    if (!q) return flat;
    return flat.filter((o) => o.label.toLowerCase().includes(q));
  }, [flat, searchable, query]);
  /** 实际渲染 / 键盘可选的选项：搜索模式下截断，避免上千个 DOM 按钮卡顿 */
  const shown = useMemo(
    () => (searchable && visible.length > MAX_RENDER ? visible.slice(0, MAX_RENDER) : visible),
    [searchable, visible],
  );

  const current = flat.find((o) => o.value === value);

  const measure = useCallback(() => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - GAP - VIEWPORT_PAD;
    const above = r.top - GAP - VIEWPORT_PAD;
    const want = Math.min(
      PANEL_MAX_H,
      shown.length * ITEM_H + groupCount * GROUP_H + 8 + (searchable ? 34 : 0),
    );
    const up = prefer === 'up' ? true : prefer === 'down' ? false : below < want && above > below;
    const avail = up ? above : below;
    setRect({
      left: r.left,
      top: up ? r.top - GAP : r.bottom + GAP,
      width: Math.max(r.width, panelMinWidth ?? 0),
      up,
      maxH: Math.max(PANEL_MIN_H, Math.min(want, avail)),
    });
  }, [shown.length, groupCount, panelMinWidth, prefer, searchable]);

  // 打开时测量一次；面板渲染后再量一次，避免首帧高度估算不准
  useLayoutEffect(() => {
    if (open) measure();
  }, [open, measure]);

  useEffect(() => {
    if (!open) return;
    const onDocDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || panelRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false);
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActive((i) => Math.min(shown.length - 1, i + 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActive((i) => Math.max(0, i - 1));
      } else if (e.key === 'Enter') {
        const o = shown[active];
        if (o && !o.disabled) {
          onChange(o.value);
          setOpen(false);
        }
      }
    };
    /**
     * 外部滚动 / 尺寸变化时收起面板，避免面板与触发按钮脱位。
     *
     * 按事件源过滤：window 上的 scroll 是**捕获**注册的，而非冒泡的 scroll 事件在捕获阶段
     * 同样会从祖先收到 —— 于是面板自己滚动内容（滚轮或拖滚动条）也会命中这里，把面板当场
     * 收起（表现为"内容滚不动、面板还折叠了"）。源在面板或触发按钮内就放行。
     * （resize 事件的 target 是 window，`contains` 对非节点返回 false，因此照旧收起。）
     */
    const onScrollResize = (e: Event) => {
      const t = e.target as Node | null;
      if (t && (panelRef.current?.contains(t) || btnRef.current?.contains(t))) return;
      setOpen(false);
    };
    document.addEventListener('mousedown', onDocDown, true);
    document.addEventListener('keydown', onKey, true);
    window.addEventListener('scroll', onScrollResize, true);
    window.addEventListener('resize', onScrollResize);
    return () => {
      document.removeEventListener('mousedown', onDocDown, true);
      document.removeEventListener('keydown', onKey, true);
      window.removeEventListener('scroll', onScrollResize, true);
      window.removeEventListener('resize', onScrollResize);
    };
  }, [open, shown, active, onChange]);

  // 尺寸档：xs 与 InputBar/群聊等工具栏里的图标按钮（h=24）齐高，样式（内边距/字号/箭头）一并统一
  const h = size === 'md' ? 34 : size === 'sm' ? 28 : 24;
  const fs = size === 'sm' ? 11 : 12;
  const pad = size === 'md' ? '0 10px' : '0 8px';
  const chevron = size === 'md' ? 12 : 11;
  /**
   * 圆角走 token（--radius-lg），与全仓控件的 `rounded-lg` 同值。
   * 此前硬编码 8px：本仓库把 Tailwind 的 --radius-lg 覆盖成 12px，于是相邻的按钮/输入框
   * 都是 12px，只有下拉选择器是 8px —— 在 InputBar 工具栏里最明显（6 个控件 12px、它 8px）。
   */
  const radius = 'var(--radius-lg)';

  /** 单个选项（index 为扁平下标，决定键盘高亮与选中态） */
  const renderItem = (o: SelectOption, i: number, grouped: boolean) => (
    <button
      key={o.value}
      type="button"
      disabled={o.disabled}
      className="w-full flex items-center gap-1.5 text-left"
      style={{
        padding: grouped ? '6px 8px 6px 14px' : '6px 8px',
        fontSize: fs,
        borderRadius: 7,
        color: o.disabled
          ? 'var(--text-tertiary)'
          : o.value === value
            ? 'var(--accent)'
            : 'var(--text-primary)',
        backgroundColor: i === active ? 'var(--bg-tertiary)' : 'transparent',
        cursor: o.disabled ? 'not-allowed' : 'pointer',
      }}
      onMouseEnter={() => setActive(i)}
      onClick={() => {
        if (o.disabled) return;
        onChange(o.value);
        setOpen(false);
      }}
    >
      {o.icon}
      <span className="truncate min-w-0">{o.label}</span>
    </button>
  );

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        title={title}
        disabled={disabled}
        className={`flex items-center justify-between gap-2 text-left ${className}`}
        style={{
          height: h,
          padding: pad,
          fontSize: fs,
          borderRadius: radius,
          backgroundColor: 'var(--bg-field)',
          color: current ? 'var(--text-primary)' : 'var(--text-tertiary)',
          border: '1px solid var(--border)',
          opacity: disabled ? 0.6 : 1,
          cursor: disabled ? 'not-allowed' : 'pointer',
          ...style,
        }}
        onClick={() => {
          if (disabled) return;
          setOpen((v) => !v);
          setQuery('');
          setActive(Math.max(0, flat.findIndex((o) => o.value === value)));
        }}
      >
        <span className="truncate">{current ? current.label : placeholder}</span>
        <ChevronDown
          size={chevron}
          style={{
            flexShrink: 0,
            opacity: 0.7,
            transform: open ? 'rotate(180deg)' : 'none',
            transition: 'transform .15s',
          }}
        />
      </button>

      {open &&
        rect &&
        createPortal(
          <div
            ref={panelRef}
            className="fixed z-[200] overflow-y-auto"
            style={{
              left: rect.left,
              width: rect.width,
              ...(rect.up
                ? { bottom: window.innerHeight - rect.top }
                : { top: rect.top }),
              maxHeight: rect.maxH,
              padding: 4,
              borderRadius: radius,
              backgroundColor: 'var(--bg-secondary)',
              border: '1px solid var(--border)',
              boxShadow: '0 10px 28px rgba(0,0,0,0.22)',
            }}
          >
            {searchable && (
              <div className="px-1 pb-1">
                <div
                  className="flex items-center gap-1.5 px-2"
                  style={{
                    height: 26,
                    borderRadius: 7,
                    backgroundColor: 'var(--bg-field)',
                    border: '1px solid var(--border)',
                  }}
                >
                  <Search size={11} style={{ flexShrink: 0, color: 'var(--text-tertiary)' }} />
                  <input
                    autoFocus
                    value={query}
                    onChange={(e) => {
                      setQuery(e.target.value);
                      setActive(0);
                    }}
                    placeholder={searchPlaceholder}
                    className="w-full outline-none"
                    style={{
                      fontSize: fs,
                      color: 'var(--text-primary)',
                      backgroundColor: 'transparent',
                    }}
                  />
                </div>
              </div>
            )}
            {searchable && visible.length === 0 ? (
              <div
                className="px-2 py-3 text-center"
                style={{ color: 'var(--text-tertiary)', fontSize: fs }}
              >
                没有匹配的选项
              </div>
            ) : groups && groups.length > 0 && !(searchable && query.trim()) ? (
              (() => {
                let idx = -1;
                return groups.map((g) => (
                  <div key={g.label}>
                    <div
                      className="px-2 pt-1.5 pb-0.5 text-[9px] font-medium"
                      style={{ color: 'var(--text-tertiary)' }}
                    >
                      {g.label}
                    </div>
                    {g.options.map((o) => {
                      idx += 1;
                      return renderItem(o, idx, true);
                    })}
                  </div>
                ));
              })()
            ) : (
              shown.map((o, i) => renderItem(o, i, false))
            )}
            {searchable && visible.length > shown.length && (
              <div
                className="px-2 pt-1 pb-0.5 text-center"
                style={{ color: 'var(--text-tertiary)', fontSize: 10 }}
              >
                匹配 {visible.length} 项，仅显示前 {MAX_RENDER} 项，继续输入可缩小范围
              </div>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
