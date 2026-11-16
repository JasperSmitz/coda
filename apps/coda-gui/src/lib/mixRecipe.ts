// The mix editor's pure logic: ingredient labels and keys, the name a
// quick mix gets when none was typed, ingredients from browse items, and
// how the editor's three searches are merged into one list. No React and
// no IPC here, so `node --test scripts/` can test it.

import type { ActiveMix, Album, Artist, Genre, Ingredient, Mix, SourceKind } from "./types";

/** At most this many ingredients per mix (`coda_core::mix::MAX_INGREDIENTS`). */
export const MAX_INGREDIENTS = 50;

/** Results shown per kind in the editor's search list. */
export const RESULTS_PER_KIND = 6;

export type IngredientKind = Ingredient["kind"];

export const KIND_LABEL: Record<IngredientKind, string> = {
  genre: "Genre",
  artist: "Artist",
  album: "Album",
};

/** What a chip shows: the name (a Navidrome genre's stray spaces trimmed). */
export function ingredientLabel(ingredient: Ingredient): string {
  return ingredient.name.trim();
}

/** The line under or beside the label: an album's artist, else nothing. */
export function ingredientDetail(ingredient: Ingredient): string | null {
  return ingredient.kind === "album" ? ingredient.artist.trim() || null : null;
}

/** For screen readers and tooltips: "genre Prog Rock", "album Relayer by Yes". */
export function describeIngredient(ingredient: Ingredient): string {
  const detail = ingredientDetail(ingredient);
  const name = ingredientLabel(ingredient);
  return `${ingredient.kind} ${name}${detail ? ` by ${detail}` : ""}`;
}

/**
 * Identifies an ingredient in a recipe, to drop duplicates: a Navidrome
 * artist or album by its id when it has one, a Navidrome genre by its
 * exact value, anything local by its names ignoring case and spaces (how
 * the library groups them). Core's `Ingredient::same_as` is the same rule.
 */
export function ingredientKey(ingredient: Ingredient, source: SourceKind): string {
  const fold = (text: string) => (source === "local" ? text.trim().toLowerCase() : text);
  switch (ingredient.kind) {
    case "genre":
      return `genre:${fold(ingredient.name)}`;
    case "artist":
      return source === "navidrome" && ingredient.id
        ? `artist#${ingredient.id}`
        : `artist:${fold(ingredient.name)}`;
    case "album":
      return source === "navidrome" && ingredient.id
        ? `album#${ingredient.id}`
        : `album:${fold(ingredient.artist)}\u{1f}${fold(ingredient.name)}`;
  }
}

/** Whether `recipe` already holds `ingredient`. */
export function hasIngredient(
  recipe: readonly Ingredient[],
  ingredient: Ingredient,
  source: SourceKind,
): boolean {
  const key = ingredientKey(ingredient, source);
  return recipe.some((i) => ingredientKey(i, source) === key);
}

/**
 * Whether two recipes are exactly the same, in order (the editor's
 * "unchanged": Play then plays the saved mix).
 */
export function sameRecipe(a: readonly Ingredient[], b: readonly Ingredient[]): boolean {
  const exact = (i: Ingredient) =>
    i.kind === "genre"
      ? `genre\u{1f}${i.name}`
      : i.kind === "artist"
        ? `artist\u{1f}${i.name}\u{1f}${i.id ?? ""}`
        : `album\u{1f}${i.artist}\u{1f}${i.name}\u{1f}${i.id ?? ""}`;
  return a.length === b.length && a.every((i, n) => exact(i) === exact(b[n]));
}

/**
 * What the editor's opening re-read of saved mix `id` (the source's
 * `list_mixes`) does: nothing once a save is in flight (what is shown is
 * what is being saved), "gone" if the mix is no longer in the file, else
 * the fresh mix, also shown unless you already started changing it.
 */
export type Reread =
  | { kind: "ignore" }
  | { kind: "gone" }
  | { kind: "fresh"; mix: Mix; show: boolean };

export function rereadOutcome(
  id: string,
  mixes: readonly Mix[],
  editor: { saving: boolean; touched: boolean },
): Reread {
  if (editor.saving) return { kind: "ignore" };
  const mix = mixes.find((m) => m.id === id);
  if (!mix) return { kind: "gone" };
  return { kind: "fresh", mix, show: !editor.touched };
}

/** `recipe` with `ingredient` added at the end, unless it is there already. */
export function withIngredient(
  recipe: readonly Ingredient[],
  ingredient: Ingredient,
  source: SourceKind,
): Ingredient[] {
  return hasIngredient(recipe, ingredient, source) ? [...recipe] : [...recipe, ingredient];
}

/**
 * A name for a recipe nobody named (a quick mix): its first labels,
 * "Prog Rock + Yes + 2 more".
 */
export function defaultMixName(recipe: readonly Ingredient[]): string {
  if (recipe.length === 0) return "Quick mix";
  const shown = recipe.length <= 3 ? recipe : recipe.slice(0, 2);
  const names = shown.map(ingredientLabel);
  if (recipe.length > shown.length) names.push(`${recipe.length - shown.length} more`);
  return names.join(" + ");
}

/** "Mix: Prog" or "Quick mix: Prog + Yes". */
export function activeMixLabel(mix: ActiveMix): string {
  return `${mix.id === null ? "Quick mix" : "Mix"}: ${mix.name}`;
}

// ----- ingredients from browse items ----------------------------------------

/**
 * A genre row or page as an ingredient. Navidrome matches a genre by its
 * exact server value, which is the genre's id there; locally the name.
 */
export function genreIngredient(
  source: SourceKind,
  genre: Pick<Genre, "id" | "name">,
): Ingredient {
  return { kind: "genre", name: source === "navidrome" ? genre.id : genre.name };
}

/** An artist as an ingredient: local by name (ids change with a rescan), Navidrome with its id. */
export function artistIngredient(
  source: SourceKind,
  artist: Pick<Artist, "id" | "name">,
): Ingredient {
  return { kind: "artist", name: artist.name, id: source === "navidrome" ? artist.id : null };
}

/**
 * An album as an ingredient, with its album artist exactly as listed
 * (Navidrome finds it again by id, else by these names). `null` without
 * an artist: such an album cannot be found again by name.
 */
export function albumIngredient(
  source: SourceKind,
  album: Pick<Album, "id" | "name" | "artist">,
): Ingredient | null {
  if (album.artist === null || album.artist.trim() === "") return null;
  return {
    kind: "album",
    artist: album.artist,
    name: album.name,
    id: source === "navidrome" ? album.id : null,
  };
}

// ----- the editor's search list ---------------------------------------------

export interface SearchGroups {
  genres: readonly Ingredient[];
  artists: readonly Ingredient[];
  albums: readonly Ingredient[];
}

/**
 * The editor's results as one list: genres, then artists, then albums;
 * within each kind, names that start with the query before the others
 * (otherwise in the order found), at most `perKind` each, leaving out
 * what the recipe already holds and duplicates.
 */
export function mergeSearchResults(
  groups: SearchGroups,
  query: string,
  recipe: readonly Ingredient[],
  source: SourceKind,
  perKind = RESULTS_PER_KIND,
): Ingredient[] {
  const needle = query.trim().toLowerCase();
  const taken = new Set(recipe.map((i) => ingredientKey(i, source)));
  const pick = (list: readonly Ingredient[]): Ingredient[] => {
    const fresh: Ingredient[] = [];
    for (const item of list) {
      const key = ingredientKey(item, source);
      if (taken.has(key)) continue;
      taken.add(key);
      fresh.push(item);
    }
    const starts = (i: Ingredient) =>
      needle !== "" && ingredientLabel(i).toLowerCase().startsWith(needle);
    // A stable sort: prefix matches first, the found order kept within.
    return [...fresh.filter(starts), ...fresh.filter((i) => !starts(i))].slice(0, perKind);
  };
  return [...pick(groups.genres), ...pick(groups.artists), ...pick(groups.albums)];
}
