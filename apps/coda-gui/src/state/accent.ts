// The accent (Preferences). The backend's initialization script already
// set a chosen accent on <html> before the first paint; this store loads
// the preferences, applies changes live and saves them.
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
import { errorText, prefsApi } from "../lib/ipc";
import type { Prefs } from "../lib/types";

interface AccentState {
  /** The saved preferences, as the backend last answered. */
  prefs: Prefs | null;
  /** The choice shown now (`"house"` or `#rrggbb`); ahead of `prefs` while a save runs. */
  shown: string | null;
  error: string | null;
}

let state: AccentState = { prefs: null, shown: null, error: null };
const listeners = new Set<() => void>();

function setTokens(accent: string | null, text: string | null) {
  const root = document.documentElement.style;
  if (accent === null) root.removeProperty("--accent");
  else root.setProperty("--accent", accent);
  if (text === null) root.removeProperty("--accent-text");
  else root.setProperty("--accent-text", text);
  window.dispatchEvent(new Event(ACCENT_EVENT));
}

/** The confirmed preferences on <html>; the house accent is tokens.css's own. */
function applyPrefs(prefs: Prefs) {
  if (prefs.accent === HOUSE) setTokens(null, null);
  else setTokens(prefs.accentColor, prefs.accentText);
}

/** A choice before the backend has answered. */
function applyChoice(value: string) {
  if (value === HOUSE) setTokens(null, null);
  else setTokens(value, prefersDarkInk(value) ? "var(--ink-dark)" : "var(--ink-light)");
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
  /** Chooses `"house"` or `#rrggbb`: applied at once, then saved. */
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
