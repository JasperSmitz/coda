// The navigation model (DESIGN.md "Navigation"), pure: no React, no DOM,
// no storage, so `node --test scripts/` can test it (`scripts/nav.test.mjs`).
// `state/nav.ts` holds the live state and wires it to React.
//
// Per source, three browse roots (Home, Search, Library), each with its
// own back stack, and the source's active root. Preferences is no
// source's: one flag says it is shown over them, and leaving it shows the
// source's browse root again. Library's root route is the chosen section
// (Albums by default; Folders only for the local library).
// Every entry remembers its scroll position, so Back returns to the same
// place; switching roots or sources shows a stack where it was left.

import type { Album, AlbumSort, Genre, LocalAlbumOrder, Playlist, SourceKind } from "./types";

export type Route =
  | { kind: "home"; source: SourceKind }
  | { kind: "search"; source: SourceKind; query: string }
  | { kind: "artists"; source: SourceKind }
  /** No `order` / `sort`: the one remembered for the source (`state/libraryView.ts`). */
  | { kind: "albums"; source: "local"; order?: LocalAlbumOrder }
  | { kind: "albums"; source: "navidrome"; sort?: AlbumSort }
  | { kind: "playlists"; source: SourceKind }
  | { kind: "mixes"; source: SourceKind }
  | { kind: "genres"; source: SourceKind }
  | { kind: "folder"; path: string[] }
  | { kind: "artist"; source: SourceKind; id: string; name: string }
  /** `hint`: the listing's data, shown while the album loads. */
  | { kind: "album"; source: SourceKind; id: string; name: string; hint?: Album }
  | { kind: "playlist"; source: SourceKind; id: string; name: string; hint?: Playlist }
  /** `hint`: the genre list's row (its counts). */
  | { kind: "genre"; source: SourceKind; id: string; name: string; hint?: Genre }
  | { kind: "settings" };

export interface Entry {
  /** Unique per push, a React key: the same route twice is two views. */
  id: number;
  route: Route;
  scrollTop: number;
}

/** The three browse roots (per source). */
export type BrowseRoot = "home" | "search" | "library";
/** The top bar's destinations: the browse roots and Preferences (source-independent). */
export type Root = BrowseRoot | "prefs";

/** Library's sections, in the order of its control. */
export const SECTIONS = ["albums", "artists", "genres", "playlists", "mixes", "folders"] as const;
export type Section = (typeof SECTIONS)[number];

export const SECTION_LABEL: Record<Section, string> = {
  albums: "Albums",
  artists: "Artists",
  genres: "Genres",
  playlists: "Playlists",
  mixes: "Mixes",
  folders: "Folders",
};

/** One source's navigation. */
export interface SourceNav {
  /** The browse root shown for this source (when Preferences is not). */
  root: BrowseRoot;
  /** Library's chosen section (as chosen; see `effectiveSection`). */
  section: Section;
  stacks: Record<BrowseRoot, Entry[]>;
}

export interface NavState {
  /** The source shown. */
  source: SourceKind;
  /** Preferences is shown (over whichever source and root). */
  prefs: boolean;
  /** Preferences' one-entry stack (one page for both sources). */
  prefsStack: Entry[];
  sources: Record<SourceKind, SourceNav>;
}

let nextId = 1;

export function entry(route: Route): Entry {
  return { id: nextId++, route, scrollTop: 0 };
}

export function isSection(value: unknown): value is Section {
  return typeof value === "string" && (SECTIONS as readonly string[]).includes(value);
}

/** The sections a source offers (Folders: the local library only). */
export function sectionsFor(source: SourceKind): Section[] {
  return SECTIONS.filter((s) => s !== "folders" || source === "local");
}

/** The section shown for a choice: Folders falls back to Albums on Navidrome. */
export function effectiveSection(section: Section, source: SourceKind): Section {
  return sectionsFor(source).includes(section) ? section : "albums";
}

/** A section's root route. */
export function sectionRoute(section: Section, source: SourceKind): Route {
  switch (effectiveSection(section, source)) {
    case "albums":
      return { kind: "albums", source };
    case "artists":
      return { kind: "artists", source };
    case "genres":
      return { kind: "genres", source };
    case "playlists":
      return { kind: "playlists", source };
    case "mixes":
      return { kind: "mixes", source };
    case "folders":
      return { kind: "folder", path: [] };
  }
}

const LOCAL_ORDERS: readonly LocalAlbumOrder[] = ["name", "artist", "year", "mostPlayed"];
const SERVER_SORTS: readonly AlbumSort[] = [
  "name",
  "artist",
  "newest",
  "frequent",
  "recent",
  "random",
];

/**
 * The local Albums order a route shows: its own, else the remembered one
 * (`remembered`, as stored; anything unknown means A–Z).
 */
export function localOrderOf(order: LocalAlbumOrder | undefined, remembered: string | null): LocalAlbumOrder {
  return order ?? LOCAL_ORDERS.find((o) => o === remembered) ?? "name";
}

/** The Navidrome Albums sort a route shows, like `localOrderOf`. */
export function serverSortOf(sort: AlbumSort | undefined, remembered: string | null): AlbumSort {
  return sort ?? SERVER_SORTS.find((s) => s === remembered) ?? "name";
}

/** The section a library root route shows (`null`: not a section's root). */
export function sectionOf(route: Route): Section | null {
  switch (route.kind) {
    case "albums":
    case "artists":
    case "genres":
    case "playlists":
    case "mixes":
      return route.kind;
    case "folder":
      return route.path.length === 0 ? "folders" : null;
    default:
      return null;
  }
}

/** A browse root's root route. */
export function rootRoute(root: BrowseRoot, source: SourceKind, section: Section): Route {
  switch (root) {
    case "home":
      return { kind: "home", source };
    case "search":
      return { kind: "search", source, query: "" };
    case "library":
      return sectionRoute(section, source);
  }
}

/**
 * The browse roots in the top bar's order (2.0 R4): Home in the middle,
 * so every root is one step or one swipe from it. Fixed: the start screen
 * does not reorder it.
 */
export const BROWSE_ROOTS: readonly BrowseRoot[] = ["search", "home", "library"];

/** Whether `value` names a browse root (a saved start screen, for one). */
export function isBrowseRoot(value: unknown): value is BrowseRoot {
  return BROWSE_ROOTS.includes(value as BrowseRoot);
}

/**
 * The browse root `step` places from `root` in the top bar's order
 * (`null`: past an end, or from Preferences).
 */
export function rootBeside(root: Root, step: -1 | 1): BrowseRoot | null {
  const at = BROWSE_ROOTS.indexOf(root as BrowseRoot);
  if (at < 0) return null;
  return BROWSE_ROOTS[at + step] ?? null;
}

function freshStacks(source: SourceKind, section: Section): Record<BrowseRoot, Entry[]> {
  const stacks = {} as Record<BrowseRoot, Entry[]>;
  for (const root of BROWSE_ROOTS) stacks[root] = [entry(rootRoute(root, source, section))];
  return stacks;
}

function freshSource(source: SourceKind, section: Section, start: BrowseRoot): SourceNav {
  return { root: start, section, stacks: freshStacks(source, section) };
}

/** Every stack at its root, `start` (the start screen; Home by default) shown, on `source`. */
export function initialNav(
  sections: Record<SourceKind, Section>,
  source: SourceKind = "local",
  start: BrowseRoot = "home",
): NavState {
  return {
    source,
    prefs: false,
    prefsStack: [entry({ kind: "settings" })],
    sources: {
      local: freshSource("local", sections.local, start),
      navidrome: freshSource("navidrome", sections.navidrome, start),
    },
  };
}

/** The active source's navigation. */
export function current(state: NavState): SourceNav {
  return state.sources[state.source];
}

/** The top bar's active entry. */
export function activeRoot(state: NavState): Root {
  return state.prefs ? "prefs" : current(state).root;
}

/** The stack shown: Preferences', or the active source's browse root's. */
export function currentStack(state: NavState): Entry[] {
  if (state.prefs) return state.prefsStack;
  const nav = current(state);
  return nav.stacks[nav.root];
}

export function topEntry(state: NavState): Entry {
  const stack = currentStack(state);
  return stack[stack.length - 1];
}

function withSource(state: NavState, next: SourceNav): NavState {
  return { ...state, sources: { ...state.sources, [state.source]: next } };
}

/** Shows `root`'s stack, set to `stack`, on the active source (leaving Preferences). */
function withStack(state: NavState, root: BrowseRoot, stack: Entry[]): NavState {
  const nav = current(state);
  return withSource(
    { ...state, prefs: false },
    { ...nav, root, stacks: { ...nav.stacks, [root]: stack } },
  );
}

/**
 * A top bar entry: shows that root's stack where it was left; the active
 * root's entry again pops its stack to the root (a tab bar). Leaving
 * Preferences shows the root as it was.
 */
export function selectRoot(state: NavState, root: Root): NavState {
  if (root === "prefs") return state.prefs ? state : { ...state, prefs: true };
  const nav = current(state);
  if (state.prefs || nav.root !== root) {
    return withSource({ ...state, prefs: false }, { ...nav, root });
  }
  const stack = nav.stacks[root];
  if (stack.length <= 1) return state;
  return withStack(state, root, stack.slice(0, 1));
}

/** The same page (an id-bearing route of the same kind, source and id). */
function samePage(a: Route, b: Route): boolean {
  return (
    "id" in a &&
    "id" in b &&
    a.kind === b.kind &&
    a.source === b.source &&
    a.id === b.id
  );
}

/** The browse root a push opens on: the active one; from Preferences, Library. */
export function openRootFor(state: NavState): BrowseRoot {
  return state.prefs ? "library" : current(state).root;
}

/** Pushes `route` onto `root`'s stack and shows it; already its top: just shown. */
function pushOnto(state: NavState, root: BrowseRoot, route: Route): NavState {
  const stack = current(state).stacks[root];
  const top = stack[stack.length - 1];
  if (samePage(top.route, route)) {
    if (!state.prefs && current(state).root === root) return state;
    return withStack(state, root, stack);
  }
  return withStack(state, root, [...stack, entry(route)]);
}

/** Opens `route` on the active root's stack; from Preferences, on Library's. */
export function push(state: NavState, route: Route): NavState {
  return pushOnto(state, openRootFor(state), route);
}

/**
 * "Go to album/artist", after any source switch: opens `route` on `root`
 * (the root it was asked from, `openRootFor` before the switch) of the
 * now active source.
 */
export function goTo(state: NavState, root: BrowseRoot, route: Route): NavState {
  return pushOnto(state, root, route);
}

/** Replaces the top route (e.g. a new sort), keeping the stack. */
export function replaceTop(state: NavState, route: Route): NavState {
  if (state.prefs) return { ...state, prefsStack: [entry(route)] };
  const nav = current(state);
  const stack = nav.stacks[nav.root];
  return withStack(state, nav.root, [...stack.slice(0, -1), entry(route)]);
}

/**
 * Back on the shown stack; `null` at a browse root's root. From
 * Preferences: the browse root it was opened over.
 */
export function back(state: NavState): NavState | null {
  if (state.prefs) return { ...state, prefs: false };
  const nav = current(state);
  const stack = nav.stacks[nav.root];
  if (stack.length <= 1) return null;
  return withStack(state, nav.root, stack.slice(0, -1));
}

/** The shown browse stack back at its root (`null`: already there, or Preferences). */
export function toRoot(state: NavState): NavState | null {
  if (state.prefs) return null;
  const nav = current(state);
  const stack = nav.stacks[nav.root];
  if (stack.length <= 1) return null;
  return withStack(state, nav.root, stack.slice(0, 1));
}

/**
 * Shows Search: its stack back at its root, with `query` as its query
 * (a fresh search page) or, without one, the query it had.
 */
export function search(state: NavState, query = ""): NavState {
  const nav = current(state);
  const stack = nav.stacks.search;
  const keep = query === "" ? stack[0] : entry({ kind: "search", source: state.source, query });
  if (!state.prefs && nav.root === "search" && stack.length === 1 && stack[0] === keep) {
    return state;
  }
  return withStack(state, "search", [keep]);
}

/** Library's section: its stack restarts at that section, shown. */
export function setSection(state: NavState, section: Section): NavState {
  const nav = current(state);
  const chosen = effectiveSection(section, state.source);
  const stack = [entry(sectionRoute(chosen, state.source))];
  return withSource(
    { ...state, prefs: false },
    { ...nav, root: "library", section: chosen, stacks: { ...nav.stacks, library: stack } },
  );
}

/**
 * Shows `source`'s browse roots where they were left; Preferences, being
 * no source's, stays shown or not as it was.
 */
export function setSource(state: NavState, source: SourceKind): NavState {
  if (source === state.source) return state;
  return { ...state, source };
}

/**
 * The library or server was replaced: every view of that source holds
 * stale ids, so all its stacks restart at their roots (which reload); the
 * active root stays.
 */
export function resetSource(state: NavState, source: SourceKind): NavState {
  const nav = state.sources[source];
  return {
    ...state,
    sources: { ...state.sources, [source]: { ...nav, stacks: freshStacks(source, nav.section) } },
  };
}

/** How the shown page changes from one state to the next (a page transition's kind of morph). */
export type PageChange = "push" | "back" | "other";

/**
 * `push`: the new top was pushed onto the old top (on the same stack);
 * `back`: the old top was popped, showing the entry below it; anything
 * else (a root, a section, a source, several steps): `other`.
 */
export function pageChange(before: NavState, after: NavState): PageChange {
  const was = currentStack(before);
  const is = currentStack(after);
  const from = was[was.length - 1];
  const to = is[is.length - 1];
  if (from === to) return "other";
  if (is.length >= 2 && is[is.length - 2] === from) return "push";
  if (was.length >= 2 && was[was.length - 2] === to) return "back";
  return "other";
}
