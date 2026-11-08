// Notices raised by the UI itself (a browse request behind a play action
// failed), shown with the engine's notices in `Toasts`. Expire after 5 s
// like the engine's.

import { useSyncExternalStore } from "react";
import type { Notice } from "../lib/types";

const TTL_MS = 5000;
const MAX = 3;

let notices: Notice[] = [];
let nextId = -1; // negative: never equal to an engine notice id
const listeners = new Set<() => void>();

function set(next: Notice[]) {
  notices = next;
  listeners.forEach((l) => l());
}

export function showUiNotice(text: string) {
  const id = nextId--;
  set([...notices.filter((n) => n.text !== text), { id, text }].slice(-MAX));
  window.setTimeout(() => dismissUiNotice(id), TTL_MS);
}

export function dismissUiNotice(id: number) {
  if (notices.some((n) => n.id === id)) set(notices.filter((n) => n.id !== id));
}

export function useUiNotices(): Notice[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => notices,
  );
}
