// A two-finger horizontal swipe switches between Home, Search and
// Library (DESIGN.md "Navigation"), one root per gesture (lib/swipe.ts):
// a swipe to the left (content moving left, `deltaX` > 0) goes to the
// next root, to the right the previous one; nothing at the ends or from
// Preferences. It goes through `nav.selectRoot`, as a click on the top
// bar's entry does (the page transition, the scroll restored).
//
// A gesture is ignored when it starts inside a horizontally scrollable
// element that can still scroll its way, a text field or a slider, the
// queue panel, a dialog or a menu (or while one is open), and while a page
// transition runs. After a switch the gesture stays locked (its momentum,
// also when the webview delivers it after a pause while it draws the new
// page) for at least lib/swipe.ts's cooldown and until the wheel pauses
// outside a page change. The state is the module's, one for the app, so
// nothing that remounts or re-renders on navigation resets it. Not
// listening in Now Playing or onboarding. The listener is passive: it
// never stops the page or a list from scrolling.

import { useEffect } from "react";
import { initialSwipe, swipe, swipeTarget, type SwipeState } from "../lib/swipe";
import { nav } from "../state/nav";

/** Where a swipe never switches (and what it may be doing instead). */
const EXCLUDED = [
  "input",
  "textarea",
  "select",
  "[contenteditable]",
  '[role="slider"]',
  ".queue-popover",
  ".now-playing",
  ".dialog-backdrop",
  '[role="dialog"]',
  '[role="menu"]',
].join(", ");

/** Open things that hold the swipe wherever it starts. */
const OPEN_OVERLAYS = '.dialog-backdrop, [role="dialog"], [role="menu"]';

/** Whether an element between `target` and the page can still scroll sideways `direction`. */
function scrollsSideways(target: Element, direction: 1 | -1): boolean {
  for (let el: Element | null = target; el && el !== document.body; el = el.parentElement) {
    if (!(el instanceof HTMLElement) || el.scrollWidth <= el.clientWidth) continue;
    const overflow = getComputedStyle(el).overflowX;
    if (overflow !== "auto" && overflow !== "scroll") continue;
    const room =
      direction > 0 ? el.scrollWidth - el.clientWidth - el.scrollLeft : el.scrollLeft;
    if (room > 1) return true;
  }
  return false;
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

/** The gesture so far, for the app (kept across navigation and re-renders). */
let state: SwipeState = initialSwipe;

export function useRootSwipe(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const onWheel = (e: WheelEvent) => {
      const target = e.target instanceof Element ? e.target : null;
      const allowed = (direction: 1 | -1) =>
        target !== null &&
        !target.closest(EXCLUDED) &&
        !document.querySelector(OPEN_OVERLAYS) &&
        !scrollsSideways(target, direction);
      const result = swipe(
        state,
        {
          dx: pixels(e, e.deltaX),
          dy: pixels(e, e.deltaY),
          t: e.timeStamp,
          busy: transitionRunning(),
        },
        allowed,
      );
      state = result.state;
      if (result.step === 0) return;
      const root = swipeTarget(nav.activeRoot(), result.step);
      if (root) nav.selectRoot(root);
    };
    window.addEventListener("wheel", onWheel, { passive: true });
    return () => window.removeEventListener("wheel", onWheel);
  }, [enabled]);
}
