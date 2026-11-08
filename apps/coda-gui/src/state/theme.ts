// The applied theme. The backend's initialization script already set the
// chosen theme's tokens before the first paint; this store keeps them
// current: the light or dark one as the system changes, Ghostty's theme
// again when the window regains focus (Ghostty mode), Settings changes,
// and previews (the theme list, the colour editor), reverted on cancel.
// The native window's background follows the applied background.

import { useSyncExternalStore } from "react";
import { errorText, themeApi } from "../lib/ipc";
import { applyTokens, opaquePart, tokenValue } from "../lib/theme";
import type { Appearance, ResolvedTheme } from "../lib/types";

interface ThemeState {
  appearance: Appearance | null;
  /** Shown instead of the applied theme while set. */
  preview: ResolvedTheme | null;
  error: string | null;
}

let state: ThemeState = { appearance: null, preview: null, error: null };
const listeners = new Set<() => void>();
const lightQuery = window.matchMedia?.("(prefers-color-scheme: light)");

function prefersLight(): boolean {
  return lightQuery?.matches ?? false;
}

/** The applied theme for the system's current appearance. */
export function activeTheme(appearance: Appearance): ResolvedTheme {
  return prefersLight() ? appearance.light : appearance.dark;
}

/** The native window background last sent (set at creation by the backend). */
let windowBackground = "";

function render() {
  const { appearance, preview } = state;
  const theme = preview ?? (appearance ? activeTheme(appearance) : null);
  if (!theme) return;
  applyTokens(document.documentElement, theme, appearance?.prefs.visualizer ?? "accent");
  // Behind the webview too (resizes, the next window), when it changed.
  const bg = opaquePart(tokenValue(theme, "bg"));
  if (bg && bg !== windowBackground) {
    windowBackground = bg;
    themeApi.windowBackground(bg).catch(() => {});
  }
}

function set(next: Partial<ThemeState>) {
  state = { ...state, ...next };
  render();
  listeners.forEach((l) => l());
}

let loading = 0;

/** Loads the preferences and applied themes again (newest answer wins). */
export function reloadTheme(): Promise<void> {
  const id = ++loading;
  return themeApi
    .get()
    .then((appearance) => {
      if (id === loading) set({ appearance, error: null });
    })
    .catch((e: unknown) => {
      if (id === loading) set({ error: errorText(e) });
    });
}

export const theme = {
  /** A command answered with the new state. */
  setAppearance(appearance: Appearance) {
    loading++;
    set({ appearance, error: null });
  },
  /** Shows `resolved` until `null` (the applied theme again). */
  setPreview(resolved: ResolvedTheme | null) {
    if (resolved !== state.preview) set({ preview: resolved });
  },
};

/** Starts following the system and Ghostty; returns the stop function. */
export function startTheme(): () => void {
  void reloadTheme();
  const onScheme = () => render();
  const onFocus = () => {
    if (state.appearance?.prefs.mode === "ghostty") void reloadTheme();
  };
  lightQuery?.addEventListener("change", onScheme);
  window.addEventListener("focus", onFocus);
  return () => {
    lightQuery?.removeEventListener("change", onScheme);
    window.removeEventListener("focus", onFocus);
  };
}

export function useTheme(): ThemeState {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
}

/** The scheme the system asks for now (re-rendered on change by callers). */
export function systemScheme(): "light" | "dark" {
  return prefersLight() ? "light" : "dark";
}
