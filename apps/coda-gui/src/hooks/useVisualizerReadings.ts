// Visualizer readings from the engine flow only while some visualizer is
// drawn (the player's small one, Now Playing's): each asks for them while
// it is shown, the page is visible and the window is not minimised, and
// the engine is told whether anyone still wants them.

import { useEffect, useState } from "react";
import { intentInOrder, onWindowMinimized } from "../lib/ipc";

/** The visualizers that want readings now. */
const wanting = new Set<symbol>();

// Requests go out one at a time, each carrying the latest wish, so a quick
// off/on (StrictMode, fast `f`, Now Playing replacing the player) can
// never reach the engine reordered.
let chain: Promise<void> = Promise.resolve();
let sent: boolean | null = null;

function sync() {
  chain = chain.then(async () => {
    const wanted = wanting.size > 0;
    if (wanted === sent) return;
    sent = wanted;
    await intentInOrder({ type: "setVisualizerVisible", visible: wanted });
  });
}

/**
 * Asks for visualizer readings while `wanted`, the page is visible and the
 * window is not minimised.
 */
export function useVisualizerReadings(wanted: boolean) {
  useEffect(() => {
    if (!wanted) return;
    const me = Symbol("visualizer");
    let minimized = false;
    let disposed = false;
    const update = () => {
      if (disposed) return;
      if (document.visibilityState === "visible" && !minimized) wanting.add(me);
      else wanting.delete(me);
      sync();
    };
    update();
    document.addEventListener("visibilitychange", update);
    const unlisten = onWindowMinimized((m) => {
      minimized = m;
      update();
    });
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", update);
      unlisten.then((f) => f()).catch(() => {});
      wanting.delete(me);
      sync();
    };
  }, [wanted]);
}

/** Whether `query` (a media query) matches, following changes. */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia?.(query).matches ?? true);
  useEffect(() => {
    const list = window.matchMedia?.(query);
    if (!list) return;
    const onChange = () => setMatches(list.matches);
    onChange();
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, [query]);
  return matches;
}
