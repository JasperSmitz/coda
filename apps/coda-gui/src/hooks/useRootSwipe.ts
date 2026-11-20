// A two-finger horizontal swipe switches between Home, Search and
// Library (DESIGN.md "Navigation"), one root per gesture (lib/swipe.ts):
// a swipe to the left (content moving left, `deltaX` > 0) goes to the
// next root, to the right the previous one; nothing at the ends or from
// Preferences. It goes through `nav.selectRoot`, as a click on the top
// bar's entry does (the page transition, the scroll restored).
//
// A stream is ignored when it starts inside a horizontally scrollable
// element that can still scroll its way, a text area, a text field whose
// text is wider than it, a slider, the queue panel, a dialog or a menu
// (or while one is open) (`blocksSwipe`), while a page change runs and
// until lib/swipe.ts's settle time after it ended (watched on <html>'s
// `data-vt`, on the same clock as the events' time stamps). One switch
// per stream of wheel events (lib/swipe.ts). The state is the module's,
// one for the app, so nothing that remounts or re-renders on navigation
// resets it. Not listening in Now Playing or onboarding. The listener is
// passive: it never stops the page or a list from scrolling.
//
// Development builds trace every wheel event it sees (time stamp, deltas
// and their mode, whether a page change runs, what the rule did) into the
// app's log (lib/devTrace.ts), to tune the rule from what the webview
// really delivers (`scripts/smoke.md` "2.0 R3").

import { useEffect } from "react";
import {
  blocksSwipe,
  initialSwipe,
  swipe,
  swipeTarget,
  type SwipeNode,
  type SwipeState,
} from "../lib/swipe";
import { devTrace } from "../lib/devTrace";
import { nav } from "../state/nav";

/** Open things that hold the swipe wherever it starts. */
const OPEN_OVERLAYS = '.dialog-backdrop, [role="dialog"], [role="menu"]';

/** `element` and its ancestors up to the page's body, as lib/swipe.ts reads them. */
function swipeNode(element: Element | null): SwipeNode | null {
  if (!element || element === document.body || element === document.documentElement) return null;
  const html = element instanceof HTMLElement ? element : null;
  return {
    tag: element.tagName.toLowerCase(),
    inputType: element instanceof HTMLInputElement ? element.type.toLowerCase() : undefined,
    role: element.getAttribute("role"),
    editable: html?.isContentEditable ?? false,
    classes: [...element.classList],
    overflowX: getComputedStyle(element).overflowX,
    scrollWidth: element.scrollWidth,
    clientWidth: element.clientWidth,
    scrollLeft: element.scrollLeft,
    parent: swipeNode(element.parentElement),
  };
}

/** A page transition is running (lib/motion.ts marks <html> while one does). */
function transitionRunning(): boolean {
  return document.documentElement.dataset.vt !== undefined;
}

/** The pixels a wheel event moves (line and page deltas converted). */
function pixels(e: WheelEvent, delta: number): number {
  if (e.deltaMode === WheelEvent.DOM_DELTA_LINE) return delta * 16;
  if (e.deltaMode === WheelEvent.DOM_DELTA_PAGE) return delta * window.innerWidth;
  return delta;
}

/** The stream so far, for the app (kept across navigation and re-renders). */
let state: SwipeState = initialSwipe;
/** When the last page change ended (`performance.now()`, the events' clock). */
let changeEnded = Number.NEGATIVE_INFINITY;

export function useRootSwipe(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    // A page change's end: <html> loses `data-vt`.
    let wasBusy = transitionRunning();
    const observer = new MutationObserver(() => {
      const busy = transitionRunning();
      if (wasBusy && !busy) changeEnded = performance.now();
      wasBusy = busy;
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-vt"] });
    const onWheel = (e: WheelEvent) => {
      const target = e.target instanceof Element ? e.target : null;
      const allowed = (direction: 1 | -1) => {
        if (!target || document.querySelector(OPEN_OVERLAYS)) return false;
        const node = swipeNode(target);
        return node !== null && !blocksSwipe(node, direction);
      };
      const busy = transitionRunning();
      const result = swipe(
        state,
        {
          dx: pixels(e, e.deltaX),
          dy: pixels(e, e.deltaY),
          t: e.timeStamp,
          busy,
          changeEnded,
        },
        allowed,
      );
      state = result.state;
      if (import.meta.env.DEV) {
        devTrace(
          `swipe t=${e.timeStamp.toFixed(1)} dx=${e.deltaX.toFixed(1)} dy=${e.deltaY.toFixed(1)} ` +
            `mode=${e.deltaMode} busy=${busy ? 1 : 0} ended=${changeEnded.toFixed(1)} ` +
            `${result.note} phase=${state.phase} travel=${state.dx.toFixed(0)}`,
        );
      }
      if (result.step === 0) return;
      const root = swipeTarget(nav.activeRoot(), result.step);
      if (root) nav.selectRoot(root);
    };
    window.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      window.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [enabled]);
}
