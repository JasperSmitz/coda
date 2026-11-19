// The crossfading cover layers behind the page and the player
// (components/PageGlow), as pure state changes (`node --test scripts/`
// tests them in scripts/glow.test.mjs).
//
// A new cover is added hidden and fades in over the shown ones once it
// has loaded; the component then prunes the older ones after the fade. No
// cover, or the newest cover failing to load, hides every layer (they fade
// out) and the component prunes them after the fade, so an album without
// art never keeps the previous album's glow.

export interface Layer {
  id: number;
  url: string;
  shown: boolean;
}

export interface Layers {
  layers: Layer[];
  /** The layer of the cover wanted now (-1: none); only it may fade in. */
  latest: number;
  /** The next layer's id. */
  next: number;
}

export function initialLayers(url: string | null): Layers {
  return url ? { layers: [{ id: 0, url, shown: true }], latest: 0, next: 1 } : { layers: [], latest: -1, next: 1 };
}

/**
 * The wanted cover changed: a new hidden layer over the shown ones (at most
 * two kept; one still loading or fading out is replaced), or, for none,
 * every layer hidden.
 */
export function setUrl(state: Layers, url: string | null): Layers {
  if (!url) return hideAll(state);
  const id = state.next;
  return {
    layers: [...state.layers.filter((l) => l.shown).slice(-2), { id, url, shown: false }],
    latest: id,
    next: id + 1,
  };
}

/** Layer `id` loaded: shown if it is still the wanted one. */
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

/** Drops the layers older than `keepFrom` (after a fade). */
export function prune(state: Layers, keepFrom: number): Layers {
  const layers = state.layers.filter((l) => l.id >= keepFrom);
  return layers.length === state.layers.length ? state : { ...state, layers };
}

function hideAll(state: Layers): Layers {
  return { ...state, latest: -1, layers: state.layers.map((l) => (l.shown ? { ...l, shown: false } : l)) };
}
