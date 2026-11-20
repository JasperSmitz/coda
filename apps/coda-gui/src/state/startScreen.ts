// The screen the app starts on (Preferences › Start on, 2.0 R4): Home,
// Search or Library, saved in settings.toml (`startScreen`). It applies at
// the next launch (the backend marks it on <html> before the first paint;
// state/nav.ts reads it), not at once: choosing it does not move you.
//
// Saved like the Library default (LESSONS: a controlled input whose save
// round-trips): a choice shows at once, saves go one at a time with the
// newest waiting choice next, and only the answer to the last choice
// replaces what is shown.

import { useSyncExternalStore } from "react";
import { errorText, prefsApi } from "../lib/ipc";
import type { StartScreen } from "../lib/types";
import { useAccent } from "./accent";

interface State {
  /** The saved screen, as the backend last answered (`null`: not yet). */
  saved: StartScreen | null;
  /** The choice shown now; ahead of `saved` while a save runs. */
  shown: StartScreen | null;
  error: string | null;
}

let state: State = { saved: null, shown: null, error: null };
let sending = false;
let waiting: StartScreen | null = null;
const listeners = new Set<() => void>();
let version = 0;

function notify() {
  version++;
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function sendNext() {
  if (sending || waiting === null) return;
  const value = waiting;
  waiting = null;
  sending = true;
  prefsApi
    .setStartScreen(value)
    .then((prefs) => {
      state =
        waiting === null
          ? { saved: prefs.startScreen, shown: prefs.startScreen, error: null }
          : { ...state, saved: prefs.startScreen };
      notify();
    })
    .catch((e: unknown) => {
      state =
        waiting === null
          ? { ...state, shown: state.saved, error: errorText(e) }
          : { ...state, error: errorText(e) };
      notify();
    })
    .finally(() => {
      sending = false;
      sendNext();
    });
}

/** Chooses the start screen: shown at once, then saved. */
export function chooseStartScreen(screen: StartScreen) {
  state = { ...state, shown: screen, error: null };
  waiting = screen;
  notify();
  sendNext();
}

/** The start screen as shown in Preferences (the loaded preferences until a choice is made here). */
export function useStartScreen(): { screen: StartScreen; error: string | null } {
  const { prefs } = useAccent();
  useSyncExternalStore(subscribe, () => version);
  return {
    screen: state.shown ?? state.saved ?? prefs?.startScreen ?? "home",
    error: state.error,
  };
}
