// Modal dialogs as promises: `confirm`, `promptText`, `pickPlaylist`,
// `pickMix` and `editMix` open one dialog (a newer one cancels an open one) and resolve with the
// answer. `DialogHost` (components/Dialog.tsx) draws the open one (the mix
// dialogs: `MixDialogHost`, components/MixDialogs.tsx).

import { useSyncExternalStore } from "react";
import type { Ingredient, Mix, Playlist, SourceKind } from "../lib/types";

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

/** The saved mix the ingredient was added to, `"new"` for "New mix with this…", or `null` (cancelled). */
export type MixChoice = Mix | "new" | null;

/** The mix editor: a new mix (optionally prefilled) or a saved one to edit. */
export interface MixEditorSpec {
  source: SourceKind;
  /** The saved mix being edited; absent for a new one. */
  mix?: Mix;
  /** A new mix's first ingredients ("New mix with this"). */
  ingredients?: Ingredient[];
}

/** The editor's result: the saved mix, `"played"` (a quick mix), or `null`. */
export type MixEditorResult = Mix | "played" | null;

export type DialogState =
  | { id: number; kind: "confirm"; spec: ConfirmSpec; resolve: (ok: boolean) => void }
  | { id: number; kind: "prompt"; spec: PromptSpec; resolve: (value: string | null) => void }
  | { id: number; kind: "pickPlaylist"; title: string; resolve: (choice: PlaylistChoice) => void }
  | {
      id: number;
      kind: "pickMix";
      source: SourceKind;
      ingredient: Ingredient;
      resolve: (choice: MixChoice) => void;
    }
  | {
      id: number;
      kind: "mixEditor";
      spec: MixEditorSpec;
      resolve: (result: MixEditorResult) => void;
    };

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

/** "Add to mix…": a saved mix of `source` to add `ingredient` to, or a new one. */
export function pickMix(source: SourceKind, ingredient: Ingredient): Promise<MixChoice> {
  cancelOpen();
  return new Promise((resolve) =>
    set({ id: nextId++, kind: "pickMix", source, ingredient, resolve }),
  );
}

/** Opens the mix editor; resolves when it closes. */
export function editMix(spec: MixEditorSpec): Promise<MixEditorResult> {
  cancelOpen();
  return new Promise((resolve) => set({ id: nextId++, kind: "mixEditor", spec, resolve }));
}

/** Closes mix dialog `id` with its answer (ignored if it is no longer open). */
export function answerMix(id: number, value: MixChoice | MixEditorResult) {
  const current = open;
  if (!current || current.id !== id) return;
  if (current.kind === "pickMix") {
    set(null);
    current.resolve(value === "played" ? null : value);
  } else if (current.kind === "mixEditor") {
    set(null);
    current.resolve(value === "new" ? null : value);
  }
}

/** Closes dialog `id` with its answer (ignored if it is no longer open). */
export function answer(id: number, value: boolean | string | PlaylistChoice) {
  const current = open;
  if (!current || current.id !== id) return;
  set(null);
  if (current.kind === "confirm") current.resolve(value === true);
  else if (current.kind === "prompt") current.resolve(typeof value === "string" ? value : null);
  else if (current.kind === "pickPlaylist")
    current.resolve(value === true || value === false ? null : (value as PlaylistChoice));
  else current.resolve(null);
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
