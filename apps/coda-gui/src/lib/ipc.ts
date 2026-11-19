// The only place that talks to the backend: typed intents, the startup
// snapshot and event subscriptions.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Intent,
  LibraryView,
  Lyrics,
  LyricsIndexStatus,
  Palette,
  Prefs,
  Progress,
  ServerInput,
  Settings,
  Snapshot,
  SourceKind,
  TrackRef,
  VisualizerFrame,
} from "./types";

/** Sends an intent to the engine. Failures (only while quitting) are logged. */
export function intent(value: Intent): void {
  invoke("intent", { intent: value }).catch((error: unknown) => {
    console.warn("intent failed", value.type, error);
  });
}

/** Like `intent`, but resolves once the engine has received it (for ordering). */
export function intentInOrder(value: Intent): Promise<void> {
  return invoke<void>("intent", { intent: value }).catch((error: unknown) => {
    console.warn("intent failed", value.type, error);
  });
}

/**
 * The backend's report that the main window was minimised (`true`) or
 * restored; `document.visibilityState` does not say so on every platform.
 */
export function onWindowMinimized(handler: (minimized: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("window-minimized", (event) => handler(event.payload));
}

/**
 * Whether the main window is full screen, however it got there (Tauri's
 * own `isFullscreen` misses a window manager's full screen on Linux).
 */
export function windowIsFullscreen(): Promise<boolean> {
  return invoke<boolean>("window_is_fullscreen");
}

/** Linux: the window manager put the window in or out of full screen. */
export function onWindowFullscreen(handler: (fullscreen: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("window-fullscreen", (event) => handler(event.payload));
}

/** The full current state, queue included. */
export function fetchSnapshot(): Promise<Snapshot> {
  return invoke<Snapshot>("snapshot");
}

export function onSnapshot(handler: (snapshot: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>("snapshot", (event) => handler(event.payload));
}

export function onProgress(handler: (progress: Progress) => void): Promise<UnlistenFn> {
  return listen<Progress>("progress", (event) => handler(event.payload));
}

export function onVisualizer(handler: (frame: VisualizerFrame) => void): Promise<UnlistenFn> {
  return listen<VisualizerFrame>("visualizer", (event) => handler(event.payload));
}

/**
 * The lyrics of the playing track: `null` when it has none; rejects with a
 * message when they could not be read or fetched.
 */
export function fetchLyrics(track: TrackRef): Promise<Lyrics | null> {
  return invoke<Lyrics | null>("now_playing_lyrics", { track });
}

export type CoverSize = "small" | "large";

/** The image URL for a track's cover at `size`. */
export function coverSrc(coverUrl: string, size: CoverSize): string {
  return `${coverUrl}/${size}`;
}

/** Cover URLs one `cover_palettes` call may carry (the backend's `MAX_BATCH`). */
export const PALETTE_BATCH = 64;

/**
 * The palettes of up to `PALETTE_BATCH` cover URLs (as the DTOs carry
 * them): `null` for an unknown URL or a cover without art; a URL whose
 * cover failed to load is missing from the answer (ask again later).
 */
export function fetchPalettes(urls: string[]): Promise<Record<string, Palette | null>> {
  return invoke<Record<string, Palette | null>>("cover_palettes", { urls });
}

// ----- the Navidrome lyrics index (each rejects with a message to show) -----

export const lyricsIndexApi = {
  status: () => invoke<LyricsIndexStatus>("lyrics_index_status"),
  rebuild: () => invoke<void>("lyrics_index_rebuild"),
  setEnabled: (enabled: boolean) =>
    invoke<LyricsIndexStatus>("lyrics_index_set_enabled", { enabled }),
};

export function onLyricsIndex(handler: (status: LyricsIndexStatus) => void): Promise<UnlistenFn> {
  return listen<LyricsIndexStatus>("lyrics-index", (event) => handler(event.payload));
}

// ----- settings and onboarding (each rejects with a message to show) -----

export const settingsApi = {
  get: () => invoke<Settings>("settings_get"),
  testServer: (server: ServerInput) => invoke<void>("settings_test_server", { server }),
  saveServer: (server: ServerInput) => invoke<void>("settings_save_server", { server }),
  disconnect: () => invoke<void>("settings_disconnect"),
  /** The native folder picker; `null` if cancelled. */
  pickFolder: () => invoke<string | null>("settings_pick_folder"),
  setMusicDir: (folder: string) => invoke<void>("settings_set_music_dir", { folder }),
  setOptions: (options: { coverArt?: boolean; scrobble?: boolean }) =>
    invoke<void>("settings_set_options", options),
  finishOnboarding: (musicDir: string | null, server: ServerInput | null) =>
    invoke<void>("onboarding_finish", { musicDir, server }),
};

// ----- Preferences (`settings.toml`; each rejects with a message to show) -----

export const prefsApi = {
  get: () => invoke<Prefs>("prefs_get"),
  /** `"music"`, `"house"` or `#rrggbb`; answers with the new state. */
  setAccent: (accent: string) => invoke<Prefs>("prefs_set_accent", { accent }),
  /** Library's default view; answers with the new state. */
  setLibraryView: (view: LibraryView) => invoke<Prefs>("prefs_set_library_view", { view }),
};

// ----- the browsed source (`settings.toml`) -----

export const sourceApi = {
  /** The saved choice, `null` if none was made yet. */
  get: () => invoke<SourceKind | null>("ui_source_get"),
  set: (source: SourceKind) => invoke<void>("ui_source_set", { source }),
};

/** A command's rejection as text. */
export function errorText(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : "Something went wrong";
}
