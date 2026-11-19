// Covers as blurred light (DESIGN.md "Colour"): `PageGlow` behind the top
// of a page (its subject's cover, very blurred, under a radial darkening
// of the page's tint, fading out by ~600px), and `CoverGlow`, the same
// crossfading layers for other places (the docked bar's background).
//
// A new cover fades in over the old one once loaded (`--motion-glow`),
// then the old one goes; no cover, or a new one that fails to load, fades
// the old ones out (lib/glowLayers.ts). At most three layers, briefly.
// Purely decorative: hidden from assistive tech, no pointer events, each
// layer its own compositing layer (CSS), so scrolling or anything moving
// over it does not repaint the blur.

import { useEffect, useRef, useState } from "react";
import { coverSrc } from "../lib/ipc";
import { failed, initialLayers, loaded, prune, setUrl, type Layers } from "../lib/glowLayers";
import { cssMs } from "../lib/motion";

/** The glow's crossfade, in ms (tokens.css `--motion-glow`; 0 with reduced motion). */
function fadeMs(): number {
  return cssMs("--motion-glow", 1800);
}

export function CoverGlow({ url, className }: { url: string | null; className: string }) {
  const [state, setState] = useState<Layers>(() => initialLayers(url));
  const [seen, setSeen] = useState(url);
  const timers = useRef(new Set<number>());
  if (url !== seen) {
    setSeen(url);
    setState((s) => setUrl(s, url));
  }

  /** Drops the layers older than `keepFrom` once a fade has run. */
  const pruneLater = (keepFrom: number) => {
    const ms = fadeMs();
    if (ms <= 0) {
      setState((s) => prune(s, keepFrom));
      return;
    }
    // A timer, not `transitionend`: a cached image may load before its
    // hidden state was drawn, and then nothing transitions.
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

  // No cover: the layers fading out go once faded.
  const next = state.next;
  useEffect(() => {
    if (!url) pruneLater(next);
    // Only when the cover changes (`next` is read as of that change).
  }, [url]);

  return (
    <div className={className} aria-hidden="true">
      {state.layers.map((layer) => (
        <img
          key={layer.id}
          className={`glow-layer ${layer.shown ? "is-shown" : ""}`}
          src={coverSrc(layer.url, "small")}
          alt=""
          draggable={false}
          onLoad={() => {
            if (layer.id !== state.latest) return;
            setState((s) => loaded(s, layer.id));
            pruneLater(layer.id);
          }}
          onError={() => {
            setState((s) => failed(s, layer.id));
            // The wanted cover has none: the older layers fade out.
            if (layer.id === state.latest) pruneLater(layer.id + 1);
          }}
        />
      ))}
    </div>
  );
}

/** The page's glow: its subject's cover behind the top of the page (none: nothing). */
export function PageGlow({ url }: { url: string | null }) {
  return <CoverGlow url={url} className="page-glow" />;
}
