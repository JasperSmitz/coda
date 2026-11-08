// A horizontal slider (seek bar, volume) with pointer drag and its own
// keyboard steps. Not an <input type=range>, so the global arrow-key
// shortcuts and the slider never fight over a key.

import { useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

interface SliderProps {
  value: number;
  max: number;
  /** Keyboard step (arrow keys); Page keys move 4×. */
  step: number;
  label: string;
  valueText?: string;
  disabled?: boolean;
  className?: string;
  /** While dragging (or on each key press): the value under the pointer. */
  onInput?: (value: number) => void;
  /** Released (or a key press): the value to apply. */
  onCommit: (value: number) => void;
}

export function Slider({
  value,
  max,
  step,
  label,
  valueText,
  disabled = false,
  className = "",
  onInput,
  onCommit,
}: SliderProps) {
  const track = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const shown = drag ?? value;
  const fraction = max > 0 ? Math.min(1, Math.max(0, shown / max)) : 0;

  const valueAt = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect();
    if (!rect || rect.width === 0) return 0;
    const f = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return f * max;
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (disabled || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const v = valueAt(e.clientX);
    setDrag(v);
    onInput?.(v);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (drag === null) return;
    const v = valueAt(e.clientX);
    setDrag(v);
    onInput?.(v);
  };
  const finish = (e: PointerEvent<HTMLDivElement>, commit: boolean) => {
    if (drag === null) return;
    const v = valueAt(e.clientX);
    setDrag(null);
    if (commit) onCommit(v);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    const moves: Record<string, number> = {
      ArrowLeft: -step,
      ArrowDown: -step,
      ArrowRight: step,
      ArrowUp: step,
      PageDown: -4 * step,
      PageUp: 4 * step,
    };
    let next: number | null = null;
    if (e.key in moves) next = value + moves[e.key];
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = max;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    const clamped = Math.min(max, Math.max(0, next));
    onInput?.(clamped);
    onCommit(clamped);
  };

  return (
    <div
      className={`slider ${disabled ? "is-disabled" : ""} ${drag !== null ? "is-dragging" : ""} ${className}`}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(shown)}
      aria-valuetext={valueText}
      aria-disabled={disabled}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(e) => finish(e, true)}
      onPointerCancel={(e) => finish(e, false)}
      onKeyDown={onKeyDown}
    >
      <div className="slider-track" ref={track}>
        <div className="slider-fill" style={{ width: `${fraction * 100}%` }} />
        <div className="slider-thumb" style={{ left: `${fraction * 100}%` }} />
      </div>
    </div>
  );
}
