import { useEffect, useRef, type CSSProperties, type ReactNode } from 'react';

interface Props {
  scale: number;
  onScaleChange: (next: number) => void;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}

const MIN_SCALE = 0.5;
const MAX_SCALE = 2;
const SCALE_STEP = 0.1;
const DRAG_THRESHOLD = 4;

/**
 * 拓扑图视口：滚轮缩放（以指针为锚点）+ 按住左/中键拖动平移。
 * 滚动容器本身负责裁剪，缩放只改 SVG 尺寸，不引入 transform，避免坐标换算。
 */
export function TaskGraphViewport({ scale, onScaleChange, className, style, children }: Props) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const scaleRef = useRef(scale);
  scaleRef.current = scale;
  const anchorRef = useRef<{ x: number; y: number; scrollLeft: number; scrollTop: number; ratio: number } | null>(null);
  const dragRef = useRef<{ x: number; y: number; scrollLeft: number; scrollTop: number; active: boolean } | null>(null);
  const suppressClickRef = useRef(false);

  // React 的 onWheel 在部分版本以 passive 方式注册，preventDefault 会失效；必须原生监听。
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.shiftKey) {
        el.scrollLeft += e.deltaY || e.deltaX;
        return;
      }
      const current = scaleRef.current;
      const step = e.deltaY < 0 ? SCALE_STEP : -SCALE_STEP;
      const next = Math.min(MAX_SCALE, Math.max(MIN_SCALE, Number((current + step).toFixed(2))));
      if (next === current) return;
      const rect = el.getBoundingClientRect();
      anchorRef.current = {
        x: e.clientX - rect.left,
        y: e.clientY - rect.top,
        scrollLeft: el.scrollLeft,
        scrollTop: el.scrollTop,
        ratio: next / current,
      };
      onScaleChange(next);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [onScaleChange]);

  // 缩放渲染完成后再按比例修正滚动位置，保持指针下的节点不漂移。
  useEffect(() => {
    const anchor = anchorRef.current;
    const el = viewportRef.current;
    if (!anchor || !el) return;
    anchorRef.current = null;
    el.scrollLeft = (anchor.scrollLeft + anchor.x) * anchor.ratio - anchor.x;
    el.scrollTop = (anchor.scrollTop + anchor.y) * anchor.ratio - anchor.y;
  }, [scale]);

  const handlePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 && e.button !== 1) return;
    const el = viewportRef.current;
    if (!el) return;
    if (e.button === 1) e.preventDefault();
    dragRef.current = { x: e.clientX, y: e.clientY, scrollLeft: el.scrollLeft, scrollTop: el.scrollTop, active: false };
    el.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    const el = viewportRef.current;
    if (!drag || !el) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!drag.active && Math.abs(dx) + Math.abs(dy) < DRAG_THRESHOLD) return;
    drag.active = true;
    el.scrollLeft = drag.scrollLeft - dx;
    el.scrollTop = drag.scrollTop - dy;
    el.style.cursor = 'grabbing';
  };

  const handlePointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    const el = viewportRef.current;
    dragRef.current = null;
    if (el) {
      el.style.cursor = '';
      el.releasePointerCapture?.(e.pointerId);
    }
    // 拖动结束后吞掉本次 click，避免误选中节点；下一轮事件循环恢复。
    if (drag?.active) {
      suppressClickRef.current = true;
      window.setTimeout(() => {
        suppressClickRef.current = false;
      }, 0);
    }
  };

  return (
    <div
      ref={viewportRef}
      className={`overflow-auto ${className ?? ''}`}
      style={{ cursor: 'grab', ...style }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onClickCapture={(e) => {
        if (!suppressClickRef.current) return;
        suppressClickRef.current = false;
        e.stopPropagation();
        e.preventDefault();
      }}
    >
      {children}
    </div>
  );
}
