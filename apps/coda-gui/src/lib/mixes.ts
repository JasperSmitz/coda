// Typed wrappers for the mixes commands (src-tauri/src/mixes.rs): the
// saved mixes file shared with the TUI, and the mix editor's searches.
// Playing a mix goes through `intent()` (`playMix`, `playQuickMix`). Below:
// the flows the views share (Add to mix…, delete with a confirmation) and
// a version that changes after every edit, so the Mixes view reloads.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, editMix, pickMix } from "../state/dialogs";
import { showUiNotice } from "../state/uiNotices";
import { toBrowseError } from "./browse";
import { intent } from "./ipc";
import { ingredientLabel } from "./mixRecipe";
import type { Ingredient, Mix, MixError, MixInput, SourceKind } from "./types";

/** Normalizes whatever a mixes command rejected with into a `MixError`. */
export function toMixError(error: unknown): MixError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as MixError;
  }
  return { kind: "failed", message: typeof error === "string" ? error : "Something went wrong" };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toMixError(error);
  }
}

async function search(command: string, source: SourceKind, query: string): Promise<Ingredient[]> {
  try {
    return await invoke<Ingredient[]>(command, { source, query });
  } catch (error) {
    throw toBrowseError(error);
  }
}

export const mixesApi = {
  /** The saved mixes of `source`, in file order. Rejects with a `MixError`. */
  list: (source: SourceKind) => call<Mix[]>("list_mixes", { source }),
  add: (mix: MixInput) => call<Mix>("add_mix", { mix }),
  /** Rejects with `otherSource` if the saved mix is not of `mix.source` now (another server or account). */
  update: (id: string, mix: MixInput) => call<Mix>("update_mix", { id, mix }),
  /**
   * Adds one ingredient to the end of saved mix `id` as it is in the file
   * now (checked under the file's lock, so another app's edits are kept).
   * Rejects with `alreadyIn`, `mixFull`, `notFound` or `otherSource`.
   */
  addTo: (id: string, source: SourceKind, ingredient: Ingredient) =>
    call<Mix>("add_to_mix", { id, source, ingredient }),
  /** Rejects with `otherSource` if the mix is not of `source` now (another server or account). */
  remove: (id: string, source: SourceKind) => call<void>("delete_mix", { id, source }),
  /** Genres whose name contains `query` (all for an empty one). Rejects with a `BrowseError`. */
  searchGenres: (source: SourceKind, query: string) =>
    search("mix_search_genres", source, query),
  searchArtists: (source: SourceKind, query: string) =>
    search("mix_search_artists", source, query),
  searchAlbums: (source: SourceKind, query: string) => search("mix_search_albums", source, query),
};

// ----- reload signal and flows ----------------------------------------------

let version = 0;
const listeners = new Set<() => void>();

/** Changes after every add, update or delete here (a `useLoad` refresh value). */
export function useMixesVersion(): number {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => version,
  );
}

function changed() {
  version += 1;
  listeners.forEach((l) => l());
}

/** Saves `mix` (`id`: an update), announcing the change. Rejects with a `MixError`. */
export async function saveMix(id: string | null, mix: MixInput): Promise<Mix> {
  const saved = id === null ? await mixesApi.add(mix) : await mixesApi.update(id, mix);
  changed();
  return saved;
}

/**
 * Adds `ingredient` to the end of saved mix `id` as it is in the file now,
 * announcing the change. Rejects with a `MixError` (`alreadyIn`,
 * `mixFull`, `notFound`, `otherSource`).
 */
export async function addToSavedMix(
  id: string,
  source: SourceKind,
  ingredient: Ingredient,
): Promise<Mix> {
  const saved = await mixesApi.addTo(id, source, ingredient);
  changed();
  return saved;
}

/** Plays a saved mix (read again from the file by the engine). */
export function playSavedMix(mix: Pick<Mix, "id">) {
  intent({ type: "playMix", id: mix.id });
}

/** Opens the editor for a new mix of `source`, prefilled with `ingredients`. */
export function newMixFlow(source: SourceKind, ingredients: Ingredient[] = []) {
  void editMix({ source, ingredients });
}

export function editMixFlow(mix: Mix) {
  void editMix({ source: mix.source, mix });
}

/** Asks, then deletes `mix`; the outcome is a notice. */
export async function deleteMixFlow(mix: Mix) {
  const ok = await confirm({
    title: "Delete mix?",
    message: `"${mix.name}" will be deleted. Its music stays where it is.`,
    confirmLabel: "Delete",
    danger: true,
  });
  if (!ok) return;
  try {
    await mixesApi.remove(mix.id, mix.source);
    changed();
    showUiNotice(`Deleted mix ${mix.name}`);
  } catch (error) {
    showUiNotice(`Could not delete the mix: ${toMixError(error).message}`);
  }
}

/**
 * "Add to mix…": add the ingredient to one of `source`'s saved mixes or
 * start a new one with it in the editor. The picker adds it itself
 * (`addToSavedMix`, against the mix as saved now) and says there when the
 * mix already holds it or is full; it closes with the mix once added.
 */
export async function addToMixFlow(source: SourceKind, ingredient: Ingredient) {
  const choice = await pickMix(source, ingredient);
  if (choice === null) return;
  if (choice === "new") {
    newMixFlow(source, [ingredient]);
    return;
  }
  showUiNotice(`Added ${ingredientLabel(ingredient)} to ${choice.name}`);
}
