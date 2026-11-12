// Lyrics mode's pure logic: which line is current, when auto-follow
// resumes after a manual scroll, where to scroll, and a small in-memory
// cache of results. No React and no IPC here, so `node --test scripts/`
// can test it directly (Node strips the types).

import type { LyricLine, Lyrics, Source } from "./types";

/**
 * The index of the line playing at `positionMs`: the last line whose start
 * is at or before it. -1 before the first line, for untimed lyrics and
 * without a position. `lines` are sorted by start (the backend's rule).
 */
export function currentLineIndex(
  lines: readonly Pick<LyricLine, "startMs">[],
  positionMs: number | null,
): number {
  if (positionMs == null || !Number.isFinite(positionMs)) return -1;
  let lo = 0;
  let hi = lines.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const start = lines[mid].startMs;
    if (start == null) return -1;
    if (start <= positionMs) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}

/** How long a manual scroll pauses auto-follow. */
export const FOLLOW_PAUSE_MS = 4000;

/** Auto-follow is paused until this time after a manual scroll at `now`. */
export function followPausedUntil(now: number): number {
  return now + FOLLOW_PAUSE_MS;
}

/** Whether auto-follow is on at `now` (`pausedUntil` 0: never paused). */
export function isFollowing(pausedUntil: number, now: number): boolean {
  return now >= pausedUntil;
}

/** Where to scroll so the line's middle sits about a third of the way down. */
export function followScrollTop(
  lineTop: number,
  lineHeight: number,
  viewportHeight: number,
): number {
  return Math.max(0, Math.round(lineTop + lineHeight / 2 - viewportHeight / 3));
}

/**
 * The cache key of a track's lyrics; `null` for a local track without a
 * library id (not, or not yet, in the scanned library). Local ids carry
 * the library generation; Navidrome song ids get the server generation
 * (`Snapshot.browse.server`), so another server's song never matches.
 */
export function lyricsKey(source: Source, serverGeneration: number): string | null {
  if (source.kind === "local") return source.id == null ? null : `local:${source.id}`;
  return `navidrome:${serverGeneration}:${source.songId}`;
}

/** How long a programmatic (follow) scroll may take before it is over. */
export const FOLLOW_SCROLL_MAX_MS = 1500;

/** A scroll the view started itself: where to, and until when at most. */
export interface FollowScroll {
  target: number;
  until: number;
}

/**
 * Classifies a `scroll` event at `scrollTop`: part of the view's own
 * scroll in flight (`follow`, which ends once the target is reached or
 * its time is up), or manual (`manual`: a scrollbar drag or anything else
 * the user did). Returns what to keep as the scroll in flight.
 */
export function classifyScroll(
  inFlight: FollowScroll | null,
  scrollTop: number,
  now: number,
): { manual: boolean; inFlight: FollowScroll | null } {
  if (inFlight == null || now > inFlight.until) return { manual: true, inFlight: null };
  const arrived = Math.abs(scrollTop - inFlight.target) < 2;
  return { manual: false, inFlight: arrived ? null : inFlight };
}

/** The last few results (`null`: the track has no lyrics), oldest dropped first. */
export class LyricsCache {
  private readonly entries = new Map<string, Lyrics | null>();
  private readonly limit: number;

  constructor(limit = 20) {
    this.limit = limit;
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** The cached result (`undefined` if none), now the most recent. */
  get(key: string): Lyrics | null | undefined {
    if (!this.entries.has(key)) return undefined;
    const value = this.entries.get(key) ?? null;
    this.entries.delete(key);
    this.entries.set(key, value);
    return value;
  }

  set(key: string, value: Lyrics | null): void {
    this.entries.delete(key);
    this.entries.set(key, value);
    while (this.entries.size > this.limit) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) break;
      this.entries.delete(oldest);
    }
  }

  clear(): void {
    this.entries.clear();
  }

  get size(): number {
    return this.entries.size;
  }
}
