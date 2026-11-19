// Navigation (DESIGN.md "Navigation"): the live state of the pure model in
// `lib/navModel.ts`. Per source, one back stack per browse root (Home,
// Search, Library) and the active root; Preferences is shown over them,
// for both sources. The top bar switches roots, views push, Back pops.
// Switching the source shows the other source's stacks where they were
// left. Library's section is remembered per source in `localStorage`.

import { useSyncExternalStore } from "react";
import * as model from "../lib/navModel";
import { createStaged } from "../lib/staged";
import { viewTransitionsOn } from "../lib/motion";
import { pageTransition } from "../lib/pageTransition";
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

/**
 * The navigation, staged (`lib/staged.ts`): `staged.state` is the navigation
 * as it is, which every `nav` call reads and changes at once, so calls in
 * a row build on each other; `staged.shown` is what the hooks return. It
 * follows at once or, when the shown page changes and View Transitions
 * run, in the transition's update (`lib/pageTransition.ts`), after the old
 * screen is taken. Changes made while that update is pending go with it
 * (one transition, not a queue of them).
 */
const listeners = new Set<() => void>();
const staged = createStaged<NavState>(
  model.initialNav({
    local: loadSection("local"),
    navidrome: loadSection("navidrome"),
  }),
  {
    transitions: (from, to) => model.topEntry(from) !== model.topEntry(to) && viewTransitionsOn(),
    start: (from, to, publish) =>
      pageTransition(model.topEntry(from), model.topEntry(to), model.pageChange(from, to), () =>
        model.topEntry(publish()),
      ),
    notify: () => listeners.forEach((l) => l()),
  },
);

function set(next: NavState) {
  staged.set(next);
}

/** Focuses the search field once Search is shown (it may already be). */
function focusSearch() {
  staged.whenShown(() =>
    requestAnimationFrame(() => document.querySelector<HTMLInputElement>(".search-input")?.focus()),
  );
}

export const nav = {
  /** Opens `route` on the active root's stack (from Preferences: Library's). */
  push(route: Route) {
    set(model.push(staged.state, route));
  },
  /** Replaces the top route (e.g. a new sort), keeping the stack. */
  replace(route: Route) {
    set(model.replaceTop(staged.state, route));
  },
  /** Shows the top route again from scratch (reloads it). */
  reload() {
    nav.replace(nav.top().route);
  },
  /** Back (from Preferences: its browse root); a root's root stays. Returns `false` there. */
  back(): boolean {
    const next = model.back(staged.state);
    if (!next) return false;
    set(next);
    return true;
  },
  /** The shown browse stack back at its root. */
  toRoot() {
    const next = model.toRoot(staged.state);
    if (next) set(next);
  },
  /** The browse root a push opens on (from Preferences: Library). */
  openRoot(): BrowseRoot {
    return model.openRootFor(staged.state);
  },
  /** Go to album/artist: `route` on `root` of the (already switched) source. */
  goTo(route: Route, root: BrowseRoot) {
    set(model.goTo(staged.state, root, route));
  },
  /** A top bar entry: that root, or the active one back to its root. */
  selectRoot(root: Root) {
    set(model.selectRoot(staged.state, root));
  },
  /** Library's section (remembered for the source). */
  setSection(section: Section) {
    set(model.setSection(staged.state, section));
    saveSection(staged.state.source, model.current(staged.state).section);
  },
  /** Shows Search and focuses its field; `query` starts a new search. */
  search(query = "") {
    set(model.search(staged.state, query));
    focusSearch();
  },
  top(): Entry {
    return model.topEntry(staged.state);
  },
  /** The source whose stacks are shown. */
  source(): SourceKind {
    return staged.state.source;
  },
  /**
   * Shows `source`'s stacks (where they were left). Callers go through
   * `state/source.ts`, which also saves the choice.
   */
  setSource(source: SourceKind) {
    set(model.setSource(staged.state, source));
  },
};

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const getState = () => staged.shown;
const getStack = () => model.currentStack(staged.shown);

/** The shown stack (the active source's active root's). */
export function useNav(): Entry[] {
  return useSyncExternalStore(subscribe, getStack);
}

/** The whole navigation state (sections, stacks). */
export function useNavState(): NavState {
  return useSyncExternalStore(subscribe, getState);
}

const getRoot = () => model.activeRoot(staged.shown);

/** The top bar's active entry. */
export function useActiveRoot(): Root {
  return useSyncExternalStore(subscribe, getRoot);
}

/**
 * The library or server was replaced (settings): every view of that
 * source holds stale ids, so its stacks restart at their roots, which reload.
 */
export function sourceReplaced(source: SourceKind) {
  set(model.resetSource(staged.state, source));
}
