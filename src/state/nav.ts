// Navigation: a back stack of routes (DESIGN.md: Artists → Artist →
// Album). The sidebar resets the stack to one of its roots; views push.
// Each entry remembers its scroll position, so Back returns to the same
// place.

import { useSyncExternalStore } from "react";
import type { Album, AlbumSort, LocalAlbumOrder, Playlist, SourceKind } from "../lib/types";
import type { ViewId } from "../views/viewList";

export type Route =
  | { kind: "search"; query: string }
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

let stack: Entry[] = [entry({ kind: "artists", source: "local" })];
const listeners = new Set<() => void>();

function set(next: Entry[]) {
  stack = next;
  listeners.forEach((l) => l());
}

export const nav = {
  push(route: Route) {
    set([...stack, entry(route)]);
  },
  /** Replaces the top route (e.g. a new sort), keeping the stack. */
  replace(route: Route) {
    set([...stack.slice(0, -1), entry(route)]);
  },
  /** Back; the root stays. Returns `false` at the root. */
  back(): boolean {
    if (stack.length <= 1) return false;
    set(stack.slice(0, -1));
    return true;
  },
  /** A sidebar destination: a new stack with `route` at its root. */
  root(route: Route) {
    set([entry(route)]);
  },
  /** Opens search, or focuses it if it is already the top view. */
  search() {
    const top = stack[stack.length - 1].route;
    if (top.kind === "search") {
      document.querySelector<HTMLInputElement>(".search-input")?.focus();
      return;
    }
    nav.push({ kind: "search", query: "" });
  },
  top(): Entry {
    return stack[stack.length - 1];
  },
};

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useNav(): Entry[] {
  return useSyncExternalStore(subscribe, () => stack);
}

/** The sidebar's routes. */
export function routeFor(view: ViewId): Route {
  switch (view) {
    case "search":
      return { kind: "search", query: "" };
    case "local-artists":
      return { kind: "artists", source: "local" };
    case "local-albums":
      return { kind: "albums", source: "local", order: "name" };
    case "local-folders":
      return { kind: "folder", path: [] };
    case "local-playlists":
      return { kind: "playlists", source: "local" };
    case "navidrome-artists":
      return { kind: "artists", source: "navidrome" };
    case "navidrome-albums":
      return { kind: "albums", source: "navidrome", sort: "name" };
    case "navidrome-playlists":
      return { kind: "playlists", source: "navidrome" };
    case "settings":
      return { kind: "settings" };
  }
}

/** The sidebar entry a route belongs to (the stack's root decides). */
export function viewFor(route: Route): ViewId | null {
  switch (route.kind) {
    case "search":
      return "search";
    case "settings":
      return "settings";
    case "folder":
      return "local-folders";
    case "artists":
    case "artist":
      return route.source === "local" ? "local-artists" : "navidrome-artists";
    case "albums":
    case "album":
      return route.source === "local" ? "local-albums" : "navidrome-albums";
    case "playlists":
    case "playlist":
      return route.source === "local" ? "local-playlists" : "navidrome-playlists";
  }
}
