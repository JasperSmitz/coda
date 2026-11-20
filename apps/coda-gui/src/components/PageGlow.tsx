// Covers as blurred light (DESIGN.md "Colour"): `PageGlow` behind the top
// of a page (its subject's cover, very blurred, under a radial wash of
// its tint, fading out by ~600px), and `CoverGlow`, the same crossfading
// layers for the player's background (the whole cover stretched across
// the strip).
//
// Each layer carries its own wash: its cover's tint at the alpha the
// backend computed for that cover and that surface (`pageWash`,
// `playerWash`; core `cover::palette::wash_alpha`), so a dark cover stays
// colourful and a white one is washed enough for the text. A layer is
// opaque (the base or its tint under the cover), so the cover and its
// wash crossfade as one, and no two layers add up to something lighter
// than either.
//
// A layer fades in once both its image and its palette are there (a
// palette still unknown a second after the image: the fallback wash, kept
// for that layer), by a Web Animation from its current opacity (0 when
// new), so it blends over its full duration even when the image came from
// the cache (a CSS transition would not run if the shown state were
// committed before the hidden one was drawn). Hiding fades it out from
// wherever it is, so a layer interrupted mid-fade never jumps. The layer
// there when the glow mounts (app start, the player and the page back
// after Now Playing) appears at once when ready: no fade in from the base. The newest layer fades in over
// the others, which go after the fade (lib/glowLayers.ts); no cover, or a
// new one that fails to load, fades them all out. At most three layers.
// With reduced motion, at once. Purely decorative: hidden from assistive
// tech, no pointer events, each layer its own compositing layer (CSS), so
// scrolling or anything moving over it does not repaint the blur.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { coverSrc } from "../lib/ipc";
import { failed, initialLayers, loaded, prune, setUrl, type Layer, type Layers } from "../lib/glowLayers";
import { cssMs, easing } from "../lib/motion";
import { withAlpha } from "../lib/palette";
import type { Palette } from "../lib/types";
import { usePaletteState } from "../state/palettes";

/** The glow's crossfade, in ms (tokens.css `--motion-glow`; 0 with reduced motion). */
function fadeMs(): number {
  return cssMs("--motion-glow", 600);
}

/** Which surface draws the layers (its wash and the CSS that draws it). */
export type GlowSurface = "page" | "player";

/**
 * The wash for a cover without a palette (none for its URL): the base,
 * heavy enough for any cover (a white one needs ~0.73 on the page, ~0.84
 * in the player: core's tests).
 */
const FALLBACK_WASH: Record<GlowSurface, number> = { page: 0.75, player: 0.86 };

/** How long a loaded image waits for its palette before the fallback wash. */
const PALETTE_WAIT_MS = 1000;

/** How far the page's radial wash darkens towards its middle stop. */
const PAGE_MID_DARKEN = 0.35;

/** A layer's wash, as custom properties for its CSS (app.css `.glow-layer`). */
function washStyle(palette: Palette | null, surface: GlowSurface): CSSProperties {
  const alpha = palette ? (surface === "page" ? palette.pageWash : palette.playerWash) : FALLBACK_WASH[surface];
  const mid = alpha + (1 - alpha) * PAGE_MID_DARKEN;
  const tint = (a: number) =>
    palette ? withAlpha(palette.tint, a) : `color-mix(in srgb, var(--bg) ${Math.round(a * 100)}%, transparent)`;
  return {
    "--layer-tint": palette ? palette.tint : "var(--bg)",
    "--layer-wash": tint(alpha),
    "--layer-wash-mid": tint(mid),
  } as CSSProperties;
}

export function CoverGlow({
  url,
  className,
  surface,
}: {
  url: string | null;
  className: string;
  surface: GlowSurface;
}) {
  const [state, setState] = useState<Layers>(() => initialLayers(url));
  // The layer there at mount (appears without a fade).
  const [mountLayer] = useState(() => state.layers[0]?.id ?? -1);
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

  const latest = state.latest;
  return (
    <div className={className} aria-hidden="true">
      {state.layers.map((layer) => (
        <GlowLayer
          key={layer.id}
          layer={layer}
          surface={surface}
          instant={layer.id === mountLayer}
          onReady={() => {
            if (layer.id !== latest) return;
            setState((s) => loaded(s, layer.id));
            pruneLater(layer.id);
          }}
          onFailed={() => {
            setState((s) => failed(s, layer.id));
            // The wanted cover has none: the older layers fade out.
            if (layer.id === latest) pruneLater(layer.id + 1);
          }}
        />
      ))}
    </div>
  );
}

/**
 * One cover and its wash. Ready (reported once) when its image has loaded
 * and its palette is known; shown once ready and wanted (`layer.shown`).
 */
function GlowLayer({
  layer,
  surface,
  instant,
  onReady,
  onFailed,
}: {
  layer: Layer;
  surface: GlowSurface;
  /** The first appearance is at once (the layer was there at mount). */
  instant: boolean;
  onReady: () => void;
  onFailed: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const fade = useRef<Animation | null>(null);
  const appeared = useRef(false);
  const palette = usePaletteState(layer.url);
  const [imageLoaded, setImageLoaded] = useState(false);
  const [gaveUp, setGaveUp] = useState(false);
  const waiting = imageLoaded && palette === undefined && !gaveUp;
  useEffect(() => {
    if (!waiting) return;
    const timer = window.setTimeout(() => setGaveUp(true), PALETTE_WAIT_MS);
    return () => window.clearTimeout(timer);
  }, [waiting]);
  const ready = imageLoaded && (palette !== undefined || gaveUp);
  const visible = ready && layer.shown;
  // The wash it became ready with stays (a palette arriving later would
  // change a shown layer at once).
  const [wash, setWash] = useState<Palette | null | undefined>(undefined);
  if (ready && wash === undefined) setWash(palette ?? null);

  const report = useRef(onReady);
  report.current = onReady;
  useEffect(() => {
    if (ready) report.current();
  }, [ready]);

  // Fades to the wanted opacity from wherever the layer is now.
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const target = visible ? 1 : 0;
    const from = Number(getComputedStyle(element).opacity);
    fade.current?.cancel();
    fade.current = null;
    element.style.opacity = String(target);
    const first = visible && !appeared.current;
    if (visible) appeared.current = true;
    const ms = first && instant ? 0 : fadeMs();
    if (ms <= 0 || from === target || typeof element.animate !== "function") return;
    fade.current = element.animate([{ opacity: from }, { opacity: target }], {
      duration: ms * Math.abs(target - from),
      easing: easing("--ease-in-out"),
    });
  }, [visible]);
  useEffect(() => () => fade.current?.cancel(), []);

  return (
    <div ref={ref} className="glow-layer" style={washStyle(wash !== undefined ? wash : (palette ?? null), surface)}>
      <img
        className="glow-image"
        src={coverSrc(layer.url, "small")}
        alt=""
        draggable={false}
        onLoad={() => setImageLoaded(true)}
        onError={onFailed}
      />
    </div>
  );
}

/** The page's glow: its subject's cover behind the top of the page (none: nothing). */
export function PageGlow({ url }: { url: string | null }) {
  return <CoverGlow url={url} className="page-glow" surface="page" />;
}
