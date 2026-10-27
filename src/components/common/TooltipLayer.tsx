/**
 * TooltipLayer — 全局接管原生 `title` 的统一浮层。
 *
 * 背景：应用里有 600+ 处 `title`，原生提示由 OS 绘制 —— 样式丑、不换行、不设最大宽度，
 * 还能画到窗口客户区之外。这里用**事件委托**统一接管，业务组件一行都不用改：
 * - hover / focus 命中 `[title]`：把文本搬到 `data-pd-title` 并摘掉 `title`（压掉原生提示）；
 * - 用一个挂到 body 的 portal 浮层渲染（气泡 + 指向锚点的小箭头）：最多 360px、自动换行、
 *   超出视口时翻转/夹取；气泡被夹取时箭头仍指向锚点中心；
 * - 离开 / 失焦 / 卸载时把 `title` 原样还原（不影响读屏）。滚动、点击、Esc 只收起气泡，
 *   **不还原 title** —— 指针还在元素上，还原后不会再触发 `mouseover`，原生提示会在约 1 秒后
 *   迟到地冒出来（"长时间悬停仍出现原生提示"就是这么来的）。
 *
 * 豁免：`iframe`、`<svg>` 内部（它们的 title 是图形/文档名称），以及给元素或祖先加
 * `data-pd-tip="off"` 的自绘富提示。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

/** 悬浮多久后出现（原生约 1s，这里更快但仍能避免扫过即闪） */
const SHOW_DELAY_MS = 300;
/** 浮层与锚点的间距（留出箭头尖端的余量） */
const GAP = 8;
/** 与视口边缘的最小留白（也是「不溢出窗口」的保证） */
const EDGE = 8;
/** 单行/多行的最大宽度 */
const MAX_WIDTH = 360;
/** 指向锚点的小箭头边长（正方形旋转 45°，露在气泡外半个） */
const ARROW_SIZE = 8;

interface TipState {
  text: string;
  rect: DOMRect;
}

export function TooltipLayer() {
  const [tip, setTip] = useState<TipState | null>(null);
  const layerRef = useRef<HTMLDivElement | null>(null);
  const arrowRef = useRef<HTMLSpanElement | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 已摘掉 title、等待延迟结束的元素 */
  const pendingElRef = useRef<HTMLElement | null>(null);
  /** 正在显示提示的元素 */
  const activeElRef = useRef<HTMLElement | null>(null);
  /**
   * 当前被摘掉 `title` 的元素（**独立于浮层是否可见**）。
   *
   * 收起气泡不等于指针离开了元素：滚动 / 点击 / Esc 之后指针还停着，此时若把 title 还回去，
   * 浏览器会重新排程原生提示，而指针不动就再也不会触发 `mouseover`，于是 1 秒后冒出原生框。
   * 因此 title 只在指针真正离开（或失焦 / 卸载）时还原。
   */
  const stashedElRef = useRef<HTMLElement | null>(null);

  const cancelPending = useCallback((): HTMLElement | null => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    const el = pendingElRef.current;
    pendingElRef.current = null;
    return el;
  }, []);

  /** 还原被摘掉的 title（读屏 / 原生行为都依赖它） */
  const restoreTitle = useCallback((el: HTMLElement | null) => {
    if (!el) return;
    if (stashedElRef.current === el) stashedElRef.current = null;
    const stashed = el.getAttribute('data-pd-title');
    if (stashed === null) return;
    el.setAttribute('title', stashed);
    el.removeAttribute('data-pd-title');
  }, []);

  /** 把 title 搬到 data-pd-title（同一时刻只保留一个被摘除的元素） */
  const stashTitle = useCallback(
    (el: HTMLElement) => {
      if (!el.hasAttribute('title')) return;
      const prev = stashedElRef.current;
      if (prev && prev !== el) restoreTitle(prev);
      el.setAttribute('data-pd-title', el.getAttribute('title') ?? '');
      el.removeAttribute('title');
      stashedElRef.current = el;
    },
    [restoreTitle],
  );

  /**
   * 收起气泡。
   *
   * `restoreTitle` 仅在指针真的离开 / 失焦 / 卸载时为 true；其余场景（滚动、点击、Esc、缩放）
   * 只收起视觉，title 继续保持摘除状态，避免原生提示迟到地冒出来。
   */
  const hide = useCallback(
    (restoreTitleToo = true) => {
      cancelPending();
      activeElRef.current = null;
      setTip(null);
      if (restoreTitleToo) restoreTitle(stashedElRef.current);
    },
    [cancelPending, restoreTitle],
  );

  const scheduleShow = useCallback(
    (el: HTMLElement) => {
      const text = (el.getAttribute('title') ?? el.getAttribute('data-pd-title') ?? '').trim();
      if (!text) return;
      // iframe 的 title 是给读屏的文档名；SVG 内的 title 是图形标题，都不应变成浮层
      if (el.tagName === 'IFRAME' || el instanceof SVGElement) return;
      if (el.closest('[data-pd-tip="off"]')) return;

      // 先改名压掉原生提示：原生延迟约 1s，同步改名不会闪
      stashTitle(el);

      const prevPending = cancelPending();
      if (prevPending && prevPending !== el) restoreTitle(prevPending);
      if (activeElRef.current && activeElRef.current !== el) {
        restoreTitle(activeElRef.current);
        activeElRef.current = null;
        setTip(null);
      }

      pendingElRef.current = el;
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        pendingElRef.current = null;
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 && rect.height === 0) return; // 元素已不可见
        activeElRef.current = el;
        setTip({ text, rect });
      }, SHOW_DELAY_MS);
    },
    [cancelPending, restoreTitle, stashTitle],
  );

  useEffect(() => {
    const findTrigger = (node: EventTarget | null): HTMLElement | null => {
      if (!(node instanceof Element)) return null;
      return node.closest<HTMLElement>('[title], [data-pd-title]');
    };

    const onOver = (event: Event) => {
      const el = findTrigger(event.target);
      if (!el) return;
      if (el === activeElRef.current || el === pendingElRef.current) return;
      scheduleShow(el);
    };

    const onOut = (event: Event) => {
      // 以「仍被摘掉 title 的元素」为准：滚动/点击已把 active 清空，但指针可能还停在原处
      const anchor = stashedElRef.current ?? activeElRef.current ?? pendingElRef.current;
      if (!anchor) return;
      const to = (event as MouseEvent).relatedTarget;
      // 在锚点内部移动（父 → 子）不收起
      if (to instanceof Node && anchor.contains(to)) return;
      hide(true);
    };

    /**
     * 动态 title 兜底：元素挂着提示时被 React 重新写回 `title`（标题随状态变化、组件重挂载），
     * 若不管，原生提示会在约 1 秒后冒出来。这里节流复查指针下的元素，重新摘除。
     */
    let lastRecheckAt = 0;
    const onMove = (event: Event) => {
      const now = performance.now();
      if (now - lastRecheckAt < 200) return;
      lastRecheckAt = now;
      const el = findTrigger(event.target);
      // 只处理「本来就在被摘除的那个元素」，避免父子元素交替时把上一层 title 还回去
      if (el && el === stashedElRef.current) stashTitle(el);
    };

    const onFocusIn = (event: Event) => {
      const el = findTrigger(event.target);
      if (el) scheduleShow(el);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') hide(false);
    };

    /** 以下几类只收起气泡、不还原 title：指针还在元素上，还回去就会招来原生提示 */
    const onFocusOut = () => hide(true);
    const collapseOnly = () => hide(false);

    document.addEventListener('mouseover', onOver, true);
    document.addEventListener('mouseout', onOut, true);
    document.addEventListener('mousemove', onMove, true);
    document.addEventListener('focusin', onFocusIn, true);
    document.addEventListener('focusout', onFocusOut, true);
    document.addEventListener('mousedown', collapseOnly, true);
    // 任意滚动容器 / 页面滚动都收起（浮层是 fixed 定位，不跟随）
    document.addEventListener('scroll', collapseOnly, true);
    document.addEventListener('wheel', collapseOnly, { capture: true, passive: true });
    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', collapseOnly);
    window.addEventListener('blur', onFocusOut);

    return () => {
      document.removeEventListener('mouseover', onOver, true);
      document.removeEventListener('mouseout', onOut, true);
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('focusin', onFocusIn, true);
      document.removeEventListener('focusout', onFocusOut, true);
      document.removeEventListener('mousedown', collapseOnly, true);
      document.removeEventListener('scroll', collapseOnly, true);
      document.removeEventListener('wheel', collapseOnly, { capture: true });
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', collapseOnly);
      window.removeEventListener('blur', onFocusOut);
      restoreTitle(cancelPending());
      restoreTitle(activeElRef.current);
      restoreTitle(stashedElRef.current);
    };
  }, [cancelPending, hide, restoreTitle, scheduleShow, stashTitle]);

  // 渲染后量尺寸：默认在下方，放不下翻到上方，水平/垂直都夹取在视口内。
  // 直接写 DOM 样式（不改 state）：避免多一次渲染，也避开 set-state-in-effect。
  useLayoutEffect(() => {
    if (!tip) return;
    const layer = layerRef.current;
    if (!layer) return;
    const h = layer.offsetHeight;
    const w = layer.offsetWidth;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const { rect } = tip;

    const spaceBelow = vh - rect.bottom - GAP;
    const spaceAbove = rect.top - GAP;
    const placeAbove = spaceBelow < h + EDGE && spaceAbove > spaceBelow;
    let top = placeAbove ? rect.top - GAP - h : rect.bottom + GAP;
    top = Math.max(EDGE, Math.min(top, vh - h - EDGE));

    let left = rect.left + rect.width / 2 - w / 2;
    left = Math.max(EDGE, Math.min(left, vw - w - EDGE));

    layer.style.left = `${Math.round(left)}px`;
    layer.style.top = `${Math.round(top)}px`;
    layer.style.visibility = 'visible';

    // 气泡被夹取后，箭头仍对准锚点中心（限制在气泡内部，避免戳出圆角）
    const arrow = arrowRef.current;
    if (arrow) {
      const anchorCenterX = rect.left + rect.width / 2;
      const arrowX = Math.max(12, Math.min(anchorCenterX - left, w - 12));
      arrow.style.left = `${Math.round(arrowX - ARROW_SIZE / 2)}px`;
      if (placeAbove) {
        // 气泡在锚点上方：箭头挂在气泡底边、朝下（可见边为右下两条）
        arrow.style.top = '100%';
        arrow.style.bottom = '';
        arrow.style.marginTop = `${-ARROW_SIZE / 2}px`;
        arrow.style.borderRight = '1px solid var(--border)';
        arrow.style.borderBottom = '1px solid var(--border)';
        arrow.style.borderTop = '0';
        arrow.style.borderLeft = '0';
      } else {
        // 气泡在锚点下方：箭头挂在气泡顶边、朝上（可见边为左上两条）
        arrow.style.top = '';
        arrow.style.bottom = '100%';
        arrow.style.marginBottom = `${-ARROW_SIZE / 2}px`;
        arrow.style.borderTop = '1px solid var(--border)';
        arrow.style.borderLeft = '1px solid var(--border)';
        arrow.style.borderRight = '0';
        arrow.style.borderBottom = '0';
      }
    }
  }, [tip]);

  if (!tip) return null;

  return createPortal(
    <div
      ref={layerRef}
      role="tooltip"
      style={{
        position: 'fixed',
        // 量好前先隐藏，避免在 (0,0) 闪一下
        left: 0,
        top: 0,
        visibility: 'hidden',
        maxWidth: `min(${MAX_WIDTH}px, calc(100vw - ${EDGE * 2}px))`,
        padding: '6px 9px',
        borderRadius: 8,
        fontSize: 11,
        lineHeight: 1.5,
        // 换行与长串（路径 / 命令）折行都在这里解决
        whiteSpace: 'pre-wrap',
        wordBreak: 'break-word',
        backgroundColor: 'var(--bg-primary)',
        color: 'var(--text-primary)',
        border: '1px solid var(--border)',
        boxShadow: '0 6px 20px rgba(0, 0, 0, 0.18)',
        pointerEvents: 'none',
        zIndex: 100000,
      }}
    >
      {tip.text}
      {/* 气泡箭头：正方形旋转 45°，只露半个在气泡外；边和位置在 layout effect 里按翻转方向设置 */}
      <span
        ref={arrowRef}
        aria-hidden="true"
        style={{
          position: 'absolute',
          width: ARROW_SIZE,
          height: ARROW_SIZE,
          backgroundColor: 'var(--bg-primary)',
          transform: 'rotate(45deg)',
          pointerEvents: 'none',
        }}
      />
    </div>,
    document.body,
  );
}
