// Modal dialogs as promises: `confirm`, `promptText` and `pickPlaylist`
// open one dialog (a newer one cancels an open one) and resolve with the
// answer. `DialogHost` (components/Dialog.tsx) draws the open one.

import { useSyncExternalStore } from "react";
import type { Playlist } from "../lib/types";

export interface ConfirmSpec {
  title: string;
  message: string;
  confirmLabel: string;
  /** A destructive action (red button). */
  danger?: boolean;
}

export interface PromptSpec {
  title: string;
  label: string;
  initial: string;
  confirmLabel: string;
  /** Text under the field (e.g. what is left out). */
  note?: string;
}

/** A picked playlist, `"new"` for "New playlist…", or `null` (cancelled). */
export type PlaylistChoice = Playlist | "new" | null;

export type DialogState =
  | { id: number; kind: "confirm"; spec: ConfirmSpec; resolve: (ok: boolean) => void }
  | { id: number; kind: "prompt"; spec: PromptSpec; resolve: (value: string | null) => void }
  | { id: number; kind: "pickPlaylist"; title: string; resolve: (choice: PlaylistChoice) => void };

let open: DialogState | null = null;
let nextId = 1;
const listeners = new Set<() => void>();

function set(next: DialogState | null) {
  open = next;
  listeners.forEach((l) => l());
}

/** Cancels the open dialog, if any. */
function cancelOpen() {
  const current = open;
  if (!current) return;
  set(null);
  if (current.kind === "confirm") current.resolve(false);
  else current.resolve(null);
}

export function confirm(spec: ConfirmSpec): Promise<boolean> {
  cancelOpen();
  return new Promise((resolve) => set({ id: nextId++, kind: "confirm", spec, resolve }));
}

export function promptText(spec: PromptSpec): Promise<string | null> {
  cancelOpen();
  return new Promise((resolve) => set({ id: nextId++, kind: "prompt", spec, resolve }));
}

export function pickPlaylist(title: string): Promise<PlaylistChoice> {
  cancelOpen();
  return new Promise((resolve) => set({ id: nextId++, kind: "pickPlaylist", title, resolve }));
}

/** Closes dialog `id` with its answer (ignored if it is no longer open). */
export function answer(id: number, value: boolean | string | PlaylistChoice) {
  const current = open;
  if (!current || current.id !== id) return;
  set(null);
  if (current.kind === "confirm") current.resolve(value === true);
  else if (current.kind === "prompt") current.resolve(typeof value === "string" ? value : null);
  else current.resolve(value === true || value === false ? null : (value as PlaylistChoice));
}

export function useDialog(): DialogState | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => open,
  );
}
