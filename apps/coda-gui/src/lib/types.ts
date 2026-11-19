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
  /** In tag order; a Navidrome track keeps them in the queue. */
  genres: string[];
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

/** One line of lyrics (`LyricLineDto`); `startMs` is set when timed. */
export interface LyricLine {
  startMs: number | null;
  text: string;
}

/**
 * A track's lyrics (`LyricsDto`). When `timed`, every line has a
 * `startMs` and the lines are sorted by it.
 */
export interface Lyrics {
  timed: boolean;
  lines: LyricLine[];
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
  /** Browse generations: a change makes that source's ids and cached answers stale. */
  browse: { library: number; server: number };
  /** The play history's revision: a change means Home's history rows may have changed. */
  historyRevision: number;
  /** The context whose queue is playing (Home's Now Playing). */
  playingContext: PlayingContext | null;
  /** The mix that owns the queue (or is loading its songs), if any. */
  mix: ActiveMix | null;
}

/** The active mix, for "Mix: <name>" (`ActiveMixDto`). */
export interface ActiveMix {
  name: string;
  /** The saved mix's id; `null` for a quick mix. */
  id: string | null;
  /** Its songs are still being fetched (Navidrome): not playing yet. */
  loading: boolean;
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
  | {
      type: "playTracks";
      tracks: TrackRef[];
      start?: number | null;
      shuffle?: boolean;
      /** The view it plays from, for the play history (Home). */
      context?: PlayContext | null;
      /** Resume (Home): seek here, in ms, once the start track plays, if it is `seekTrack` (a track's id as `HomeStop.trackId` names it). */
      seekMs?: number | null;
      seekTrack?: string | null;
    }
  | { type: "enqueue"; tracks: TrackRef[]; next?: boolean }
  | { type: "dismissNotice"; id: number }
  | { type: "shuffleLibrary" }
  /** Moves the entries (one block, in queue order) before the entry `before` (`null`: the end). */
  | { type: "moveEntries"; keys: string[]; before: string | null }
  | { type: "removeEntries"; keys: string[] }
  /** Moves the entries right after the current one. */
  | { type: "playNextEntries"; keys: string[] }
  | { type: "shuffleUpcoming" }
  | {
      type: "playLocal";
      context: LocalContext;
      start?: number | null;
      shuffle?: boolean;
      /** Resume (Home): as `playTracks`'s. */
      seekMs?: number | null;
      seekTrack?: string | null;
    }
  /** Plays the saved mix `id` (read again from the mixes file). */
  | { type: "playMix"; id: string }
  /** Plays a recipe without saving it (a quick mix). */
  | { type: "playQuickMix"; mix: MixInput };

// ----- Mixes (src-tauri/src/mixes.rs) -----

/**
 * One part of a mix. Local: by name. Navidrome: a genre by the server's
 * exact value; an artist or album by id plus name (an album with its
 * album artist).
 */
export type Ingredient =
  | { kind: "genre"; name: string }
  | { kind: "artist"; name: string; id?: string | null }
  | { kind: "album"; artist: string; name: string; id?: string | null };

/** A saved mix of one source (Navidrome: the current server's). */
export interface Mix {
  id: string;
  name: string;
  source: SourceKind;
  ingredients: Ingredient[];
}

/** A recipe from the editor, to save or to play as a quick mix. */
export interface MixInput {
  name: string;
  source: SourceKind;
  ingredients: Ingredient[];
}

export type MixErrorKind =
  | "invalid"
  | "notFound"
  | "otherSource"
  | "full"
  | "alreadyIn"
  | "mixFull"
  | "newer"
  | "notConfigured"
  | "failed";

export interface MixError {
  kind: MixErrorKind;
  message: string;
}

/**
 * What a play intent plays from (`PlayContextDto`): an album, artist or
 * playlist by the id the view has (local ids of the current library), or
 * a local folder. Navidrome contexts also say how to show them.
 */
export type PlayContext =
  | {
      kind: "album" | "artist" | "playlist";
      source: SourceKind;
      id: string;
      name?: string | null;
      subtitle?: string | null;
      coverUrl?: string | null;
    }
  | { kind: "folder"; path: string[] };

/** A local context the engine resolves and plays itself. */
export type LocalContext =
  | { kind: "folder"; path: string[] }
  | { kind: "artist"; id: string }
  | { kind: "genre"; id: string };

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
  /** One genre for a list row (local: the first tagged; Navidrome: the album's). */
  genre: string | null;
  /**
   * Listens: Navidrome the server's `playCount` (when it sends one);
   * local the play history's, only in the Most played order.
   */
  playCount: number | null;
}

/** One of an artist's most played songs (`artist_most_played`). */
export interface TopTrack {
  track: Track;
  /** Listens this app counted. */
  listens: number;
}

export interface Playlist {
  id: string;
  name: string;
  trackCount: number | null;
  durationMs: number | null;
  coverUrl: string | null;
  /** Local: the covers of its first different albums (up to four, a 2×2 mosaic); Navidrome: none (its cover is the server's mosaic). */
  covers: string[];
  owner: string | null;
  readonly: boolean;
  /** Navidrome: the connected user may change it (always `false` for local). */
  editable: boolean;
  /** Navidrome: why it cannot be changed. */
  readOnlyReason: string | null;
}

export interface ArtistDetail {
  artist: Artist;
  albums: Album[];
}

/**
 * A genre. A Navidrome genre's `id` is the server's exact value (what its
 * albums and songs are asked for by); `name` is for display.
 */
export interface Genre {
  id: string;
  name: string;
  albumCount: number | null;
  trackCount: number | null;
  /** Local: its first album's cover (its tile's colours); Navidrome: `null` (the tile looks one up). */
  coverUrl: string | null;
}

export interface AlbumDetail {
  album: Album;
  tracks: Track[];
  /** The album's genres in tag order, linking to their genre pages. */
  genres: Genre[];
}

/** A local genre and its albums (by artist, then album). */
export interface GenreDetail {
  genre: Genre;
  albums: Album[];
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

/** One part of a lyric snippet; `matched` parts are the search phrase. */
export interface SnippetPart {
  text: string;
  matched: boolean;
}

export interface LyricsMatch {
  track: Track;
  snippet: SnippetPart[];
}

/**
 * A lyrics search: at most 50 matches, and how many there are in all.
 * `indexing` (Navidrome only): songs are left to check, `done` of `total`
 * checked so far (`total` 0: the index is still loading).
 */
export interface LyricsSearch {
  matches: LyricsMatch[];
  total: number;
  indexing: { done: number; total: number } | null;
}

/** The Navidrome lyrics index (`LyricsIndexDto`, the `lyrics-index` event). */
export interface LyricsIndexStatus {
  state: "off" | "noServer" | "loading" | "idle" | "albums" | "songs" | "upToDate" | "paused";
  pause: "unreachable" | "rejected" | "failed" | null;
  /** This sync's progress in albums (state `albums`); see `checked`/`songs` for songs. */
  done: number;
  todo: number;
  songs: number;
  checked: number;
  withLyrics: number;
}

export type LocalAlbumOrder = "name" | "artist" | "year" | "mostPlayed";
export type AlbumSort = "name" | "artist" | "newest" | "frequent" | "recent" | "random";

export interface BrowseError {
  kind: "notConfigured" | "stale" | "notFound" | "failed";
  message: string;
}

// ----- Home (src-tauri/src/history.rs) -----

export type ContextKind = "album" | "artist" | "playlist" | "folder" | "mix";

/** A context as the UI names it (`HomeItem`'s source, kind and id). */
export interface PlayingContext {
  source: SourceKind;
  kind: ContextKind;
  id: string;
}

/** Where a Home item stopped. */
export interface HomeStop {
  /** The entry's place among the context's tracks. */
  index: number;
  /** The context's tracks. */
  total: number;
  /** The entry's track as a `Track` names it (local id or song id); null: gone (then `index` is where it was). */
  trackId: string | null;
  positionMs: number;
}

/** One item of Where you left off: a context from the play history, resolved now. */
export interface HomeItem {
  source: SourceKind;
  kind: ContextKind;
  /** The id to open or play it by (a folder: its path joined by `/`; a mix: its id). */
  id: string;
  /** Folders only. */
  path: string[] | null;
  name: string;
  subtitle: string | null;
  /** Albums: the artist, when known. */
  artistId: string | null;
  coverUrl: string | null;
  /** When it was last played (unix seconds). */
  playedAt: number;
  stop: HomeStop | null;
}

export interface HomeHistory {
  /** Newest first, at most 8. */
  recent: HomeItem[];
}

/** One album of a mood card. */
export interface MoodAlbum {
  id: string;
  name: string;
  artist: string | null;
  artistId: string | null;
  coverUrl: string | null;
  durationMs: number | null;
  /** The last listen counted (unix seconds); null: never. */
  lastPlayed: number | null;
  /** The album as a quick mix's ingredient. */
  ingredient: Ingredient;
}

/** One mood: a genre and three of its albums. */
export interface Mood {
  genre: string;
  /** The genre's albums in the library, when known. */
  albumCount: number | null;
  albums: MoodAlbum[];
}

/** The day's moods: today's three and the other three. */
export interface Moods {
  today: Mood[];
  other: Mood[];
}

// ----- Playlist edits (src-tauri/src/playlists/dto.rs) -----

export type EditStatus = "done" | "partial" | "stale" | "failed" | "refused";

/** A playlist edit's outcome; `message` is also shown as a notice. */
export interface EditOutcome {
  status: EditStatus;
  message: string;
  /** The new playlist's id, for a create. */
  created: string | null;
}

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

// ----- Preferences (src-tauri/src/prefs.rs) -----

/** A curated accent. */
export interface Swatch {
  name: string;
  /** What choosing it saves: `"house"` or `#rrggbb`. */
  value: string;
  /** `#rrggbb`, to draw it. */
  color: string;
}

/** The GUI's own preferences (`~/.config/coda-app/settings.toml`). */
export interface Prefs {
  /** `"music"` (from the music, the default), `"house"` or `#rrggbb`, as saved. */
  accent: string;
  /** The accent's colour, `#rrggbb` (the house accent's for `"music"`). */
  accentColor: string;
  /** Black or white, whichever reads on the accent. */
  accentText: string;
  /** WCAG contrast of the accent with the base. */
  accentContrast: number;
  swatches: Swatch[];
  source: SourceKind | null;
  lyricsIndex: boolean;
  /** Library's default view of albums and artists. */
  libraryView: LibraryView;
}

/** How Library shows albums and artists. */
export type LibraryView = "covers" | "list";

/** A cover's colours (`cover_palettes`; backend `cover::palettes::PaletteDto`). */
export interface Palette {
  /** A deep dark of the dominant hue, `#rrggbb`. */
  tint: string;
  /** The most vivid colour, readable on the base and the tint, `#rrggbb`. */
  accent: string;
  /** Black or white on the accent, `#rrggbb`. */
  accentText: string;
  /** The accent, translucent, `#rrggbbaa`. */
  glow: string;
  /** `false`: a greyscale cover (the house accent is used). */
  vivid: boolean;
}
