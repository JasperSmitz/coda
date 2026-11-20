// A bloom of the actual art behind a cover (2.0 R3, DESIGN.md "Colour"):
// the cover itself, stretched over a box about 2.4× its width (Home's
// carousel) or 220px wider and taller (a page header), very blurred and
// saturated (`--bloom-filter`, `--bloom-opacity`), so its own colours
// spread out around it. A mask (app.css `.cover-bloom`) fades it out into
// the ground field: strong only up to `--bloom-reach` beyond the cover on
// the sides where text sits, then at most `--bloom-edge` (the ground
// veil's alpha: core's `BLOOM_TEXT_ALPHA`, so every text level keeps its
// contrast there over any colour), then nothing.
//
// It follows its cover with the glows' crossfade (lib/glowLayers.ts,
// hooks/useLayerFade.ts: the new art fades in once its image has loaded,
// `--motion-glow`, at once with reduced motion), sitting still while
// Home's covers slide. The bloom there when it mounts appears at once. One
// composited layer per art (`will-change: opacity`), blurred once, never
// transformed. Decorative: hidden from assistive tech, no pointer events.

import { useEffect, useRef, useState } from "react";
import { useLayerFade } from "../hooks/useLayerFade";
import { coverSrc } from "../lib/ipc";
import { failed, initialLayers, loaded, prune, setUrl, type Layer, type Layers } from "../lib/glowLayers";
import { cssMs } from "../lib/motion";

function fadeMs(): number {
  return cssMs("--motion-glow", 600);
}

export function CoverBloom({ url, className }: { url: string | null; className: string }) {
  const [state, setState] = useState<Layers>(() => initialLayers(url));
  const [mountLayer] = useState(() => state.layers[0]?.id ?? -1);
  const [seen, setSeen] = useState(url);
  const timers = useRef(new Set<number>());
  if (url !== seen) {
    setSeen(url);
    setState((s) => setUrl(s, url));
  }
  const pruneLater = (keepFrom: number) => {
    const ms = fadeMs();
    if (ms <= 0) {
      setState((s) => prune(s, keepFrom));
      return;
    }
    const timer = window.setTimeout(() => {
      timers.current.delete(timer);
      setState((s) => prune(s, keepFrom));
    }, ms + 50);
    timers.current.add(timer);
  };
  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((t) => window.clearTimeout(t));
  }, []);
  const next = state.next;
  useEffect(() => {
    if (!url) pruneLater(next);
    // Only when the cover changes.
  }, [url]);

  const latest = state.latest;
  return (
    <div className={`cover-bloom ${className}`} aria-hidden="true">
      {state.layers.map((layer) => (
        <BloomLayer
          key={layer.id}
          layer={layer}
          instant={layer.id === mountLayer}
          onReady={() => {
            if (layer.id !== latest) return;
            setState((s) => loaded(s, layer.id));
            pruneLater(layer.id);
          }}
          onFailed={() => {
            setState((s) => failed(s, layer.id));
            if (layer.id === latest) pruneLater(layer.id + 1);
          }}
        />
      ))}
    </div>
  );
}

function BloomLayer({
  layer,
  instant,
  onReady,
  onFailed,
}: {
  layer: Layer;
  instant: boolean;
  onReady: () => void;
  onFailed: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);
  const report = useRef(onReady);
  report.current = onReady;
  useEffect(() => {
    if (ready) report.current();
  }, [ready]);
  useLayerFade(ref, ready && layer.shown, instant, fadeMs);
  return (
    <div ref={ref} className="bloom-layer">
      <img
        className="bloom-image"
        src={coverSrc(layer.url, "small")}
        alt=""
        draggable={false}
        onLoad={() => setReady(true)}
        onError={onFailed}
      />
    </div>
  );
}
