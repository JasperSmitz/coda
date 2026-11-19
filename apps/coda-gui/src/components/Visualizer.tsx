// Visualizer bars on a canvas: raw readings from `visualizer` events are
// smoothed and decayed here (requestAnimationFrame), and the 7 bands are
// spread into ~32–48 bars. With reduced motion the bars update a few
// times a second instead of animating. The bars are `--viz` (the
// accent, or the playing cover's), re-read every frame while a colour
// change blends (after the accent or a scope's palette changes).

import { useEffect, useRef } from "react";
import { onVisualizer } from "../lib/ipc";
import { ACCENT_EVENT } from "../lib/accent";
import { PALETTE_EVENT } from "../lib/palette";
import { prefersReducedMotion } from "../lib/motion";
import { Meter, spreadBands } from "../lib/visualizer";

const REDUCED_INTERVAL_MS = 250;
/** How long `--viz` is re-read after a change: its blend (`--motion-accent`) and a margin. */
const BLEND_READ_MS = 2000;

export function Visualizer({ active }: { active: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;
    const meter = new Meter();
    let latest: (number | null)[] | null = null;
    let lastReduced = 0;
    let frame = 0;
    let drawn = false;
    let color = "";
    /** Until when the colour is re-read each frame (it may be blending). */
    let blendUntil = 0;
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
    };
    readColor();
    window.addEventListener(ACCENT_EVENT, colourChanged);
    window.addEventListener(PALETTE_EVENT, colourChanged);
    const unlisten = onVisualizer((f) => {
      latest = f.bands;
    });

    const draw = (now: number) => {
      frame = requestAnimationFrame(draw);
      const reduced = prefersReducedMotion();
      if (reduced && now - lastReduced < REDUCED_INTERVAL_MS) return;
      lastReduced = now;
      if (now < blendUntil) readColor();
      let moved: boolean;
      if (latest) {
        meter.update(latest, now);
        latest = null;
        moved = true;
      } else {
        moved = meter.fade(now);
      }
      const dpr = window.devicePixelRatio || 1;
      const width = element.clientWidth;
      const height = element.clientHeight;
      if (element.width !== Math.round(width * dpr) || element.height !== Math.round(height * dpr)) {
        element.width = Math.round(width * dpr);
        element.height = Math.round(height * dpr);
        moved = true;
      }
      // Nothing changed since the last frame: keep what is drawn.
      if (!moved && drawn) return;
      drawn = true;
      context.setTransform(dpr, 0, 0, dpr, 0, 0);
      context.clearRect(0, 0, width, height);
      const bars = Math.max(32, Math.min(48, Math.round(width / 14)));
      const gap = 3;
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
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener(ACCENT_EVENT, colourChanged);
      window.removeEventListener(PALETTE_EVENT, colourChanged);
      unlisten.then((f) => f()).catch(() => {});
    };
  }, []);

  return (
    <canvas
      ref={canvas}
      className={`visualizer ${active ? "" : "is-idle"}`}
      aria-hidden="true"
    />
  );
}
