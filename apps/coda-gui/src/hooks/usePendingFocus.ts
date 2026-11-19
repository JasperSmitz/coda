// Focusing an item of a virtualized list or grid after scrolling to it.
// The item may not be mounted yet: the scroll's own render (the scroller's
// scroll event, then the list's state update) mounts it later than any
// one `requestAnimationFrame`. So a focus that finds no element is kept
// pending and taken up by an effect after every render of the list until
// the element is there (LESSONS: focus after a programmatic scroll).

import { useEffect, useRef, type RefObject } from "react";

/**
 * `focus(index)` focuses the element with `data-index="<index>"` inside
 * `container` now, or once a later render of the calling component has
 * mounted it.
 */
export function usePendingFocus(container: RefObject<HTMLElement | null>): (index: number) => void {
  const pending = useRef<number | null>(null);
  const find = (index: number) =>
    container.current?.querySelector<HTMLElement>(`[data-index="${index}"]`) ?? null;
  useEffect(() => {
    const index = pending.current;
    if (index === null) return;
    const el = find(index);
    if (el) {
      el.focus({ preventScroll: true });
      pending.current = null;
    }
  });
  return (index: number) => {
    const el = find(index);
    if (el) {
      el.focus({ preventScroll: true });
      pending.current = null;
    } else {
      pending.current = index;
    }
  };
}
