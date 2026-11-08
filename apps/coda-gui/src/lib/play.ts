// Playing in context: thin helpers over the `playTracks` / `enqueue`
// intents, the shuffle-context hook for `s`, and track navigation. A play
// from a view names its context (album, artist, playlist, folder) for the
// play history behind Home.

import { useEffect, useRef } from "react";
import { nav, type Route } from "../state/nav";
import { switchSource } from "../state/source";
import { showUiNotice } from "../state/uiNotices";
import { browse, clearBrowseCache, toBrowseError } from "./browse";
import { intent } from "./ipc";
import type {
  Album,
  Artist,
  LocalContext,
  PlayContext,
  Playlist,
  SourceKind,
  Track,
} from "./types";

/**
 * Runs a play action that first loads from the library or server, and
 * tells the user when that fails. A stale answer (the library or server
 * changed) reloads the shown view.
 */
export function playFrom(action: () => Promise<void>) {
  action().catch((error: unknown) => {
    const e = toBrowseError(error);
    if (e.kind === "stale") {
      clearBrowseCache();
      nav.reload();
      showUiNotice("The library changed; the view was reloaded");
    } else {
      showUiNotice(`Could not play: ${e.message}`);
    }
  });
}

/** A local folder or artist, resolved and played by the engine. */
export function playLocal(context: LocalContext, shuffle: boolean, start?: number) {
  intent({ type: "playLocal", context, start: start ?? null, shuffle });
}

export function playTracks(
  tracks: Track[],
  start?: number,
  shuffle = false,
  context?: PlayContext | null,
) {
  if (tracks.length === 0) return;
  intent({ type: "playTracks", tracks, start: start ?? null, shuffle, context: context ?? null });
}

// ----- contexts for the play history -------------------------------------

export function albumContext(
  source: SourceKind,
  album: Pick<Album, "id" | "name" | "artist" | "coverUrl">,
): PlayContext {
  return {
    kind: "album",
    source,
    id: album.id,
    name: album.name,
    subtitle: album.artist,
    coverUrl: album.coverUrl,
  };
}

export function artistContext(
  source: SourceKind,
  artist: Pick<Artist, "id" | "name" | "coverUrl">,
): PlayContext {
  return { kind: "artist", source, id: artist.id, name: artist.name, coverUrl: artist.coverUrl };
}

export function playlistContext(
  source: SourceKind,
  playlist: Pick<Playlist, "id" | "name" | "coverUrl">,
): PlayContext {
  return {
    kind: "playlist",
    source,
    id: playlist.id,
    name: playlist.name,
    coverUrl: playlist.coverUrl,
  };
}

export function folderContext(path: string[]): PlayContext {
  return { kind: "folder", path };
}

export function enqueue(tracks: Track[], next: boolean) {
  if (tracks.length === 0) return;
  intent({ type: "enqueue", tracks, next });
}

/** "Shuffle all": the whole local library, or fresh random songs. */
export function shuffleAll(source: SourceKind) {
  if (source === "local") {
    intent({ type: "shuffleLibrary" });
    return;
  }
  // Already random, in served order (as in Coda).
  playFrom(async () => playTracks(await browse.randomSongs()));
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

/**
 * A shown artist name may be a link to `artistId`'s page: the id is the
 * song's artist on Navidrome, but the album artist for local tracks (the
 * tags), so a local track links its artist only when that is the same name
 * (or there is no album artist). The menu's "Go to artist" uses
 * `canGoToArtist` (it names no artist).
 */
export function canLinkArtist(track: Track): boolean {
  if (!canGoToArtist(track) || !track.artist?.trim()) return false;
  if (track.source.kind !== "local") return true;
  const fold = (s: string) => s.trim().toLocaleLowerCase();
  const albumArtist = track.albumArtist?.trim();
  return !albumArtist || fold(albumArtist) === fold(track.artist);
}

/** Opens `route`, on its own source's stack (switching source first). */
export function openRoute(route: Route & { source: SourceKind }) {
  if (route.source !== nav.source()) switchSource(route.source);
  // The switch is refused for a source that is not configured.
  if (route.source !== nav.source()) return;
  // Already shown (an album's own link inside it): no duplicate entry.
  const top = nav.top().route;
  if (
    "id" in top &&
    "id" in route &&
    top.kind === route.kind &&
    top.source === route.source &&
    top.id === route.id
  ) {
    return;
  }
  nav.push(route);
}

export function goToAlbum(track: Track) {
  if (track.albumId === null) return;
  openRoute({
    kind: "album",
    source: sourceOf(track),
    id: track.albumId,
    name: track.album ?? "Album",
  });
}

export function goToArtist(track: Track) {
  if (track.artistId === null) return;
  openRoute({
    kind: "artist",
    source: sourceOf(track),
    id: track.artistId,
    // Navidrome: the id is the song's artist; local: the album artist.
    name:
      (track.source.kind === "navidrome"
        ? (track.artist ?? track.albumArtist)
        : (track.albumArtist ?? track.artist)) ?? "Artist",
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
