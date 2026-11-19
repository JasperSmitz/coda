// A state shown in two steps (pure: no React, no DOM; `scripts/staged.test.mjs`).
// `state` changes at once with every `set`, so calls in a row build on
// each other; `shown` (what the views draw) follows it at once, or, when
// a change is to be shown as a transition, only when the transition
// publishes it (in its update, after the old screen was taken). Changes
// made while that publish is pending go out with it: a burst of changes
// is one transition, never a queue of them. The navigation store
// (`state/nav.ts`) uses it for page transitions.

export interface StagedOptions<S> {
  /** This change is shown as a transition (e.g. the page changes and View Transitions run). */
  transitions(from: S, to: S): boolean;
  /** Starts the transition; it must call `publish` exactly once (in its update). */
  start(from: S, to: S, publish: () => S): void;
  /** The shown state changed (the views re-read it). */
  notify(): void;
}

export interface Staged<S> {
  /** The state as it is. */
  readonly state: S;
  /** The state as the views show it. */
  readonly shown: S;
  /** A publish is pending (a transition started, its update not run yet). */
  readonly pending: boolean;
  set(next: S): void;
  /** Runs `f` once the views show the state as it is now (at once if nothing is pending). */
  whenShown(f: () => void): void;
}

export function createStaged<S>(initial: S, options: StagedOptions<S>): Staged<S> {
  let state = initial;
  let shown = initial;
  let pending = false;
  let after: (() => void)[] = [];

  /** Shows the state as it is: first the views (`notify`), then what waited for it. */
  const publish = (): S => {
    pending = false;
    if (shown !== state) {
      shown = state;
      options.notify();
    }
    const waiting = after;
    after = [];
    waiting.forEach((f) => f());
    return shown;
  };

  return {
    get state() {
      return state;
    },
    get shown() {
      return shown;
    },
    get pending() {
      return pending;
    },
    set(next: S) {
      if (next === state) return;
      state = next;
      // The pending transition's publish takes this along.
      if (pending) return;
      if (!options.transitions(shown, state)) {
        publish();
        return;
      }
      pending = true;
      options.start(shown, state, publish);
    },
    whenShown(f: () => void) {
      if (pending) after.push(f);
      else f();
    },
  };
}
