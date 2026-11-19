// Library's per-section choices (DESIGN.md "Library"): Covers or List for
// Albums, Artists, Genres, Playlists and a genre's albums, and the Albums sort. Each is
// remembered per source in `localStorage`; Covers / List falls back to
// the Preferences default (`settings.toml` `libraryView`) until a section
// has its own choice.
//
// The Preferences default is saved through the backend like the accent
// (LESSONS: a controlled input whose save round-trips): a choice shows at
// once, saves go one at a time with the newest waiting choice next, and
// only the answer to the last choice replaces what is shown.

import { useSyncExternalStore } from "react";
import { errorText, prefsApi } from "../lib/ipc";
import { localOrderOf, serverSortOf } from "../lib/navModel";
import type { AlbumSort, LibraryView, LocalAlbumOrder, SourceKind } from "../lib/types";
import { useAccent } from "./accent";

/** The sections with a Covers / List choice. */
export type ViewSection = "albums" | "artists" | "genres" | "playlists" | "genreAlbums";

const viewKey = (source: SourceKind, section: ViewSection) =>
  `coda.libraryView.${source}.${section}`;
const sortKey = (source: SourceKind) => `coda.albumSort.${source}`;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembered; harmless.
  }
}

const isView = (value: unknown): value is LibraryView => value === "covers" || value === "list";

interface DefaultState {
  /** The saved default, as the backend last answered (`null`: not yet). */
  saved: LibraryView | null;
  /** The choice shown now; ahead of `saved` while a save runs. */
  shown: LibraryView | null;
  error: string | null;
}

let defaults: DefaultState = { saved: null, shown: null, error: null };
/** Sections' own choices, read from storage once. */
const sections = new Map<string, LibraryView | null>();
const listeners = new Set<() => void>();
let version = 0;

function notify() {
  version++;
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function sectionView(source: SourceKind, section: ViewSection): LibraryView | null {
  const key = viewKey(source, section);
  if (!sections.has(key)) {
    const value = read(key);
    sections.set(key, isView(value) ? value : null);
  }
  return sections.get(key) ?? null;
}

let sending = false;
let waiting: LibraryView | null = null;

function sendNext() {
  if (sending || waiting === null) return;
  const value = waiting;
  waiting = null;
  sending = true;
  prefsApi
    .setLibraryView(value)
    .then((prefs) => {
      defaults =
        waiting === null
          ? { saved: prefs.libraryView, shown: prefs.libraryView, error: null }
          : { ...defaults, saved: prefs.libraryView };
      notify();
    })
    .catch((e: unknown) => {
      defaults =
        waiting === null
          ? { ...defaults, shown: defaults.saved, error: errorText(e) }
          : { ...defaults, error: errorText(e) };
      notify();
    })
    .finally(() => {
      sending = false;
      sendNext();
    });
}

/** Chooses the Preferences default: shown at once, then saved. */
export function chooseLibraryDefault(view: LibraryView) {
  defaults = { ...defaults, shown: view, error: null };
  waiting = view;
  notify();
  sendNext();
}

/**
 * The Preferences default (the loaded preferences until a choice is made
 * here). `known`: the preferences have loaded (or failed to: then Covers).
 */
export function useLibraryDefault(): { view: LibraryView; known: boolean; error: string | null } {
  const { prefs, error } = useAccent();
  useSyncExternalStore(subscribe, () => version);
  const chosen = defaults.shown ?? defaults.saved ?? prefs?.libraryView ?? null;
  return {
    view: chosen ?? "covers",
    known: chosen !== null || error !== null,
    error: defaults.error,
  };
}

/**
 * A section's Covers / List, a setter that remembers it for this section
 * and source, and whether it is known yet: a section without its own
 * choice waits for the preferences (a List default must not show Covers
 * first; callers draw their loading state and hold the scroll restore).
 */
export function useLibraryView(
  source: SourceKind,
  section: ViewSection,
): [LibraryView, (view: LibraryView) => void, boolean] {
  const { view: fallback, known } = useLibraryDefault();
  const own = useSyncExternalStore(subscribe, () => sectionView(source, section));
  const set = (view: LibraryView) => {
    sections.set(viewKey(source, section), view);
    write(viewKey(source, section), view);
    notify();
  };
  return [own ?? fallback, set, own !== null || known];
}

/** A local Albums route's order: its own, else the remembered one (A–Z until one is chosen). */
export function localAlbumOrder(order?: LocalAlbumOrder): LocalAlbumOrder {
  return localOrderOf(order, order ? null : read(sortKey("local")));
}

/** A Navidrome Albums route's sort, like `localAlbumOrder`. */
export function serverAlbumSort(sort?: AlbumSort): AlbumSort {
  return serverSortOf(sort, sort ? null : read(sortKey("navidrome")));
}

/** Remembers the Albums sort chosen for `source`. */
export function rememberAlbumSort(source: SourceKind, sort: LocalAlbumOrder | AlbumSort) {
  write(sortKey(source), sort);
}
