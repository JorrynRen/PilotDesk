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
  const suppressClickRef = useRef(false);
  const cleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => () => cleanupRef.current?.(), []);

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

  // 只在判定为拖动后才接管指针；不提前 setPointerCapture，否则 click 会被重定向到容器，节点点击失效。
  const handlePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 && e.button !== 1) return;
    const el = viewportRef.current;
    if (!el) return;
    if (e.button === 1) e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = el.scrollLeft;
    const startTop = el.scrollTop;
    let active = false;

    const onMove = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      const dy = ev.clientY - startY;
      if (!active && Math.abs(dx) + Math.abs(dy) < DRAG_THRESHOLD) return;
      active = true;
      el.scrollLeft = startLeft - dx;
      el.scrollTop = startTop - dy;
      el.style.cursor = 'grabbing';
      ev.preventDefault();
    };

    const cleanup = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      el.style.cursor = '';
      cleanupRef.current = null;
    };

    const onUp = () => {
      const wasActive = active;
      cleanup();
      // 拖动结束后吞掉本次 click，避免误选中节点；下一轮事件循环恢复。
      if (wasActive) {
        suppressClickRef.current = true;
        window.setTimeout(() => {
          suppressClickRef.current = false;
        }, 0);
      }
    };

    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    cleanupRef.current = cleanup;
  };

  return (
    <div
      ref={viewportRef}
      className={`overflow-auto ${className ?? ''}`}
      style={{ cursor: 'grab', ...style }}
      onPointerDown={handlePointerDown}
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
