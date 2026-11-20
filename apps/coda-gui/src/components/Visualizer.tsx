// Visualizer bars on a canvas: raw readings from `visualizer` events are
// smoothed and decayed here (requestAnimationFrame), and the 7 bands are
// spread into ~32–48 bars (or a fixed number: the player's 5). With
// reduced motion the bars update a few times a second instead of
// animating. The bars are `--viz` (the accent, or the playing cover's),
// re-read every frame while a colour change blends (after the accent or
// a scope's palette changes). The frame loop sleeps once the bars have
// fallen to rest (paused, stopped, nothing playing), no colour is
// blending and nothing was resized; a reading, a colour change or a
// resize (ResizeObserver) wakes it.

import { useEffect, useRef } from "react";
import { onVisualizer } from "../lib/ipc";
import { ACCENT_EVENT } from "../lib/accent";
import { PALETTE_EVENT } from "../lib/palette";
import { prefersReducedMotion } from "../lib/motion";
import { Meter, spreadBands } from "../lib/visualizer";

const REDUCED_INTERVAL_MS = 250;
/** How long `--viz` is re-read after a change: its blend (`--motion-accent`) and a margin. */
const BLEND_READ_MS = 1000;

interface Props {
  active: boolean;
  /** A fixed number of bars (else ~32–48 by width). */
  bars?: number;
  className?: string;
}

export function Visualizer({ active, bars: fixedBars, className = "" }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;
    const meter = new Meter();
    let latest: (number | null)[] | null = null;
    let lastReduced = 0;
    /** The scheduled frame (0: the loop sleeps). */
    let frame = 0;
    let drawn = false;
    let color = "";
    /** Until when the colour is re-read each frame (it may be blending). */
    let blendUntil = 0;
    /** The canvas's CSS size (from the ResizeObserver). */
    let width = 0;
    let height = 0;
    let disposed = false;
    const wake = () => {
      if (frame === 0 && !disposed) frame = requestAnimationFrame(draw);
    };
    const readColor = () => {
      const next = getComputedStyle(element).getPropertyValue("--viz").trim();
      if (next !== color) {
        color = next;
        drawn = false;
      }
    };
    const colourChanged = () => {
      blendUntil = performance.now() + BLEND_READ_MS;
      readColor();
      wake();
    };

    const draw = (now: number) => {
      frame = 0;
      const reduced = prefersReducedMotion();
      if (reduced && now - lastReduced < REDUCED_INTERVAL_MS) {
        frame = requestAnimationFrame(draw);
        return;
      }
      lastReduced = now;
      const blending = now < blendUntil;
      if (blending) readColor();
      let moved: boolean;
      if (latest) {
        meter.update(latest, now);
        latest = null;
        moved = true;
      } else {
        moved = meter.fade(now);
      }
      const dpr = window.devicePixelRatio || 1;
      if (element.width !== Math.round(width * dpr) || element.height !== Math.round(height * dpr)) {
        element.width = Math.round(width * dpr);
        element.height = Math.round(height * dpr);
        moved = true;
      }
      if (moved || !drawn) {
        drawn = true;
        paint(dpr);
      }
      // Keep going while the bars move (or wait to fall) or a colour
      // blends; at rest the loop sleeps until something wakes it.
      if (!meter.atRest() || blending) frame = requestAnimationFrame(draw);
    };

    const paint = (dpr: number) => {
      context.setTransform(dpr, 0, 0, dpr, 0, 0);
      context.clearRect(0, 0, width, height);
      const bars = fixedBars ?? Math.max(32, Math.min(48, Math.round(width / 14)));
      const gap = fixedBars ? Math.max(2, Math.round(width / bars / 3)) : 3;
      const barWidth = Math.max(2, (width - gap * (bars - 1)) / bars);
      context.fillStyle = color;
      const floor = 2;
      spreadBands(meter.bands, bars).forEach((value, i) => {
        if (value == null) return;
        const h = Math.max(floor, value * height);
        const x = i * (barWidth + gap);
        context.beginPath();
        context.roundRect(x, height - h, barWidth, h, Math.min(2, barWidth / 2));
        context.fill();
      });
    };

    readColor();
    window.addEventListener(ACCENT_EVENT, colourChanged);
    window.addEventListener(PALETTE_EVENT, colourChanged);
    const unlisten = onVisualizer((f) => {
      latest = f.bands;
      wake();
    });
    const observer = new ResizeObserver(() => {
      width = element.clientWidth;
      height = element.clientHeight;
      wake();
    });
    observer.observe(element);
    width = element.clientWidth;
    height = element.clientHeight;
    wake();
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      frame = 0;
      observer.disconnect();
      window.removeEventListener(ACCENT_EVENT, colourChanged);
      window.removeEventListener(PALETTE_EVENT, colourChanged);
      unlisten.then((f) => f()).catch(() => {});
    };
  }, [fixedBars]);

  return (
    <canvas
      ref={canvas}
      className={`visualizer ${className} ${active ? "" : "is-idle"}`}
      aria-hidden="true"
    />
  );
}
