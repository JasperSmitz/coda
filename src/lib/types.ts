// Hand-maintained mirror of the backend DTOs in `src-tauri/src/dto.rs`.
// Keep the two in step: field names are the camelCase serde output.

/** Where a track comes from. Sent back unchanged inside a `TrackRef`. */
export type Source =
  | { kind: "local"; path: string }
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
  | { type: "dismissNotice"; id: number };

// ----- settings (`src-tauri/src/settings.rs`) -----

/** The server as the settings show it (never a password). */
export type ServerSettings =
  | { kind: "notConfigured" }
  /** Written by Coda (keychain): can be edited and disconnected. */
  | { kind: "coda"; url: string; username: string }
  /** Set outside Coda; `by` names the variables or keys. */
  | { kind: "outside"; url: string | null; username: string | null; by: string[] };

export interface Settings {
  configPath: string | null;
  /** `config.toml` is a symlink; a save replaces the link with a regular file. */
  configIsLink: boolean;
  /** As written in the file (`~/Music` stays `~/Music`). */
  musicDir: string | null;
  /** `CODA_MUSIC_DIR` is set and used instead. */
  musicDirEnv: boolean;
  server: ServerSettings;
  /** Variables that keep a server from being saved here. */
  serverBlockedBy: string[];
  scrobble: boolean;
  coverArt: boolean;
  /** Why the file could not be read (nothing can be saved then). */
  problem: string | null;
}

/** A server as typed; the password goes to the keychain only. */
export interface ServerInput {
  url: string;
  username: string;
  password: string;
}
