// The only place that talks to the backend: typed intents, the startup
// snapshot and event subscriptions.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Appearance,
  AppearancePrefs,
  ColorEdits,
  Intent,
  Progress,
  ResolvedTheme,
  ServerInput,
  Settings,
  Snapshot,
  SourceKind,
  ThemeSummary,
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

export type CoverSize = "small" | "large";

/** The image URL for a track's cover at `size`. */
export function coverSrc(coverUrl: string, size: CoverSize): string {
  return `${coverUrl}/${size}`;
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

// ----- appearance (each rejects with a message to show) -----

export const themeApi = {
  get: () => invoke<Appearance>("appearance_get"),
  set: (prefs: AppearancePrefs) => invoke<Appearance>("appearance_set", { prefs }),
  list: () => invoke<ThemeSummary[]>("theme_list"),
  resolve: (name: string, edits?: ColorEdits) => invoke<ResolvedTheme>("theme_resolve", { name, edits }),
  saveCustom: (base: string, name: string, edits: ColorEdits, replacing: string | null) =>
    invoke<Appearance>("theme_save_custom", { base, name, edits, replacing }),
  rename: (from: string, to: string) => invoke<Appearance>("theme_rename", { from, to }),
  duplicate: (name: string, to: string) => invoke<Appearance>("theme_duplicate", { name, to }),
  remove: (name: string) => invoke<Appearance>("theme_delete", { name }),
  /** Opens the custom themes folder in the file manager. */
  revealFolder: () => invoke<void>("theme_reveal_folder"),
  /** The native window's background behind the webview (`#rrggbb`). */
  windowBackground: (color: string) => invoke<void>("theme_window_background", { color }),
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
