// Playing in context: thin helpers over the `playTracks` / `enqueue`
// intents, the shuffle-context hook for `s`, and track navigation.

import { useEffect, useRef } from "react";
import { nav } from "../state/nav";
import { browse } from "./browse";
import { intent } from "./ipc";
import type { SourceKind, Track } from "./types";

export function playTracks(tracks: Track[], start?: number, shuffle = false) {
  if (tracks.length === 0) return;
  intent({ type: "playTracks", tracks, start: start ?? null, shuffle });
}

export function enqueue(tracks: Track[], next: boolean) {
  if (tracks.length === 0) return;
  intent({ type: "enqueue", tracks, next });
}

/** "Shuffle all": the whole local library, or fresh random songs. */
export async function shuffleAll(source: SourceKind): Promise<void> {
  if (source === "local") {
    intent({ type: "shuffleLibrary" });
    return;
  }
  // Already random, in served order (as in Coda).
  playTracks(await browse.randomSongs());
}

export function sourceOf(track: Track): SourceKind {
  return track.source.kind;
}

/** The same track (by source id), e.g. to mark the playing row. */
export function sameTrack(a: Track | null | undefined, b: Track | null | undefined): boolean {
  if (!a || !b) return false;
  if (a.source.kind === "local" && b.source.kind === "local") {
    return a.source.id !== null && a.source.id === b.source.id;
  }
  if (a.source.kind === "navidrome" && b.source.kind === "navidrome") {
    return a.source.songId === b.source.songId;
  }
  return false;
}

export function canGoToAlbum(track: Track): boolean {
  return track.albumId !== null;
}

export function canGoToArtist(track: Track): boolean {
  return track.artistId !== null;
}

export function goToAlbum(track: Track) {
  if (track.albumId === null) return;
  nav.push({
    kind: "album",
    source: sourceOf(track),
    id: track.albumId,
    name: track.album ?? "Album",
  });
}

export function goToArtist(track: Track) {
  if (track.artistId === null) return;
  nav.push({
    kind: "artist",
    source: sourceOf(track),
    id: track.artistId,
    name: track.albumArtist ?? track.artist ?? "Artist",
  });
}

// ----- `s`: shuffle the shown context ----------------------------------

let shuffleHandler: (() => void) | null = null;

/** `s` (DESIGN.md): shuffles whatever the shown view plays. */
export function shuffleContext() {
  shuffleHandler?.();
}

/** Registers what `s` shuffles while the calling view is shown. */
export function useShuffleContext(handler: (() => void) | null) {
  const ref = useRef(handler);
  ref.current = handler;
  const active = handler !== null;
  useEffect(() => {
    if (!active) return;
    const run = () => ref.current?.();
    shuffleHandler = run;
    return () => {
      if (shuffleHandler === run) shuffleHandler = null;
    };
  }, [active]);
}
