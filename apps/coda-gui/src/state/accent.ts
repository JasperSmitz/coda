// The accent (Preferences). The backend's initialization script already
// set a chosen accent on <html> before the first paint; this store loads
// the preferences, applies changes live and saves them. With "From the
// music" (the default) and the house accent, <html> keeps tokens.css's
// house accent; the colour scopes (state/palettes.ts) read `shown` to
// decide whether they take the covers' accents.
//
// A choice is applied at once (optimistically: `--accent`, and black or
// white text on it by luminance until the backend's answer gives the
// exact one), then saved. Saves go one at a time, the newest waiting
// choice next (a dragged colour picker sends many), so the file always
// ends with the last choice. Answers arrive in order; each updates the
// confirmed preferences, but only the answer to the last choice touches
// the page, so a drag never jumps back to an older colour.

import { useSyncExternalStore } from "react";
import { ACCENT_EVENT, HOUSE, prefersDarkInk } from "../lib/accent";
import { MUSIC } from "../lib/palette";
import { errorText, prefsApi } from "../lib/ipc";
import type { Prefs } from "../lib/types";

interface AccentState {
  /** The saved preferences, as the backend last answered. */
  prefs: Prefs | null;
  /** The choice shown now (`"music"`, `"house"` or `#rrggbb`); ahead of `prefs` while a save runs. */
  shown: string | null;
  error: string | null;
}

let state: AccentState = { prefs: null, shown: null, error: null };
const listeners = new Set<() => void>();

/** How long after the listener's last accent change scopes skip the accent blend. */
const LIVE_MS = 500;
let liveTimer: number | null = null;

/**
 * The listener is choosing a fixed colour: the colour scopes follow at
 * once instead of blending (`data-accent-live` on <html>, app.css), so a
 * dragged picker is followed live everywhere. Not for "From the music" or
 * the house accent: switching to those blends.
 */
function markLive() {
  const root = document.documentElement;
  root.dataset.accentLive = "";
  if (liveTimer !== null) window.clearTimeout(liveTimer);
  liveTimer = window.setTimeout(() => {
    liveTimer = null;
    delete root.dataset.accentLive;
  }, LIVE_MS);
}

/** `data-accent-mode` on <html> (the init script sets it first). */
function modeOf(value: string): string {
  return value === MUSIC ? "music" : value === HOUSE ? "house" : "custom";
}

/**
 * Whether the colour scopes take the covers' accents: "From the music"
 * shown, or, until the preferences have loaded, the mode the backend's
 * init script marked on <html> before the first paint.
 */
export function musicAccentOn(shown: string | null): boolean {
  if (shown !== null) return shown === MUSIC;
  return document.documentElement.dataset.accentMode === "music";
}

function setTokens(value: string, accent: string | null, text: string | null) {
  if (accent !== null) markLive();
  document.documentElement.dataset.accentMode = modeOf(value);
  const root = document.documentElement.style;
  if (accent === null) root.removeProperty("--accent");
  else root.setProperty("--accent", accent);
  if (text === null) root.removeProperty("--accent-text");
  else root.setProperty("--accent-text", text);
  window.dispatchEvent(new Event(ACCENT_EVENT));
}

/** No fixed accent on <html>: tokens.css's house accent (scopes may take the music's). */
function isUnfixed(value: string): boolean {
  return value === HOUSE || value === MUSIC;
}

/** The confirmed preferences on <html>; the house accent is tokens.css's own. */
function applyPrefs(prefs: Prefs) {
  if (isUnfixed(prefs.accent)) setTokens(prefs.accent, null, null);
  else setTokens(prefs.accent, prefs.accentColor, prefs.accentText);
}

/** A choice before the backend has answered. */
function applyChoice(value: string) {
  if (isUnfixed(value)) setTokens(value, null, null);
  else setTokens(value, value, prefersDarkInk(value) ? "var(--ink-dark)" : "var(--ink-light)");
}

function set(next: Partial<AccentState>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

let sending = false;
let waiting: string | null = null;

function sendNext() {
  if (sending || waiting === null) return;
  const value = waiting;
  waiting = null;
  sending = true;
  prefsApi
    .setAccent(value)
    .then((prefs) => {
      if (waiting === null) {
        // The last choice: the backend's exact colours.
        applyPrefs(prefs);
        set({ prefs, shown: prefs.accent, error: null });
      } else {
        set({ prefs });
      }
    })
    .catch((e: unknown) => {
      // Back to what is saved, unless a newer choice is on its way.
      if (waiting === null && state.prefs) {
        applyPrefs(state.prefs);
        set({ shown: state.prefs.accent, error: errorText(e) });
      } else {
        set({ error: errorText(e) });
      }
    })
    .finally(() => {
      sending = false;
      sendNext();
    });
}

export const accent = {
  /** Chooses `"music"`, `"house"` or `#rrggbb`: applied at once, then saved. */
  choose(value: string) {
    applyChoice(value);
    set({ shown: value, error: null });
    waiting = value;
    sendNext();
  },
};

/** Loads the preferences once (at startup). */
export function startAccent(): () => void {
  let live = true;
  prefsApi
    .get()
    .then((prefs) => {
      if (!live || state.prefs !== null) return;
      // A choice made before the answer stays shown.
      if (state.shown === null) {
        applyPrefs(prefs);
        set({ prefs, shown: prefs.accent, error: null });
      } else {
        set({ prefs });
      }
    })
    .catch((e: unknown) => {
      if (live) set({ error: errorText(e) });
    });
  return () => {
    live = false;
  };
}

export function useAccent(): AccentState {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
}
