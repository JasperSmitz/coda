// FLIP: when `key` changes, elements that the new layout moves (or
// resizes) glide from where they were to where they are, with transforms
// only (no layout property animates, so text never rewraps mid-way).
//
// "First" is measured while rendering, before React commits the change
// (the DOM still shows the old layout); "last" in a layout effect after
// it, before the browser paints. A change in the middle of a glide starts
// from where the element is seen at that moment, so nothing jumps.

import { useLayoutEffect, useRef } from "react";
import { animate } from "../lib/motion";

export interface FlipTarget {
  element: HTMLElement;
  /** Also scale (for a box whose size changes, such as a cover); text moves only. */
  scale?: boolean;
}

/** The glide running on each element, so a new one replaces it. */
const running = new WeakMap<Element, Animation>();

export function useFlip(key: string, targets: () => FlipTarget[]): void {
  const last = useRef(key);
  const first = useRef<Map<HTMLElement, DOMRect> | null>(null);
  if (key !== last.current) {
    last.current = key;
    const rects = new Map<HTMLElement, DOMRect>();
    for (const { element } of targets()) {
      if (element.isConnected) rects.set(element, element.getBoundingClientRect());
    }
    first.current = rects;
  }

  const targetsRef = useRef(targets);
  targetsRef.current = targets;

  useLayoutEffect(() => {
    const before = first.current;
    first.current = null;
    if (!before) return;
    for (const { element, scale } of targetsRef.current()) {
      const from = before.get(element);
      // A new element (another track's cover) has no "before".
      if (!from) continue;
      running.get(element)?.cancel();
      running.delete(element);
      const to = element.getBoundingClientRect();
      if (to.width === 0 || to.height === 0) continue;
      const dx = from.left - to.left;
      const dy = from.top - to.top;
      const sx = scale ? from.width / to.width : 1;
      const sy = scale ? from.height / to.height : 1;
      if (Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5 && Math.abs(sx - 1) < 0.005 && Math.abs(sy - 1) < 0.005) {
        continue;
      }
      const animation = animate(
        element,
        [
          { transformOrigin: "0 0", transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})` },
          { transformOrigin: "0 0", transform: "none" },
        ],
        "--motion-slow",
      );
      if (!animation) continue;
      running.set(element, animation);
      animation.addEventListener("finish", () => {
        if (running.get(element) === animation) running.delete(element);
      });
    }
  }, [key]);
}
