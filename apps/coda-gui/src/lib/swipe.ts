// A two-finger horizontal swipe between Home, Search and Library
// (DESIGN.md "Navigation"; hooks/useRootSwipe.ts listens), as pure state
// changes (`node --test scripts/`, scripts/swipe.test.mjs).
//
// 2.0 R3, one simple rule: **one switch per stream of wheel events**.
// - A stream is a run of wheel events with gaps shorter than
//   `SWIPE_GAP_MS` (on the events' own time stamps); a quiet gap that long
//   ends it. Momentum after the fingers lift is part of the same stream.
// - Its first event that moves decides it: mostly sideways, and where a
//   swipe may switch (`allowed`), it is tracked in that direction;
//   otherwise (vertical, blocked) it is ignored to its end.
// - A tracked stream adds up its sideways events and switches once, when
//   the travel in its direction passes `SWIPE_PX`; then it is locked until
//   it ends.
// - No stream may start while a page change runs, nor until
//   `SWIPE_SETTLE_MS` after the last one ended (or after the switch, where
//   no page change ran): events the webview held back while drawing the
//   new page and delivered late must not count as a new swipe. A stream
//   that starts before then is ignored for its whole length.
// Every constant is here, to be tuned from the dev trace
// (hooks/useRootSwipe.ts, `scripts/smoke.md` "2.0 R3").
//
// `blocksSwipe` decides where a stream may not start (pure, over a small
// view of the element and its ancestors: `SwipeNode`).

import type { BrowseRoot, Root } from "./navModel";

/** Sideways travel that makes a swipe, in pixels. */
export const SWIPE_PX = 120;
/** A quiet gap this long (no wheel events at all) ends a stream. */
export const SWIPE_GAP_MS = 220;
/** After a page change (or a switch) ends, no stream starts for this long. */
export const SWIPE_SETTLE_MS = 300;

/** What a wheel event did (for the trace). */
export type SwipeNote =
  | "still"
  | "start"
  | "accumulate"
  | "switch"
  | "locked"
  | "ignored:vertical"
  | "ignored:blocked"
  | "ignored:settling"
  | "ignored";

export interface SwipeState {
  /** `idle`: nothing moved yet in this stream; `tracking`; `done` (switched) and `ignored` wait for its end. */
  phase: "idle" | "tracking" | "done" | "ignored";
  /** The stream's direction (1: towards the next root), decided by its first sideways event. */
  direction: 1 | -1;
  /** The tracked sideways travel. */
  dx: number;
  /** The last wheel event's time (ms). */
  last: number;
  /** The last switch's time (ms). */
  switchedAt: number;
}

export const initialSwipe: SwipeState = {
  phase: "idle",
  direction: 1,
  dx: 0,
  last: Number.NEGATIVE_INFINITY,
  switchedAt: Number.NEGATIVE_INFINITY,
};

export interface WheelInput {
  /** `deltaX` and `deltaY` in pixels. */
  dx: number;
  dy: number;
  /** The event's time (ms, the page's clock: `Event.timeStamp`). */
  t: number;
  /** A page change is running. */
  busy?: boolean;
  /** When the last page change ended (ms, the same clock; none: -Infinity). */
  changeEnded?: number;
}

/**
 * One wheel event. `allowed(direction)` is asked once per stream, when it
 * is decided (1: towards the next root, -1: the previous one): whether a
 * swipe may switch from where it started. Returns the new state, the
 * step to take (0: none) and what happened.
 */
export function swipe(
  state: SwipeState,
  input: WheelInput,
  allowed: (direction: 1 | -1) => boolean,
): { state: SwipeState; step: -1 | 0 | 1; note: SwipeNote } {
  const busy = input.busy ?? false;
  const fresh = input.t - state.last >= SWIPE_GAP_MS;
  let next: SwipeState = fresh ? { ...state, phase: "idle", dx: 0, last: input.t } : { ...state, last: input.t };
  const sideways = Math.abs(input.dx) > Math.abs(input.dy);

  if (next.phase === "done") return { state: next, step: 0, note: "locked" };
  if (next.phase === "ignored") return { state: next, step: 0, note: "ignored" };
  if (next.phase === "idle") {
    if (input.dx === 0 && input.dy === 0) return { state: next, step: 0, note: "still" };
    const settled = Math.max(input.changeEnded ?? Number.NEGATIVE_INFINITY, next.switchedAt) + SWIPE_SETTLE_MS;
    if (busy || input.t < settled) return { state: { ...next, phase: "ignored" }, step: 0, note: "ignored:settling" };
    if (!sideways) return { state: { ...next, phase: "ignored" }, step: 0, note: "ignored:vertical" };
    const direction = input.dx > 0 ? 1 : -1;
    if (!allowed(direction)) return { state: { ...next, phase: "ignored" }, step: 0, note: "ignored:blocked" };
    next = { ...next, phase: "tracking", direction, dx: 0 };
  }
  // Tracking: only sideways events add travel.
  if (sideways) next = { ...next, dx: next.dx + input.dx };
  // Not while a page change runs (one started otherwise, mid-swipe).
  if (next.dx * next.direction < SWIPE_PX || busy) {
    return { state: next, step: 0, note: state.phase === "tracking" && !fresh ? "accumulate" : "start" };
  }
  return { state: { ...next, phase: "done", switchedAt: input.t }, step: next.direction, note: "switch" };
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
