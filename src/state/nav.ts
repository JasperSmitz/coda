// Navigation: one back stack of routes per source (DESIGN.md: Home →
// Artists → Artist → Album). The sidebar resets the active source's stack
// to one of its roots; views push. Switching the source shows the other
// stack where it was left. Each entry remembers its scroll position, so
// Back returns to the same place.

import { useSyncExternalStore } from "react";
import type { Album, AlbumSort, LocalAlbumOrder, Playlist, SourceKind } from "../lib/types";
import type { ViewId } from "../views/viewList";

export type Route =
  | { kind: "home"; source: SourceKind }
  | { kind: "search"; source: SourceKind; query: string }
  | { kind: "artists"; source: SourceKind }
  | { kind: "albums"; source: "local"; order: LocalAlbumOrder }
  | { kind: "albums"; source: "navidrome"; sort: AlbumSort }
  | { kind: "playlists"; source: SourceKind }
  | { kind: "folder"; path: string[] }
  | { kind: "artist"; source: SourceKind; id: string; name: string }
  /** `hint`: the listing's data, shown while the album loads. */
  | { kind: "album"; source: SourceKind; id: string; name: string; hint?: Album }
  | { kind: "playlist"; source: SourceKind; id: string; name: string; hint?: Playlist }
  | { kind: "settings" };

export interface Entry {
  /** Unique per push, a React key: the same route twice is two views. */
  id: number;
  route: Route;
  scrollTop: number;
}

let nextId = 1;
const entry = (route: Route): Entry => ({ id: nextId++, route, scrollTop: 0 });

const home = (source: SourceKind): Entry => entry({ kind: "home", source });

let active: SourceKind = "local";
const stacks: Record<SourceKind, Entry[]> = {
  local: [home("local")],
  navidrome: [home("navidrome")],
};
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((l) => l());
}

function set(next: Entry[], source: SourceKind = active) {
  stacks[source] = next;
  if (source === active) notify();
}

const stack = () => stacks[active];

export const nav = {
  push(route: Route) {
    set([...stack(), entry(route)]);
  },
  /** Replaces the top route (e.g. a new sort), keeping the stack. */
  replace(route: Route) {
    set([...stack().slice(0, -1), entry(route)]);
  },
  /** Shows the top route again from scratch (reloads it). */
  reload() {
    nav.replace(nav.top().route);
  },
  /** Back; the root stays. Returns `false` at the root. */
  back(): boolean {
    if (stack().length <= 1) return false;
    set(stack().slice(0, -1));
    return true;
  },
  /** A sidebar destination: a new stack with `route` at its root. */
  root(route: Route) {
    set([entry(route)]);
  },
  /** Opens search, or focuses it if it is already the top view. */
  search(query = "") {
    const top = nav.top().route;
    if (top.kind === "search" && query === "") {
      document.querySelector<HTMLInputElement>(".search-input")?.focus();
      return;
    }
    nav.push({ kind: "search", source: active, query });
  },
  top(): Entry {
    const s = stack();
    return s[s.length - 1];
  },
  /** The source whose stack is shown. */
  source(): SourceKind {
    return active;
  },
  /**
   * Shows `source`'s stack (where it was left). Callers go through
   * `state/source.ts`, which also saves the choice. `keepSettings`: if
   * Settings was shown, it stays shown (a source disconnected there).
   */
  setSource(source: SourceKind, keepSettings = false) {
    if (source === active) return;
    const settings = nav.top().route.kind === "settings";
    active = source;
    if (keepSettings && settings && nav.top().route.kind !== "settings") {
      stacks[source] = [...stacks[source], entry({ kind: "settings" })];
    }
    notify();
  },
};

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The active source's stack. */
export function useNav(): Entry[] {
  return useSyncExternalStore(subscribe, stack);
}

/** The source a route reads (`null`: none). */
export function routeSource(route: Route): SourceKind | null {
  switch (route.kind) {
    case "folder":
      return "local";
    case "settings":
      return null;
    default:
      return route.source;
  }
}

/**
 * The library or server was replaced (settings): every view of that
 * source holds stale ids, so its stack restarts at Home, which reloads.
 */
export function sourceReplaced(source: SourceKind) {
  set([home(source)], source);
}

/** The sidebar's routes, for the active source. */
export function routeFor(view: ViewId, source: SourceKind): Route {
  switch (view) {
    case "home":
      return { kind: "home", source };
    case "search":
      return { kind: "search", source, query: "" };
    case "artists":
      return { kind: "artists", source };
    case "albums":
      return source === "local"
        ? { kind: "albums", source, order: "name" }
        : { kind: "albums", source, sort: "name" };
    case "folders":
      return { kind: "folder", path: [] };
    case "playlists":
      return { kind: "playlists", source };
    case "settings":
      return { kind: "settings" };
  }
}

/** The sidebar entry a route belongs to (the stack's root decides). */
export function viewFor(route: Route): ViewId {
  switch (route.kind) {
    case "home":
      return "home";
    case "search":
      return "search";
    case "settings":
      return "settings";
    case "folder":
      return "folders";
    case "artists":
    case "artist":
      return "artists";
    case "albums":
    case "album":
      return "albums";
    case "playlists":
    case "playlist":
      return "playlists";
  }
}
