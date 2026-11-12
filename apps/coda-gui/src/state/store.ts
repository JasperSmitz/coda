// A tiny store fed by backend events. Two parts with separate
// subscriptions: the snapshot (changes rarely) and progress (up to 10/s),
// so the position does not re-render the queue.

import { useSyncExternalStore } from "react";
import { fetchSnapshot, onProgress, onSnapshot } from "../lib/ipc";
import { playingEntryChanged } from "../lib/progress";
import type { Progress, QueueItem, Snapshot } from "../lib/types";

export interface AppState {
  snapshot: Snapshot | null;
  /** The last queue received (events omit an unchanged queue). */
  queue: QueueItem[];
}

type Listener = () => void;

function createStore<T>(initial: T) {
  let value = initial;
  const listeners = new Set<Listener>();
  return {
    get: () => value,
    set(next: T) {
      value = next;
      listeners.forEach((l) => l());
    },
    subscribe(listener: Listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

const app = createStore<AppState>({ snapshot: null, queue: [] });
const progress = createStore<Progress>({ positionMs: null, durationMs: null });

function applySnapshot(snapshot: Snapshot) {
  const current = app.get();
  // Another entry is heard: the last position was the previous track's.
  // Cleared first, so nothing sees the new track with the old position
  // (the engine's next progress report can be up to 100 ms away, and has
  // no position until the new load starts).
  if (playingEntryChanged(current.snapshot, snapshot)) {
    progress.set({ positionMs: null, durationMs: null });
  }
  app.set({ snapshot, queue: snapshot.queue ?? current.queue });
}

/**
 * Subscribes to the backend's events, then loads the full snapshot. The
 * snapshot only applies if no event arrived first (events are newer; the
 * backend resends the queue in the next event after a `snapshot` call).
 * Returns a cleanup function.
 */
export function connect(): () => void {
  let disposed = false;
  let sawEvent = false;
  const unlisten = [
    onSnapshot((s) => {
      sawEvent = true;
      applySnapshot(s);
    }),
    onProgress((p) => progress.set(p)),
  ];
  Promise.all(unlisten)
    .then(() => fetchSnapshot())
    .then((s) => {
      if (!disposed && !sawEvent) applySnapshot(s);
    })
    .catch((error: unknown) => console.warn("could not load the snapshot", error));
  return () => {
    disposed = true;
    unlisten.forEach((p) => p.then((f) => f()).catch(() => {}));
  };
}

export function useAppState(): AppState {
  return useSyncExternalStore(app.subscribe, app.get);
}

export function useProgress(): Progress {
  return useSyncExternalStore(progress.subscribe, progress.get);
}
