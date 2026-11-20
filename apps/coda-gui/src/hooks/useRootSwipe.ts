// A two-finger horizontal swipe switches between Home, Search and
// Library (DESIGN.md "Navigation"), one root per gesture (lib/swipe.ts):
// a swipe to the left (content moving left, `deltaX` > 0) goes to the
// next root, to the right the previous one; nothing at the ends or from
// Preferences. It goes through `nav.selectRoot`, as a click on the top
// bar's entry does (the page transition, the scroll restored).
//
// A gesture is ignored when it starts inside a horizontally scrollable
// element that can still scroll its way, a text area, a text field whose
// text is wider than it (Search's field otherwise lets swipes through), a
// slider, the queue panel, a dialog or a menu (or while one is open)
// (`blocksSwipe`); one that starts while a page transition runs waits
// for it to end. After a switch the gesture stays locked (its momentum,
// also when the webview delivers it after a pause while it draws the new
// page) for at least lib/swipe.ts's cooldown and until the wheel pauses
// outside a page change or a new swipe rises out of the momentum (by
// speed, from the events' time stamps; one seen during the lock starts
// when it ends). The state is the module's, one for the app, so
// nothing that remounts or re-renders on navigation resets it. Not
// listening in Now Playing or onboarding. The listener is passive: it
// never stops the page or a list from scrolling.

import { useEffect } from "react";
import {
  blocksSwipe,
  initialSwipe,
  swipe,
  swipeTarget,
  type SwipeNode,
  type SwipeState,
} from "../lib/swipe";
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

/** The gesture so far, for the app (kept across navigation and re-renders). */
let state: SwipeState = initialSwipe;

export function useRootSwipe(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const onWheel = (e: WheelEvent) => {
      const target = e.target instanceof Element ? e.target : null;
      const allowed = (direction: 1 | -1) => {
        if (!target || document.querySelector(OPEN_OVERLAYS)) return false;
        const node = swipeNode(target);
        return node !== null && !blocksSwipe(node, direction);
      };
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
