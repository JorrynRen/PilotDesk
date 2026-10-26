/**
 * TooltipLayer — 全局接管原生 `title` 的统一浮层。
 *
 * 背景：应用里有 600+ 处 `title`，原生提示由 OS 绘制 —— 样式丑、不换行、不设最大宽度，
 * 还能画到窗口客户区之外。这里用**事件委托**统一接管，业务组件一行都不用改：
 * - hover / focus 命中 `[title]`：把文本搬到 `data-pd-title` 并摘掉 `title`（压掉原生提示）；
 * - 用一个挂到 body 的 portal 浮层渲染：最多 360px、自动换行、超出视口时翻转/夹取；
 * - 离开、点击、滚动、缩放、失焦、Esc 都收起，并把 `title` 原样还原（不影响读屏）。
 *
 * 豁免：`iframe`、`<svg>` 内部（它们的 title 是图形/文档名称），以及给元素或祖先加
 * `data-pd-tip="off"` 的自绘富提示。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

/** 悬浮多久后出现（原生约 1s，这里更快但仍能避免扫过即闪） */
const SHOW_DELAY_MS = 300;
/** 浮层与锚点的间距 */
const GAP = 6;
/** 与视口边缘的最小留白（也是「不溢出窗口」的保证） */
const EDGE = 8;
/** 单行/多行的最大宽度 */
const MAX_WIDTH = 360;

interface TipState {
  text: string;
  rect: DOMRect;
}

export function TooltipLayer() {
  const [tip, setTip] = useState<TipState | null>(null);
  const layerRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 已摘掉 title、等待延迟结束的元素 */
  const pendingElRef = useRef<HTMLElement | null>(null);
  /** 正在显示提示的元素 */
  const activeElRef = useRef<HTMLElement | null>(null);

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
    const stashed = el.getAttribute('data-pd-title');
    if (stashed === null) return;
    el.setAttribute('title', stashed);
    el.removeAttribute('data-pd-title');
  }, []);

  const hide = useCallback(() => {
    restoreTitle(cancelPending());
    restoreTitle(activeElRef.current);
    activeElRef.current = null;
    setTip(null);
  }, [cancelPending, restoreTitle]);

  const scheduleShow = useCallback(
    (el: HTMLElement) => {
      const text = (el.getAttribute('title') ?? el.getAttribute('data-pd-title') ?? '').trim();
      if (!text) return;
      // iframe 的 title 是给读屏的文档名；SVG 内的 title 是图形标题，都不应变成浮层
      if (el.tagName === 'IFRAME' || el instanceof SVGElement) return;
      if (el.closest('[data-pd-tip="off"]')) return;

      // 先改名压掉原生提示：原生延迟约 1s，同步改名不会闪
      if (el.hasAttribute('title')) {
        el.setAttribute('data-pd-title', el.getAttribute('title') ?? '');
        el.removeAttribute('title');
      }

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
    [cancelPending, restoreTitle],
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
      const anchor = activeElRef.current ?? pendingElRef.current;
      if (!anchor) return;
      const to = (event as MouseEvent).relatedTarget;
      // 在锚点内部移动（父 → 子）不收起
      if (to instanceof Node && anchor.contains(to)) return;
      hide();
    };

    const onFocusIn = (event: Event) => {
      const el = findTrigger(event.target);
      if (el) scheduleShow(el);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') hide();
    };

    const onHide = () => hide();

    document.addEventListener('mouseover', onOver, true);
    document.addEventListener('mouseout', onOut, true);
    document.addEventListener('focusin', onFocusIn, true);
    document.addEventListener('focusout', onHide, true);
    document.addEventListener('mousedown', onHide, true);
    // 任意滚动容器 / 页面滚动都收起（浮层是 fixed 定位，不跟随）
    document.addEventListener('scroll', onHide, true);
    document.addEventListener('wheel', onHide, { capture: true, passive: true });
    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', onHide);
    window.addEventListener('blur', onHide);

    return () => {
      document.removeEventListener('mouseover', onOver, true);
      document.removeEventListener('mouseout', onOut, true);
      document.removeEventListener('focusin', onFocusIn, true);
      document.removeEventListener('focusout', onHide, true);
      document.removeEventListener('mousedown', onHide, true);
      document.removeEventListener('scroll', onHide, true);
      document.removeEventListener('wheel', onHide, { capture: true });
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', onHide);
      window.removeEventListener('blur', onHide);
      restoreTitle(cancelPending());
      restoreTitle(activeElRef.current);
    };
  }, [cancelPending, hide, restoreTitle, scheduleShow]);

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
        padding: '6px 8px',
        borderRadius: 6,
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
    </div>,
    document.body,
  );
}
