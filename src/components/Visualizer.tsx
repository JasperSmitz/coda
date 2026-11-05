// Visualizer bars on a canvas: raw readings from `visualizer` events are
// smoothed and decayed here (requestAnimationFrame), and the 7 bands are
// spread into ~32–48 bars. With reduced motion the bars update a few
// times a second instead of animating.

import { useEffect, useRef } from "react";
import { onVisualizer } from "../lib/ipc";
import { Meter, spreadBands } from "../lib/visualizer";

const REDUCED_INTERVAL_MS = 250;

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

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
    const scheme = window.matchMedia?.("(prefers-color-scheme: light)");
    const readColor = () => {
      color = getComputedStyle(element).getPropertyValue("--viz-color").trim() || "#f0a83a";
      drawn = false;
    };
    readColor();
    scheme?.addEventListener("change", readColor);
    const unlisten = onVisualizer((f) => {
      latest = f.bands;
    });

    const draw = (now: number) => {
      frame = requestAnimationFrame(draw);
      const reduced = prefersReducedMotion();
      if (reduced && now - lastReduced < REDUCED_INTERVAL_MS) return;
      lastReduced = now;
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
      scheme?.removeEventListener("change", readColor);
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
