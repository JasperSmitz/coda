// Pointer-based drag reordering of fixed-height, virtualized rows inside
// a scrolling element. A press on a row that moves more than a few pixels
// becomes a drag; the drop gap (0..count, "before row n") follows the
// pointer, the scroller autoscrolls near its edges, Esc cancels. The
// pointer is captured by the scroller, so rows may unmount while
// scrolling. The click that ends a drag is swallowed.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";

const THRESHOLD = 4;
/** Distance from an edge where autoscroll starts, and its top speed (px/frame). */
const EDGE = 40;
const MAX_SPEED = 18;

interface Options {
  /** The scrolling element (holds the rows). */
  scroller: RefObject<HTMLElement | null>;
  /** The rows' container (row 0 at its top), inside the scroller. */
  rows: RefObject<HTMLElement | null>;
  rowHeight: number;
  count: number;
  /** A drop at `gap`; `payload` is what `start` was given. */
  onDrop: (gap: number, payload: string[]) => void;
  disabled?: boolean;
}

export interface DragState {
  /** The drop gap while dragging, else `null`. */
  gap: number | null;
  /** What is being dragged. */
  payload: string[];
}

export function useDragReorder({ scroller, rows, rowHeight, count, onDrop, disabled }: Options) {
  const [drag, setDrag] = useState<DragState>({ gap: null, payload: [] });
  const state = useRef<{
    pointerId: number;
    startY: number;
    clientY: number;
    payload: () => string[];
    active: boolean;
    frame: number;
  } | null>(null);
  const swallowClick = useRef(false);
  const options = useRef({ rowHeight, count, onDrop });
  options.current = { rowHeight, count, onDrop };

  const gapAt = useCallback(
    (clientY: number): number => {
      const el = rows.current;
      if (!el) return 0;
      const top = el.getBoundingClientRect().top;
      const { rowHeight, count } = options.current;
      return Math.min(count, Math.max(0, Math.round((clientY - top) / rowHeight)));
    },
    [rows],
  );

  const stop = useCallback(() => {
    const s = state.current;
    if (!s) return;
    cancelAnimationFrame(s.frame);
    const el = scroller.current;
    if (el?.hasPointerCapture(s.pointerId)) el.releasePointerCapture(s.pointerId);
    state.current = null;
    setDrag({ gap: null, payload: [] });
  }, [scroller]);

  // Autoscroll while the pointer is near an edge.
  const tick = useCallback(() => {
    const s = state.current;
    const el = scroller.current;
    if (!s || !s.active || !el) return;
    const rect = el.getBoundingClientRect();
    let speed = 0;
    if (s.clientY < rect.top + EDGE) speed = -((rect.top + EDGE - s.clientY) / EDGE) * MAX_SPEED;
    else if (s.clientY > rect.bottom - EDGE)
      speed = ((s.clientY - (rect.bottom - EDGE)) / EDGE) * MAX_SPEED;
    if (speed !== 0) {
      el.scrollTop += Math.max(-MAX_SPEED, Math.min(MAX_SPEED, speed));
      const gap = gapAt(s.clientY);
      setDrag((d) => (d.gap === gap ? d : { ...d, gap }));
    }
    s.frame = requestAnimationFrame(tick);
  }, [gapAt, scroller]);

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const onMove = (e: PointerEvent) => {
      const s = state.current;
      if (!s || e.pointerId !== s.pointerId) return;
      s.clientY = e.clientY;
      if (!s.active) {
        if (Math.abs(e.clientY - s.startY) < THRESHOLD) return;
        s.active = true;
        el.setPointerCapture(s.pointerId);
        s.frame = requestAnimationFrame(tick);
        setDrag({ gap: gapAt(e.clientY), payload: s.payload() });
        return;
      }
      const gap = gapAt(e.clientY);
      setDrag((d) => (d.gap === gap ? d : { ...d, gap }));
    };
    const onUp = (e: PointerEvent) => {
      const s = state.current;
      if (!s || e.pointerId !== s.pointerId) return;
      if (s.active) {
        // The click that follows this release (if any) is not a click.
        swallowClick.current = true;
        setTimeout(() => {
          swallowClick.current = false;
        }, 0);
        const payload = s.payload();
        const gap = gapAt(e.clientY);
        stop();
        options.current.onDrop(gap, payload);
      } else {
        stop();
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && state.current?.active) {
        e.preventDefault();
        e.stopPropagation();
        stop();
      }
    };
    // Moves before the capture are seen on the window.
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", stop);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", stop);
      window.removeEventListener("keydown", onKey, true);
      stop();
    };
  }, [gapAt, scroller, stop, tick]);

  /**
   * From a row's `onPointerDown`: a primary-button press may become a
   * drag of `payload()` (read when the drag starts).
   */
  const start = useCallback(
    (e: ReactPointerEvent, payload: () => string[]) => {
      if (disabled || e.button !== 0 || e.pointerType === "touch") return;
      if (e.target instanceof HTMLElement && e.target.closest("button")) return;
      state.current = {
        pointerId: e.pointerId,
        startY: e.clientY,
        clientY: e.clientY,
        payload,
        active: false,
        frame: 0,
      };
    },
    [disabled],
  );

  /** From the rows' `onClickCapture`: drops the click that ends a drag. */
  const onClickCapture = useCallback((e: ReactMouseEvent) => {
    if (swallowClick.current) {
      swallowClick.current = false;
      e.stopPropagation();
      e.preventDefault();
    }
  }, []);

  return { drag, start, onClickCapture };
}
