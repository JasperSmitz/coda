// Typed wrappers for the mixes commands (src-tauri/src/mixes.rs): the
// saved mixes file shared with the TUI, and the mix editor's searches.
// Playing a mix goes through `intent()` (`playMix`, `playQuickMix`).

import { invoke } from "@tauri-apps/api/core";
import { toBrowseError } from "./browse";
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
  /** Rejects with `otherSource` if the mix is not of `source` now (another server or account). */
  remove: (id: string, source: SourceKind) => call<void>("delete_mix", { id, source }),
  /** Genres whose name contains `query` (all for an empty one). Rejects with a `BrowseError`. */
  searchGenres: (source: SourceKind, query: string) =>
    search("mix_search_genres", source, query),
  searchArtists: (source: SourceKind, query: string) =>
    search("mix_search_artists", source, query),
  searchAlbums: (source: SourceKind, query: string) => search("mix_search_albums", source, query),
};
