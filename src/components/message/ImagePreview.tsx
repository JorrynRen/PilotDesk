import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';
import { useImagePreviewStore } from '../../stores/imagePreviewStore';

/**
 * 全局图片放大预览（Portal 到 body，避免被消息列表 transform/overflow 影响）。
 * 支持：滚轮缩放（0.25x~6x）、左键拖拽移动、Escape / 关闭按钮 / 背景点击关闭。
 * 会话模式与群聊模式的 Markdown 图片、附件图片统一走 useImagePreviewStore。
 */
export function ImagePreview() {
  const src = useImagePreviewStore((s) => s.src);
  const close = useImagePreviewStore((s) => s.close);
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{ startX: number; startY: number; offsetX: number; offsetY: number; moved: boolean } | null>(null);
  const justDraggedRef = useRef(false);

  // 打开时重置缩放与位置
  useEffect(() => {
    if (src) {
      setScale(1);
      setOffset({ x: 0, y: 0 });
      setDragging(false);
      dragRef.current = null;
      justDraggedRef.current = false;
    }
  }, [src]);

  // Escape 关闭
  useEffect(() => {
    if (!src) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [src, close]);

  if (!src) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center p-6 cursor-zoom-out select-none"
      style={{ backgroundColor: 'rgba(0,0,0,0.85)' }}
      onClick={() => {
        if (justDraggedRef.current) {
          justDraggedRef.current = false;
          return;
        }
        close();
      }}
      onWheel={(e) => {
        e.preventDefault();
        e.stopPropagation();
        const factor = Math.exp(-e.deltaY * 0.002);
        setScale((s) => Math.min(6, Math.max(0.25, s * factor)));
      }}
      onMouseMove={(e) => {
        const d = dragRef.current;
        if (!d) return;
        const dx = e.clientX - d.startX;
        const dy = e.clientY - d.startY;
        if (!d.moved && Math.hypot(dx, dy) > 3) {
          d.moved = true;
          justDraggedRef.current = true;
        }
        if (d.moved) setOffset({ x: d.offsetX + dx, y: d.offsetY + dy });
      }}
      onMouseUp={() => {
        dragRef.current = null;
        setDragging(false);
      }}
    >
      <button
        onClick={close}
        className="absolute top-4 right-4 w-9 h-9 rounded-full flex items-center justify-center text-white/80 hover:text-white hover:bg-white/10 transition-colors"
        aria-label="关闭预览"
      >
        <X size={20} />
      </button>
      <img
        src={src}
        alt="图片预览"
        draggable={false}
        className={`max-w-full max-h-full object-contain rounded-lg shadow-2xl ${dragging ? 'cursor-grabbing' : 'cursor-grab'}`}
        style={{ transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})` }}
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          e.stopPropagation();
          dragRef.current = { startX: e.clientX, startY: e.clientY, offsetX: offset.x, offsetY: offset.y, moved: false };
          setDragging(true);
        }}
        onClick={(e) => e.stopPropagation()}
      />
    </div>,
    document.body
  );
}
