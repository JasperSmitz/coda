// The crossfading cover layers behind the page and the player
// (components/PageGlow), as pure state changes (`node --test scripts/`
// tests them in scripts/glow.test.mjs).
//
// A new cover is added hidden and fades in over the shown ones once it
// is ready (its image loaded and its palette known: the component tells
// `loaded`); the component then prunes the older ones after the fade. No
// cover, or the newest cover failing to load, hides every layer (they fade
// out) and the component prunes them after the fade, so an album without
// art never keeps the previous album's glow.
//
// At most `MAX_LAYERS`. A layer still loading is invisible, so a newer
// cover replaces it; a shown one (fully or fading in) or one fading out
// (`leaving`) is never dropped before its fade (or the one over it) has
// finished, or the backdrop would jump. So when every place holds such a
// layer (covers changing faster than the fade), the newest cover waits
// (`queued`) until a fade has finished and its prune makes room; a newer
// cover meanwhile takes its place in the queue.

/** The most layers at once. */
export const MAX_LAYERS = 3;

export interface Layer {
  id: number;
  url: string;
  shown: boolean;
  /** Was shown and fades out now (no cover wanted, or it failed). */
  leaving?: boolean;
}

export interface Layers {
  layers: Layer[];
  /** The layer of the cover wanted now (-1: none, or it is queued); only it may fade in. */
  latest: number;
  /** The next layer's id. */
  next: number;
  /** The cover wanted now, waiting for room (`null`: none waits). */
  queued: string | null;
}

export function initialLayers(url: string | null): Layers {
  return url
    ? { layers: [{ id: 0, url, shown: true }], latest: 0, next: 1, queued: null }
    : { layers: [], latest: -1, next: 1, queued: null };
}

/**
 * The wanted cover changed: a new hidden layer over the shown ones (one
 * still loading is replaced), queued while there is no room; for none,
 * every layer hidden.
 */
export function setUrl(state: Layers, url: string | null): Layers {
  if (!url) return hideAll(state);
  const kept = state.layers.filter((l) => l.shown || l.leaving);
  return add({ ...state, layers: kept, latest: -1, queued: null }, url);
}

/** Adds `url` as the wanted layer if there is room, else queues it. */
function add(state: Layers, url: string): Layers {
  if (state.layers.length >= MAX_LAYERS) return { ...state, latest: -1, queued: url };
  const id = state.next;
  return {
    layers: [...state.layers, { id, url, shown: false }],
    latest: id,
    next: id + 1,
    queued: null,
  };
}

/** Layer `id` is ready (image and palette): shown if it is still the wanted one. */
export function loaded(state: Layers, id: number): Layers {
  if (id !== state.latest) return state;
  return { ...state, layers: state.layers.map((l) => (l.id === id ? { ...l, shown: true } : l)) };
}

/**
 * Layer `id` failed to load: dropped. If it was the wanted cover, the
 * others fade out as for no cover.
 */
export function failed(state: Layers, id: number): Layers {
  const rest = { ...state, layers: state.layers.filter((l) => l.id !== id) };
  return id === state.latest ? hideAll(rest) : rest;
}

/** Drops the layers older than `keepFrom` (after a fade); a queued cover then gets its place. */
export function prune(state: Layers, keepFrom: number): Layers {
  const layers = state.layers.filter((l) => l.id >= keepFrom);
  const pruned = layers.length === state.layers.length ? state : { ...state, layers };
  return pruned.queued && pruned.layers.length < MAX_LAYERS ? add(pruned, pruned.queued) : pruned;
}

function hideAll(state: Layers): Layers {
  return {
    ...state,
    latest: -1,
    queued: null,
    layers: state.layers.map((l) => (l.shown ? { ...l, shown: false, leaving: true } : l)),
  };
}
