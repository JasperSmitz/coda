// The playing track's lyrics for lyrics mode: fetched (`now_playing_lyrics`)
// while the mode is on and the track changes, with the last ~20 results
// kept in memory (no disk cache). A result that arrives for a track that is
// no longer playing is dropped, so the view never shows another track's
// lyrics. Errors are not cached: the next time the track plays, it asks
// again. Navidrome keys carry the server generation, and App clears the
// cache when the server changes (`clearLyricsCache`).

import { useEffect, useRef, useState } from "react";
import { fetchLyrics } from "../lib/ipc";
import { LyricsCache, lyricsKey } from "../lib/lyrics";
import type { Lyrics, Track } from "../lib/types";

/** Loading for longer than this shows a quiet "Loading lyrics…". */
const SLOW_MS = 300;

const cache = new LyricsCache(20);

/** Forgets every cached result (the Navidrome server changed). */
export function clearLyricsCache(): void {
  cache.clear();
}

export type LyricsState =
  | { status: "off" }
  | { status: "loading"; slow: boolean }
  | { status: "none" }
  | { status: "error" }
  | { status: "ready"; lyrics: Lyrics };

type Result =
  | { key: string; status: "loading"; slow: boolean }
  | { key: string; status: "none" }
  | { key: string; status: "error" }
  | { key: string; status: "ready"; lyrics: Lyrics };

function fromCache(key: string, value: Lyrics | null): Result {
  return value ? { key, status: "ready", lyrics: value } : { key, status: "none" };
}

export function useLyrics(
  track: Track | null,
  enabled: boolean,
  serverGeneration: number,
): LyricsState {
  const key = track ? lyricsKey(track.source, serverGeneration) : null;
  const latest = useRef(track);
  latest.current = track;
  const [result, setResult] = useState<Result | null>(null);

  useEffect(() => {
    const track = latest.current;
    if (!enabled || key == null || !track) return;
    const cached = cache.get(key);
    if (cached !== undefined) {
      setResult(fromCache(key, cached));
      return;
    }
    let current = true;
    setResult({ key, status: "loading", slow: false });
    const slow = window.setTimeout(() => {
      if (current) setResult((r) => (r?.key === key && r.status === "loading" ? { ...r, slow: true } : r));
    }, SLOW_MS);
    fetchLyrics(track).then(
      (lyrics) => {
        cache.set(key, lyrics);
        if (current) setResult(fromCache(key, lyrics));
      },
      (error: unknown) => {
        console.warn("lyrics failed", error);
        if (current) setResult({ key, status: "error" });
      },
    );
    return () => {
      current = false;
      window.clearTimeout(slow);
    };
  }, [enabled, key]);

  if (!enabled || !track) return { status: "off" };
  // A local track without a library id yet (a restored queue before the
  // library is loaded): blank, as while loading, until the id comes.
  if (key == null) return { status: "loading", slow: false };
  // A result for another track is never shown.
  if (!result || result.key !== key) return { status: "loading", slow: false };
  switch (result.status) {
    case "loading":
      return { status: "loading", slow: result.slow };
    case "ready":
      return { status: "ready", lyrics: result.lyrics };
    default:
      return { status: result.status };
  }
}
