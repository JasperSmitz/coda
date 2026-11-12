// Typed wrappers for the browse commands (src-tauri/src/browse/commands.rs).
// Read-only: playing goes through `intent()`.

import { invoke } from "@tauri-apps/api/core";
import type {
  Album,
  AlbumDetail,
  AlbumPage,
  AlbumSort,
  Artist,
  ArtistDetail,
  BrowseError,
  Folder,
  HomeHistory,
  LocalAlbumOrder,
  LyricsSearch,
  Playlist,
  PlaylistDetail,
  SearchResults,
  SourceKind,
  Track,
} from "./types";

/** Normalizes whatever a command rejected with into a `BrowseError`. */
export function toBrowseError(error: unknown): BrowseError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as BrowseError;
  }
  return { kind: "failed", message: typeof error === "string" ? error : "Something went wrong" };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toBrowseError(error);
  }
}

// Navidrome answers are kept briefly, so Back is instant. Random lists
// are never cached. Local reads are cheap and always fresh.
const CACHE_MS = 5 * 60 * 1000;
const cache = new Map<string, { at: number; value: Promise<unknown> }>();

function cached<T>(key: string, load: () => Promise<T>): Promise<T> {
  const hit = cache.get(key);
  if (hit && Date.now() - hit.at < CACHE_MS) return hit.value as Promise<T>;
  const value = load();
  cache.set(key, { at: Date.now(), value });
  value.catch(() => cache.delete(key));
  return value;
}

/** Forgets these cached server answers (e.g. after a playlist edit). */
export function forgetCached(...keys: string[]) {
  keys.forEach((key) => cache.delete(key));
}

/** Forgets cached server answers (a retry wants fresh data). */
export function clearBrowseCache() {
  cache.clear();
}

export const browse = {
  artists(source: SourceKind): Promise<Artist[]> {
    return source === "local"
      ? call("local_artists")
      : cached("artists", () => call("navidrome_artists"));
  },

  artist(source: SourceKind, id: string): Promise<ArtistDetail> {
    if (source === "local") return call("local_artist", { id });
    return cached(`artist:${id}`, async () => {
      const albums = await call<Album[]>("navidrome_artist", { id });
      // `getArtist` gives albums only; the page title comes from the route.
      return {
        artist: { id, name: "", albumCount: albums.length, trackCount: null, coverUrl: null },
        albums,
      };
    });
  },

  /** Navidrome only (local artists play through `playLocal`). */
  navidromeArtistTracks(id: string): Promise<Track[]> {
    return cached(`artistTracks:${id}`, () => call("navidrome_artist_tracks", { id }));
  },

  album(source: SourceKind, id: string): Promise<AlbumDetail> {
    return source === "local"
      ? call("local_album", { id })
      : cached(`album:${id}`, () => call("navidrome_album", { id }));
  },

  localAlbums(order: LocalAlbumOrder): Promise<Album[]> {
    return call("local_albums", { order });
  },

  navidromeAlbums(sort: AlbumSort, offset: number): Promise<AlbumPage> {
    const load = () => call<AlbumPage>("navidrome_albums", { sort, offset });
    // Only the alphabetical orders are stable enough to mix cached and
    // fresh pages (recently added/played shift between requests).
    const stable = sort === "name" || sort === "artist";
    return stable ? cached(`albums:${sort}:${offset}`, load) : load();
  },

  folder(path: string[]): Promise<Folder> {
    return call("local_folder", { path });
  },

  playlists(source: SourceKind): Promise<Playlist[]> {
    return source === "local"
      ? call("local_playlists")
      : cached("playlists", () => call("navidrome_playlists"));
  },

  playlist(source: SourceKind, id: string): Promise<PlaylistDetail> {
    return source === "local"
      ? call("local_playlist", { id })
      : cached(`playlist:${id}`, () => call("navidrome_playlist", { id }));
  },

  randomSongs(): Promise<Track[]> {
    return call("navidrome_random_songs");
  },

  /** Recently / most played contexts of a source (never cached). */
  homeHistory(source: SourceKind): Promise<HomeHistory> {
    return call("home_history", { source });
  },

  /** The Daily Picks of the local calendar day `date` (`YYYY-MM-DD`). */
  dailyPicks(source: SourceKind, date: string): Promise<Album[]> {
    return call("home_daily_picks", { source, date });
  },

  search(source: SourceKind, query: string): Promise<SearchResults> {
    return source === "local"
      ? call("local_search", { query })
      : cached(`search:${query}`, () => call("navidrome_search", { query }));
  },

  /** Local lyrics matching `query`, leaving out the songs in `exclude`. */
  localLyrics(query: string, exclude: string[]): Promise<LyricsSearch> {
    return call("local_lyrics_search", { query, exclude });
  },
};
