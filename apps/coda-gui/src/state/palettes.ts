// The covers' palettes (music colours), by cover URL: asked of the backend
// in batches (every URL asked for within a frame goes in one
// `cover_palettes` call of at most 64), cached in memory for the session,
// `null` until known (and for a cover without art). A URL is asked for
// once while it is in use. Only a definitive answer is cached: `null`
// means no art (or an unknown URL); a URL left out of the answer (its
// cover failed to load) or a failed call stays unknown and is asked for
// again, while still in use, after a growing pause (`retryLater`), or the
// next time it is used.
//
// `useColourScope` puts a palette's tokens on an element (lib/palette.ts).

import { useEffect, useLayoutEffect, useMemo, useRef, useSyncExternalStore, type RefObject } from "react";
import { fetchPalettes, PALETTE_BATCH } from "../lib/ipc";
import { applyScope, scopeTokens } from "../lib/palette";
import type { Palette } from "../lib/types";
import { musicAccentOn, useAccent } from "./accent";

/** Palettes kept in memory (those in use are never dropped). */
const MAX_CACHED = 5000;
/** How long requests gather before a batch is sent. */
const BATCH_MS = 16;

/** Known palettes (`null`: no art), oldest use first. */
const cache = new Map<string, Palette | null>();
/** Asked for, not answered yet. */
const pending = new Set<string>();
/** Waiting for the next batch. */
let queued: string[] = [];
let timer: number | null = null;
/** Failed attempts per URL (for the retry pause). */
const failures = new Map<string, number>();
/** The first retry pause; it doubles per failure up to `RETRY_MAX_MS`. */
const RETRY_MS = 2000;
const RETRY_MAX_MS = 60_000;
/** How many hooks use each URL now. */
const uses = new Map<string, number>();
const listeners = new Set<() => void>();
/** Bumped on every change (for `usePalettes`). */
let version = 0;

function notify() {
  version++;
  listeners.forEach((l) => l());
}

function remember(url: string, palette: Palette | null) {
  cache.delete(url);
  cache.set(url, palette);
  if (cache.size <= MAX_CACHED) return;
  for (const old of cache.keys()) {
    if (cache.size <= MAX_CACHED) break;
    if (!uses.has(old)) cache.delete(old);
  }
}

function flush() {
  timer = null;
  const urls = queued;
  queued = [];
  for (let i = 0; i < urls.length; i += PALETTE_BATCH) {
    const batch = urls.slice(i, i + PALETTE_BATCH);
    fetchPalettes(batch)
      .then((answers) => {
        for (const url of batch) {
          if (url in answers) {
            failures.delete(url);
            remember(url, answers[url]);
          } else {
            retryLater(url);
          }
        }
      })
      .catch((error: unknown) => {
        console.warn("cover palettes failed", error);
        batch.forEach(retryLater);
      })
      .finally(() => {
        batch.forEach((url) => pending.delete(url));
        notify();
      });
  }
}

/** `url` stays unknown; asked for again after a pause if it is still in use. */
function retryLater(url: string) {
  const n = (failures.get(url) ?? 0) + 1;
  failures.set(url, n);
  const ms = Math.min(RETRY_MAX_MS, RETRY_MS * 2 ** (n - 1));
  window.setTimeout(() => {
    if (uses.has(url) && !cache.has(url)) request(url);
  }, ms);
}

function request(url: string) {
  if (pending.has(url)) return;
  if (cache.has(url)) {
    // Used again: the most recently used.
    remember(url, cache.get(url) ?? null);
    return;
  }
  pending.add(url);
  queued.push(url);
  timer ??= window.setTimeout(flush, BATCH_MS);
}

function retain(url: string) {
  uses.set(url, (uses.get(url) ?? 0) + 1);
  request(url);
}

function release(url: string) {
  const n = (uses.get(url) ?? 1) - 1;
  if (n <= 0) uses.delete(url);
  else uses.set(url, n);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** `url`'s palette: `undefined` while unknown, `null` without one. */
function usePaletteState(url: string | null): Palette | null | undefined {
  useEffect(() => {
    if (!url) return;
    retain(url);
    return () => release(url);
  }, [url]);
  return useSyncExternalStore(subscribe, () => (url ? cache.get(url) : null));
}

/** The palette of the cover at `url` (`null` until known, or without art). */
export function usePalette(url: string | null): Palette | null {
  return usePaletteState(url) ?? null;
}

/** The palettes of several covers (each `null` until known). */
export function usePalettes(urls: string[]): ReadonlyMap<string, Palette | null> {
  const key = urls.join("\n");
  useEffect(() => {
    const list = key ? key.split("\n") : [];
    list.forEach(retain);
    return () => list.forEach(release);
  }, [key]);
  const seen = useSyncExternalStore(subscribe, () => version);
  return useMemo(() => {
    const map = new Map<string, Palette | null>();
    for (const url of key ? key.split("\n") : []) map.set(url, cache.get(url) ?? null);
    return map;
    // `seen` changes when an answer arrives.
  }, [key, seen]);
}

/**
 * Colours the element in `ref` with the cover at `url` (lib/palette.ts):
 * its tint always, its accent with "From the music". While a new cover's
 * palette loads, the last one stays (no detour through the house accent);
 * no cover, or one without art, clears the scope. Runs after every render
 * of the caller (cheap: only changed tokens are written).
 */
export function useColourScope(ref: RefObject<HTMLElement | null>, url: string | null) {
  const state = usePaletteState(url);
  const { shown } = useAccent();
  const last = useRef<Palette | null>(null);
  const palette = url === null ? null : state === undefined ? last.current : state;
  const music = musicAccentOn(shown);
  useLayoutEffect(() => {
    last.current = palette;
    const element = ref.current;
    if (element) applyScope(element, scopeTokens(palette, music));
  });
}
