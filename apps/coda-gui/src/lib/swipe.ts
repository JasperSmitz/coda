// A two-finger horizontal swipe between Home, Search and Library
// (DESIGN.md "Navigation"; hooks/useRootSwipe.ts listens), as pure state
// changes (`node --test scripts/`, scripts/swipe.test.mjs).
//
// A gesture is a run of wheel events with less than `SWIPE_IDLE_MS`
// between them (which also takes in the momentum events after the
// fingers lift: on macOS they come every frame for a second or more).
// Its first event that moves decides it: mostly sideways, and where a
// swipe may switch (`allowed`), it is tracked in that direction;
// otherwise it is ignored to its end. A tracked gesture adds up its
// sideways events and switches once, when the travel in its direction
// passes `SWIPE_PX`.
//
// After a switch the gesture stays locked: nothing switches until at
// least `SWIPE_COOLDOWN_MS` have passed and then a pause of
// `SWIPE_IDLE_MS` has ended the gesture. A pause while a page change runs
// (`busy`: the webview may hold events back while it draws the new page,
// then deliver the momentum after a gap) or within the cooldown never
// ends it. The caller keeps one state for the app, so a page change never
// resets it.

import type { BrowseRoot, Root } from "./navModel";

/** Sideways travel that makes a swipe, in pixels. */
export const SWIPE_PX = 120;
/** A pause without wheel events this long ends a gesture. */
export const SWIPE_IDLE_MS = 250;
/** After a switch, no new gesture starts for at least this long. */
export const SWIPE_COOLDOWN_MS = 600;

export interface SwipeState {
  /** `idle`: nothing moved yet; `tracking`; `done` (switched) and `ignored` wait for the end. */
  phase: "idle" | "tracking" | "done" | "ignored";
  /** The tracked gesture's direction (1: towards the next root), decided by its first sideways event. */
  direction: 1 | -1;
  /** The tracked sideways travel. */
  dx: number;
  /** The last wheel event's time (ms). */
  last: number;
  /** After a switch: no new gesture before this time (ms). */
  lockedUntil: number;
}

export const initialSwipe: SwipeState = {
  phase: "idle",
  direction: 1,
  dx: 0,
  last: Number.NEGATIVE_INFINITY,
  lockedUntil: Number.NEGATIVE_INFINITY,
};

export interface WheelInput {
  /** `deltaX` and `deltaY` in pixels. */
  dx: number;
  dy: number;
  /** The event's time (ms). */
  t: number;
  /** A page change is running: nothing switches or starts, and no pause ends a switched gesture. */
  busy?: boolean;
}

/**
 * One wheel event. `allowed(direction)` is asked once, at the gesture's
 * first sideways event (1: towards the next root, -1: the previous one):
 * whether a swipe may switch from where it started. Returns the new
 * state and the step to take (0: none).
 */
export function swipe(
  state: SwipeState,
  input: WheelInput,
  allowed: (direction: 1 | -1) => boolean,
): { state: SwipeState; step: -1 | 0 | 1 } {
  const busy = input.busy ?? false;
  const paused = input.t - state.last >= SWIPE_IDLE_MS;
  const locked = state.phase === "done" && (busy || input.t < state.lockedUntil);
  let next: SwipeState =
    paused && !locked
      ? { ...state, phase: "idle", dx: 0, last: input.t }
      : { ...state, last: input.t };
  const sideways = Math.abs(input.dx) > Math.abs(input.dy);
  if (next.phase === "idle") {
    if (input.dx === 0 && input.dy === 0) return { state: next, step: 0 };
    const direction = input.dx > 0 ? 1 : -1;
    const tracked = sideways && !busy && allowed(direction);
    next = { ...next, direction, phase: tracked ? "tracking" : "ignored" };
  }
  if (next.phase !== "tracking" || !sideways) return { state: next, step: 0 };
  const dx = next.dx + input.dx;
  // Only travel in the gesture's own direction switches.
  if (dx * next.direction < SWIPE_PX || busy) return { state: { ...next, dx }, step: 0 };
  return {
    state: { ...next, dx, phase: "done", lockedUntil: input.t + SWIPE_COOLDOWN_MS },
    step: next.direction,
  };
}

/** The roots a swipe moves between, in the top bar's order. */
export const SWIPE_ROOTS: BrowseRoot[] = ["home", "search", "library"];

/** The root one `step` from `active` (`null`: past an end, or from Preferences). */
export function swipeTarget(active: Root, step: -1 | 1): BrowseRoot | null {
  const at = SWIPE_ROOTS.indexOf(active as BrowseRoot);
  if (at < 0) return null;
  return SWIPE_ROOTS[at + step] ?? null;
}
