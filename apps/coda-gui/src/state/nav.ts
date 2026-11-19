// Navigation (DESIGN.md "Navigation"): the live state of the pure model in
// `lib/navModel.ts`. Per source, one back stack per browse root (Home,
// Search, Library) and the active root; Preferences is shown over them,
// for both sources. The top bar switches roots, views push, Back pops.
// Switching the source shows the other source's stacks where they were
// left. Library's section is remembered per source in `localStorage`.

import { useSyncExternalStore } from "react";
import * as model from "../lib/navModel";
import type { BrowseRoot, Entry, NavState, Root, Route, Section } from "../lib/navModel";
import type { SourceKind } from "../lib/types";

export type { BrowseRoot, Entry, Root, Route, Section } from "../lib/navModel";

const sectionKey = (source: SourceKind) => `coda.librarySection.${source}`;

function loadSection(source: SourceKind): Section {
  try {
    const value = localStorage.getItem(sectionKey(source));
    return model.isSection(value) ? model.effectiveSection(value, source) : "albums";
  } catch {
    return "albums";
  }
}

function saveSection(source: SourceKind, section: Section) {
  try {
    localStorage.setItem(sectionKey(source), section);
  } catch {
    // Not saved; harmless.
  }
}

let state: NavState = model.initialNav({
  local: loadSection("local"),
  navidrome: loadSection("navidrome"),
});
const listeners = new Set<() => void>();

function set(next: NavState) {
  if (next === state) return;
  state = next;
  listeners.forEach((l) => l());
}

/** Focuses the search field once Search is shown (it may already be). */
function focusSearch() {
  requestAnimationFrame(() => document.querySelector<HTMLInputElement>(".search-input")?.focus());
}

export const nav = {
  /** Opens `route` on the active root's stack (from Preferences: Library's). */
  push(route: Route) {
    set(model.push(state, route));
  },
  /** Replaces the top route (e.g. a new sort), keeping the stack. */
  replace(route: Route) {
    set(model.replaceTop(state, route));
  },
  /** Shows the top route again from scratch (reloads it). */
  reload() {
    nav.replace(nav.top().route);
  },
  /** Back (from Preferences: its browse root); a root's root stays. Returns `false` there. */
  back(): boolean {
    const next = model.back(state);
    if (!next) return false;
    set(next);
    return true;
  },
  /** The shown browse stack back at its root. */
  toRoot() {
    const next = model.toRoot(state);
    if (next) set(next);
  },
  /** The browse root a push opens on (from Preferences: Library). */
  openRoot(): BrowseRoot {
    return model.openRootFor(state);
  },
  /** Go to album/artist: `route` on `root` of the (already switched) source. */
  goTo(route: Route, root: BrowseRoot) {
    set(model.goTo(state, root, route));
  },
  /** A top bar entry: that root, or the active one back to its root. */
  selectRoot(root: Root) {
    set(model.selectRoot(state, root));
  },
  /** Library's section (remembered for the source). */
  setSection(section: Section) {
    set(model.setSection(state, section));
    saveSection(state.source, model.current(state).section);
  },
  /** Shows Search and focuses its field; `query` starts a new search. */
  search(query = "") {
    set(model.search(state, query));
    focusSearch();
  },
  top(): Entry {
    return model.topEntry(state);
  },
  /** The source whose stacks are shown. */
  source(): SourceKind {
    return state.source;
  },
  /**
   * Shows `source`'s stacks (where they were left). Callers go through
   * `state/source.ts`, which also saves the choice.
   */
  setSource(source: SourceKind) {
    set(model.setSource(state, source));
  },
};

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const getState = () => state;
const getStack = () => model.currentStack(state);

/** The shown stack (the active source's active root's). */
export function useNav(): Entry[] {
  return useSyncExternalStore(subscribe, getStack);
}

/** The whole navigation state (sections, stacks). */
export function useNavState(): NavState {
  return useSyncExternalStore(subscribe, getState);
}

const getRoot = () => model.activeRoot(state);

/** The top bar's active entry. */
export function useActiveRoot(): Root {
  return useSyncExternalStore(subscribe, getRoot);
}

/**
 * The library or server was replaced (settings): every view of that
 * source holds stale ids, so its stacks restart at their roots, which reload.
 */
export function sourceReplaced(source: SourceKind) {
  set(model.resetSource(state, source));
}
