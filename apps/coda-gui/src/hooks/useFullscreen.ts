// Full screen for Now Playing. `enter` (from `f` or the bar's Full screen
// button, through App) takes the window full screen unless it already is;
// `toggle` is Now Playing's own button. Leaving Now Playing leaves full
// screen again, but only one we entered: a full screen the user chose
// before stays, and once they leave ours through the OS we forget it
// (never re-entered). The state comes from the backend's
// `window_is_fullscreen` (the real one, also a window manager's on Linux),
// read on every resize and on the backend's `window-fullscreen` event
// (Linux); it drives the view's button and the `is-fullscreen` class on
// <html> (app.css drops the room kept for the macOS window buttons then). Window API
// errors are ignored.

import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onWindowFullscreen, windowIsFullscreen } from "../lib/ipc";

/** Whether we put the window in full screen: asked, then seen. */
type Entered = "no" | "requested" | "confirmed";

// Requests run one at a time, in order, so leaving can never overtake an
// entry still in flight.
let requests: Promise<void> = Promise.resolve();

export function useFullscreen(nowPlayingOpen: boolean) {
  const [fullscreen, setFullscreen] = useState(false);
  const entered = useRef<Entered>("no");
  /** An `enter` is pending and Now Playing was not closed since. */
  const wanted = useRef(false);
  /** Now Playing is shown (for requests deciding at their turn). */
  const open = useRef(nowPlayingOpen);

  const seen = useCallback((now: boolean) => {
    setFullscreen(now);
    // A request counts once seen through, so a resize mid-transition
    // cannot clear it; a later "not full screen" means the user left.
    if (now && entered.current === "requested") entered.current = "confirmed";
    else if (!now && entered.current === "confirmed") entered.current = "no";
  }, []);

  const refresh = useCallback(async () => {
    try {
      seen(await windowIsFullscreen());
    } catch {
      // Unknown; keep the last state.
    }
  }, [seen]);

  /** Queues a full-screen change; `check` decides at its turn whether it is still wanted. */
  const request = useCallback(
    (on: boolean, check: () => Promise<boolean> = async () => true) => {
      const next = requests.then(async () => {
        if (!(await check())) return;
        entered.current = on ? "requested" : "no";
        try {
          await getCurrentWindow().setFullscreen(on);
        } catch {
          // Not possible here; nothing to undo.
        }
        await refresh();
      });
      requests = next.catch(() => {});
      return requests;
    },
    [refresh],
  );

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [
      getCurrentWindow().onResized(() => void refresh()),
      onWindowFullscreen(seen),
    ];
    void refresh();
    return () => {
      for (const u of unlisteners) u.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [refresh, seen]);

  useEffect(() => {
    document.documentElement.classList.toggle("is-fullscreen", fullscreen);
  }, [fullscreen]);

  useEffect(() => {
    open.current = nowPlayingOpen;
    if (nowPlayingOpen) return;
    wanted.current = false;
    // Decided at its turn: a toggle queued just before may enter first.
    void request(false, async () => entered.current !== "no");
  }, [nowPlayingOpen, request]);

  const enter = useCallback(() => {
    wanted.current = true;
    return request(true, async () => {
      // Closed again meanwhile, or full screen already (the user's): stay.
      if (!wanted.current) return false;
      try {
        const already = await windowIsFullscreen();
        return wanted.current && !already;
      } catch {
        return false;
      }
    });
  }, [request]);

  // Entering only while Now Playing is still shown at its turn.
  const toggle = useCallback(() => {
    const on = !fullscreen;
    void request(on, async () => !on || open.current);
  }, [request, fullscreen]);

  return { fullscreen, enter, toggle };
}
