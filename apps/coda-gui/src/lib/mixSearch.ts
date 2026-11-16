// The mix editor's search, without React or IPC (so `node --test scripts/`
// can drive it with controlled promises): the three searches of a source
// for one query, debounced and run together. A newer query wins; what one
// search found is shown even if another failed. Results are only
// selectable once they belong to the completed search for the query and
// source in the field now (`resultsAreCurrent`); until then the editor
// shows them as busy.

import type { SearchGroups } from "./mixRecipe";
import type { Ingredient, SourceKind } from "./types";

export const SEARCH_DELAY_MS = 200;

export type SearchState =
  | { state: "idle" }
  /** `groups`: the last results, kept up (busy) while the next ones load. */
  | { state: "searching"; source: SourceKind; query: string; groups: SearchGroups | null }
  | {
      state: "done";
      source: SourceKind;
      query: string;
      groups: SearchGroups;
      failed: string | null;
    };

type Search = (source: SourceKind, query: string) => Promise<Ingredient[]>;

export interface Searchers {
  genres: Search;
  artists: Search;
  albums: Search;
}

/** The timer functions the search uses (the window's by default). */
export interface Timers {
  set: (run: () => void, ms: number) => unknown;
  clear: (handle: unknown) => void;
}

const defaultTimers: Timers = {
  set: (run, ms) => setTimeout(run, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Runs the editor's searches; `onChange` gets every new state. */
export class MixSearch {
  private state: SearchState = { state: "idle" };
  private timer: unknown = null;
  /** Bumped by every new query: a reply for an older one is dropped. */
  private generation = 0;

  private readonly api: Searchers;
  private readonly onChange: (state: SearchState) => void;
  private readonly delayMs: number;
  private readonly timers: Timers;

  // No parameter properties: Node's type stripping (the tests) cannot run them.
  constructor(
    api: Searchers,
    onChange: (state: SearchState) => void,
    delayMs = SEARCH_DELAY_MS,
    timers: Timers = defaultTimers,
  ) {
    this.api = api;
    this.onChange = onChange;
    this.delayMs = delayMs;
    this.timers = timers;
  }

  get current(): SearchState {
    return this.state;
  }

  /** Searches `source` for `query` (trimmed) after the delay; an empty one is idle. */
  set(source: SourceKind, query: string) {
    const needle = query.trim();
    const was = this.state;
    if (was.state !== "idle" && was.source === source && was.query === needle) return;
    this.cancel();
    if (needle === "") {
      this.emit({ state: "idle" });
      return;
    }
    const generation = this.generation;
    this.emit({
      state: "searching",
      source,
      query: needle,
      groups: was.state === "idle" ? null : was.groups,
    });
    this.timer = this.timers.set(() => {
      this.timer = null;
      void Promise.allSettled([
        this.api.genres(source, needle),
        this.api.artists(source, needle),
        this.api.albums(source, needle),
      ]).then(([genres, artists, albums]) => {
        if (generation !== this.generation) return;
        const all = [genres, artists, albums];
        const value = (r: PromiseSettledResult<Ingredient[]>) =>
          r.status === "fulfilled" ? r.value : [];
        const failure = all.find((r) => r.status === "rejected");
        const allFailed = all.every((r) => r.status === "rejected");
        this.emit({
          state: "done",
          source,
          query: needle,
          groups: { genres: value(genres), artists: value(artists), albums: value(albums) },
          failed:
            failure && failure.status === "rejected"
              ? `${allFailed ? "Could not search" : "Some results are missing"}: ${
                  (failure.reason as { message?: string } | null)?.message ??
                  "something went wrong"
                }`
              : null,
        });
      });
    }, this.delayMs);
  }

  /** Stops: no timer, and no later reply is reported. */
  dispose() {
    this.cancel();
  }

  private cancel() {
    this.generation += 1;
    if (this.timer !== null) {
      this.timers.clear(this.timer);
      this.timer = null;
    }
  }

  private emit(state: SearchState) {
    this.state = state;
    this.onChange(state);
  }
}

/**
 * Whether `search`'s results may be chosen: only those of the completed
 * search for `query` (trimmed) of `source`. Otherwise they are the
 * previous query's, shown busy.
 */
export function resultsAreCurrent(search: SearchState, source: SourceKind, query: string): boolean {
  return search.state === "done" && search.source === source && search.query === query.trim();
}
