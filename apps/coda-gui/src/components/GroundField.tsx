// The page's ground field (2.0 R3, DESIGN.md "Colour"): the page's
// subject's ground stops (`Palette.grounds`, 1–3, the cover's heaviest
// colours, each deep enough for every text level) painted behind the
// whole window: the first as the fill (the top: the top bar's fade and
// the glass take `--ground`, the same colour), the second rising from the
// bottom right, the third from the bottom left. Every point is an sRGB
// mix of the stops, so it is as readable as they are (core's tests).
// Without a subject, the base.
//
// Gradients do not transition, so a change crossfades whole layers
// (lib/glowLayers.ts, as the glows do): the new field fades in over the
// old (`--motion-tint`, from wherever a fade was; at once with reduced
// motion), then the old goes. Each layer is opaque. The one there when it
// mounts appears at once. Decorative: hidden from assistive tech, no
// pointer events.

import { useEffect, useRef, useState } from "react";
import { useLayerFade } from "../hooks/useLayerFade";
import { initialLayers, loaded, prune, setUrl, type Layer, type Layers } from "../lib/glowLayers";
import { fieldBackground, fieldKey } from "../lib/groundField";
import { cssMs } from "../lib/motion";

/** The field's crossfade, in ms (`--motion-tint`; 0 with reduced motion). */
function fadeMs(): number {
  return cssMs("--motion-tint", 700);
}

export function GroundField({ stops }: { stops: readonly string[] | null }) {
  const key = fieldKey(stops);
  const [state, setState] = useState<Layers>(() => initialLayers(key));
  const [mountLayer] = useState(() => state.layers[0]?.id ?? -1);
  const [seen, setSeen] = useState(key);
  const timers = useRef(new Set<number>());
  if (key !== seen) {
    setSeen(key);
    setState((s) => setUrl(s, key));
  }
  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((t) => window.clearTimeout(t));
  }, []);

  const ready = (id: number) => {
    setState((s) => loaded(s, id));
    const ms = fadeMs();
    if (ms <= 0) {
      setState((s) => prune(s, id));
      return;
    }
    const timer = window.setTimeout(() => {
      timers.current.delete(timer);
      setState((s) => prune(s, id));
    }, ms + 50);
    timers.current.add(timer);
  };

  return (
    <div className="ground-field" aria-hidden="true">
      {state.layers.map((layer) => (
        <FieldLayer
          key={layer.id}
          layer={layer}
          instant={layer.id === mountLayer}
          latest={layer.id === state.latest}
          onReady={() => ready(layer.id)}
        />
      ))}
    </div>
  );
}

/** One field: ready at once (nothing to load), shown once wanted. */
function FieldLayer({
  layer,
  instant,
  latest,
  onReady,
}: {
  layer: Layer;
  instant: boolean;
  latest: boolean;
  onReady: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const report = useRef(onReady);
  report.current = onReady;
  useEffect(() => {
    if (latest) report.current();
  }, [latest]);
  useLayerFade(ref, layer.shown, instant, fadeMs);
  return <div ref={ref} className="ground-layer" style={fieldBackground(layer.url)} />;
}
