// Hand-maintained mirror of the backend DTOs in `src-tauri/src/dto.rs`.
// Keep the two in step: field names are the camelCase serde output.

/**
 * Where a track comes from. Sent back unchanged inside a `TrackRef`. A
 * local `id` is opaque (`null` for a queued file the library does not hold).
 */
export type Source =
  | { kind: "local"; id: string | null }
  | { kind: "navidrome"; songId: string; coverArt: string | null };

export interface Track {
  source: Source;
  title: string;
  artist: string | null;
  album: string | null;
  albumArtist: string | null;
  trackNo: number | null;
  discNo: number | null;
  durationMs: number | null;
  /** Base cover URL; append `/small` or `/large` (see `coverSrc`). */
  coverUrl: string | null;
  /** The album / artist to go to (same source as the track), if known. */
  albumId: string | null;
  artistId: string | null;
}

/**
 * A track to play or enqueue. Any `Track` received from the backend is a
 * valid `TrackRef` (the backend ignores `coverUrl`).
 */
export type TrackRef = Pick<Track, "source"> & Partial<Omit<Track, "source" | "coverUrl">>;

export interface QueueItem {
  /** Stable for the entry's lifetime in this run (e.g. `q12`). */
  key: string;
  track: Track;
}

export type Phase = "starting" | "ready" | "shuttingDown";
export type Status = "stopped" | "loading" | "playing" | "paused" | "unavailable";
export type Repeat = "off" | "all" | "one";

export interface Scan {
  state: "noLibrary" | "scanning" | "finished" | "failed";
  tracks: number;
  playlists: number;
  skipped: number;
  errors: number;
}

export interface Navidrome {
  state: "notConfigured" | "connecting" | "connected" | "failed";
  message: string | null;
}

export type ProblemKind = "noConfig" | "config" | "mpvMissing" | "playback" | "navidrome";

export interface Problem {
  kind: ProblemKind;
  message: string;
}

export interface Notice {
  id: number;
  text: string;
}

export interface Snapshot {
  phase: Phase;
  status: Status;
  currentKey: string | null;
  playingKey: string | null;
  upNextKey: string | null;
  playing: Track | null;
  /** `null` in events when the entries did not change: keep the last. */
  queue: QueueItem[] | null;
  volume: number;
  repeat: Repeat;
  scan: Scan;
  navidrome: Navidrome;
  problems: Problem[];
  notices: Notice[];
  visualizerAvailable: boolean;
}

export interface Progress {
  positionMs: number | null;
  durationMs: number | null;
}

export interface VisualizerFrame {
  levels: [number, number];
  /** 7 bands, low to high; `null` above the track's Nyquist frequency. */
  bands: (number | null)[];
}

export type Intent =
  | { type: "togglePause" }
  | { type: "next" }
  | { type: "previous" }
  | { type: "seekTo"; seconds: number }
  | { type: "seekBy"; seconds: number }
  | { type: "setVolume"; volume: number }
  | { type: "cycleRepeat" }
  | { type: "playEntry"; key: string }
  | { type: "removeEntry"; key: string }
  | { type: "clearQueue" }
  | { type: "setVisualizerVisible"; visible: boolean }
  | { type: "playTracks"; tracks: TrackRef[]; start?: number | null; shuffle?: boolean }
  | { type: "enqueue"; tracks: TrackRef[]; next?: boolean }
  | { type: "dismissNotice"; id: number }
  | { type: "shuffleLibrary" };

// ----- Browse (src-tauri/src/browse/dto.rs) -----

export type SourceKind = "local" | "navidrome";

export interface Artist {
  id: string;
  name: string;
  albumCount: number | null;
  /** Local only. */
  trackCount: number | null;
  coverUrl: string | null;
}

export interface Album {
  id: string;
  name: string;
  artist: string | null;
  artistId: string | null;
  year: number | null;
  trackCount: number | null;
  durationMs: number | null;
  coverUrl: string | null;
}

export interface Playlist {
  id: string;
  name: string;
  trackCount: number | null;
  durationMs: number | null;
  coverUrl: string | null;
  owner: string | null;
  readonly: boolean;
}

export interface ArtistDetail {
  artist: Artist;
  albums: Album[];
}

export interface AlbumDetail {
  album: Album;
  tracks: Track[];
}

export type Hidden = { kind: "none" } | { kind: "some"; count: number } | { kind: "suspect" };

export interface PlaylistDetail {
  playlist: Playlist;
  tracks: Track[];
  missing: number;
  truncated: [number, number] | null;
  hidden: Hidden;
  warning: string | null;
}

export interface Folder {
  path: string[];
  dirs: string[];
  tracks: Track[];
}

export interface AlbumPage {
  albums: Album[];
  nextOffset: number | null;
}

export interface SearchResults {
  artists: Artist[];
  albums: Album[];
  tracks: Track[];
  totalTracks: number;
}

export type LocalAlbumOrder = "name" | "artist";
export type AlbumSort = "name" | "artist" | "newest" | "frequent" | "recent" | "random";

export interface BrowseError {
  kind: "notConfigured" | "stale" | "notFound" | "failed";
  message: string;
}
