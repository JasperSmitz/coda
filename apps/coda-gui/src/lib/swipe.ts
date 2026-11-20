// A two-finger horizontal swipe between Home, Search and Library
// (DESIGN.md "Navigation"; hooks/useRootSwipe.ts listens), as pure state
// changes (`node --test scripts/`, scripts/swipe.test.mjs).
//
// A gesture is a run of wheel events with less than `SWIPE_IDLE_MS`
// between them (which also takes in the momentum events after the
// fingers lift: on macOS they come every frame for a second or more).
// Its first event that moves decides it: mostly sideways, and where a
// swipe may switch (`allowed`), it is tracked in that direction;
// otherwise (it starts vertical, or somewhere that blocks it) it is
// ignored to its end. One that starts while a page change runs (`busy`)
// waits: its travel is kept and it is decided once the change is over. A
// tracked gesture adds up its sideways events and switches once, when the
// travel in its direction passes `SWIPE_PX`.
//
// After a switch the gesture is locked for at least `SWIPE_COOLDOWN_MS`
// and while a page change runs; it ends with a pause of `SWIPE_IDLE_MS`
// outside the lock, or with a new swipe seen in the stream. Momentum only
// slows down, so fingers on the trackpad again show as a rise in **speed**
// (|deltaX| over the time since the last event, at least 8ms: WebKit
// delivers the events it held back while busy as one, with their deltas
// summed, so a raw delta can jump while the speed does not): after the
// speed has fallen to at most `DECAY_SHARE` of its peak since the switch,
// `RISE_EVENTS` events in a row each faster than the one before, the last
// at least `RISE_FACTOR` × the slowest since that peak and
// `RISE_MIN_SPEED`. Seen while locked, the new swipe is remembered with its
// travel and starts as soon as the lock ends. The caller keeps one state
// for the app, so a page change never resets it.
//
// `blocksSwipe` decides where a gesture may not start (pure, over a small
// view of the element and its ancestors: `SwipeNode`).

import type { BrowseRoot, Root } from "./navModel";

/** Sideways travel that makes a swipe, in pixels. */
export const SWIPE_PX = 120;
/** A pause without wheel events this long ends a gesture. */
export const SWIPE_IDLE_MS = 250;
/** After a switch, no new gesture starts for at least this long. */
export const SWIPE_COOLDOWN_MS = 350;
/** A new swipe within a switched gesture: once the speed has fallen to this share of its peak… */
export const DECAY_SHARE = 0.5;
/** …this many events in a row, each faster than the one before… */
export const RISE_EVENTS = 3;
/** …the last this many times the slowest since the peak… */
export const RISE_FACTOR = 2;
/** …and at least this fast (px/ms: 6px a frame). */
export const RISE_MIN_SPEED = 6 / 16;
/** Speeds are measured over at least this long (ms): events a moment apart are not infinitely fast. */
const MIN_DT = 8;

export interface SwipeState {
  /**
   * `idle`: nothing moved yet; `waiting`: started during a page change;
   * `tracking`; `done` (switched) and `ignored` wait for the end.
   */
  phase: "idle" | "waiting" | "tracking" | "done" | "ignored";
  /** The gesture's direction (1: towards the next root), decided by its first sideways event. */
  direction: 1 | -1;
  /** The sideways travel (tracking, waiting; after a switch: a new swipe's, once seen). */
  dx: number;
  /** The last wheel event's time (ms). */
  last: number;
  /** After a switch: no new gesture before this time (ms). */
  lockedUntil: number;
  /** After a switch: the peak speed, the slowest since it, the last speed and the rising run. */
  peak: number;
  floor: number;
  prevSpeed: number;
  rising: number;
  /** After a switch: a new swipe was seen (its travel in `dx`); it starts when the lock ends. */
  pending: boolean;
}

export const initialSwipe: SwipeState = {
  phase: "idle",
  direction: 1,
  dx: 0,
  last: Number.NEGATIVE_INFINITY,
  lockedUntil: Number.NEGATIVE_INFINITY,
  peak: 0,
  floor: Number.POSITIVE_INFINITY,
  prevSpeed: 0,
  rising: 0,
  pending: false,
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

type Result = { state: SwipeState; step: -1 | 0 | 1 };

/** Switches if `state`'s travel has passed the threshold (never while busy). */
function maybeSwitch(state: SwipeState, t: number, speed: number, busy: boolean): Result {
  if (busy || state.dx * state.direction < SWIPE_PX) return { state, step: 0 };
  return {
    state: {
      ...state,
      phase: "done",
      lockedUntil: t + SWIPE_COOLDOWN_MS,
      peak: speed,
      floor: speed,
      prevSpeed: speed,
      rising: 0,
      pending: false,
    },
    step: state.direction,
  };
}

/** Decides a gesture with travel `dx` so far: tracked if `allowed`, else ignored. */
function decide(state: SwipeState, allowed: (direction: 1 | -1) => boolean): SwipeState {
  return { ...state, phase: allowed(state.direction) ? "tracking" : "ignored" };
}

/**
 * One wheel event. `allowed(direction)` is asked once per gesture, when
 * it is decided (1: towards the next root, -1: the previous one): whether
 * a swipe may switch from where it started. Returns the new state and the
 * step to take (0: none).
 */
export function swipe(
  state: SwipeState,
  input: WheelInput,
  allowed: (direction: 1 | -1) => boolean,
): Result {
  const busy = input.busy ?? false;
  const dt = input.t - state.last;
  const paused = dt >= SWIPE_IDLE_MS;
  const sideways = Math.abs(input.dx) > Math.abs(input.dy);
  const speed = Math.abs(input.dx) / Math.max(dt, MIN_DT);
  let next: SwipeState = { ...state, last: input.t };

  if (state.phase === "done") {
    const locked = busy || input.t < state.lockedUntil;
    if (sideways) {
      // Watch for fingers again: a rise in speed after the decay.
      const newPeak = speed >= next.peak;
      const peak = newPeak ? speed : next.peak;
      const floor = newPeak ? speed : Math.min(next.floor, speed);
      const rising = speed > next.prevSpeed ? next.rising + 1 : 0;
      next = { ...next, peak, floor, prevSpeed: speed, rising };
      if (next.pending) {
        next = { ...next, dx: next.dx + input.dx };
      } else if (
        floor <= DECAY_SHARE * peak &&
        rising >= RISE_EVENTS &&
        speed >= RISE_FACTOR * floor &&
        speed >= RISE_MIN_SPEED
      ) {
        next = { ...next, pending: true, dx: input.dx, direction: input.dx > 0 ? 1 : -1 };
      }
    }
    if (locked) return { state: next, step: 0 };
    if (next.pending) {
      // The new swipe starts, with the travel it has made so far.
      next = decide({ ...next, pending: false }, allowed);
      return next.phase === "tracking" ? maybeSwitch(next, input.t, speed, busy) : { state: next, step: 0 };
    }
    if (!paused) return { state: next, step: 0 };
    next = { ...next, phase: "idle", dx: 0 };
  } else if (paused) {
    next = { ...next, phase: "idle", dx: 0 };
  }

  if (next.phase === "idle") {
    if (input.dx === 0 && input.dy === 0) return { state: next, step: 0 };
    if (!sideways) return { state: { ...next, phase: "ignored" }, step: 0 };
    next = { ...next, direction: input.dx > 0 ? 1 : -1, dx: input.dx };
    // During a page change it waits (its travel kept) to be decided after.
    if (busy) return { state: { ...next, phase: "waiting" }, step: 0 };
    next = decide(next, allowed);
    return next.phase === "tracking" ? maybeSwitch(next, input.t, speed, busy) : { state: next, step: 0 };
  }
  if (!sideways || (next.phase !== "tracking" && next.phase !== "waiting")) return { state: next, step: 0 };
  next = { ...next, dx: next.dx + input.dx };
  if (next.phase === "waiting") {
    if (busy) return { state: next, step: 0 };
    next = decide(next, allowed);
    if (next.phase !== "tracking") return { state: next, step: 0 };
  }
  // Only travel in the gesture's own direction switches.
  return maybeSwitch(next, input.t, speed, busy);
}

/** The roots a swipe moves between, in the top bar's order. */
export const SWIPE_ROOTS: BrowseRoot[] = ["home", "search", "library"];

/** The root one `step` from `active` (`null`: past an end, or from Preferences). */
export function swipeTarget(active: Root, step: -1 | 1): BrowseRoot | null {
  const at = SWIPE_ROOTS.indexOf(active as BrowseRoot);
  if (at < 0) return null;
  return SWIPE_ROOTS[at + step] ?? null;
}

// ----- where a swipe may not start -----

/** What `blocksSwipe` needs of an element (the hook reads it from the DOM). */
export interface SwipeNode {
  /** Lower case. */
  tag: string;
  /** An `<input>`'s type, lower case. */
  inputType?: string;
  role?: string | null;
  editable?: boolean;
  classes: readonly string[];
  /** Computed `overflow-x`. */
  overflowX: string;
  scrollWidth: number;
  clientWidth: number;
  scrollLeft: number;
  parent: SwipeNode | null;
}

/** Text fields a swipe may cross while their text fits. */
const TEXT_INPUTS = new Set(["text", "search", "url", "email", "tel", "password", ""]);
/** Where a swipe means something else (or should do nothing). */
const BLOCKING_CLASSES = ["queue-popover", "now-playing", "dialog-backdrop"];
const BLOCKING_ROLES = ["dialog", "menu", "slider"];

/** Scrolls sideways (`overflow-x` auto or scroll) and has at least 2px left to go `direction`. */
function canScroll(node: SwipeNode, direction: 1 | -1): boolean {
  if (node.overflowX !== "auto" && node.overflowX !== "scroll") return false;
  const room =
    direction > 0 ? node.scrollWidth - node.clientWidth - node.scrollLeft : node.scrollLeft;
  return room >= 2;
}

/**
 * Whether a gesture going `direction` that starts on `target` must not
 * switch: inside a text area, a select, editable text, a slider, the
 * queue panel, Now Playing, a dialog or a menu; a text field whose text
 * is wider than it (it scrolls); or an element (not clipped: `overflow-x`
 * auto or scroll) that can still scroll that way.
 */
export function blocksSwipe(target: SwipeNode, direction: 1 | -1): boolean {
  for (let node: SwipeNode | null = target; node; node = node.parent) {
    if (node.tag === "textarea" || node.tag === "select" || node.editable) return true;
    if (node.role && BLOCKING_ROLES.includes(node.role)) return true;
    if (node.classes.some((c) => BLOCKING_CLASSES.includes(c))) return true;
    if (node.tag === "input") {
      if (!TEXT_INPUTS.has(node.inputType ?? "")) return true;
      if (node.scrollWidth > node.clientWidth + 1) return true;
      continue;
    }
    if (canScroll(node, direction)) return true;
  }
  return false;
}
