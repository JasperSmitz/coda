// Whether an element is near the page's view: tiles that are not
// virtualized ask for their colours (`useSeen`: once seen, it stays seen)
// and look up their covers (`useNear`: only while near, so a tile that
// leaves the view gives up its turn) only then (Genres as covers). One
// observer per scroller.

import { useEffect, useState, type RefObject } from "react";
import { usePageScroller } from "./useLayout";

/** How far outside the view a tile counts as near. */
const MARGIN = "300px 0px";

const observers = new WeakMap<
  Element,
  { observer: IntersectionObserver; callbacks: Map<Element, (near: boolean) => void> }
>();

function observe(root: Element, el: Element, onChange: (near: boolean) => void): () => void {
  let entry = observers.get(root);
  if (!entry) {
    const callbacks = new Map<Element, (near: boolean) => void>();
    const observer = new IntersectionObserver(
      (records) => {
        for (const record of records) callbacks.get(record.target)?.(record.isIntersecting);
      },
      { root, rootMargin: MARGIN },
    );
    entry = { observer, callbacks };
    observers.set(root, entry);
  }
  const { observer, callbacks } = entry;
  callbacks.set(el, onChange);
  observer.observe(el);
  return () => {
    callbacks.delete(el);
    observer.unobserve(el);
  };
}

/** `true` while `ref`'s element is near the page's view. */
export function useNear(ref: RefObject<Element | null>): boolean {
  const scroller = usePageScroller();
  const [near, setNear] = useState(false);
  useEffect(() => {
    const root = scroller.current;
    const el = ref.current;
    if (!root || !el) return;
    return observe(root, el, setNear);
  }, [scroller, ref]);
  return near;
}

/** `true` once `ref`'s element has been near the page's view. */
export function useSeen(ref: RefObject<Element | null>): boolean {
  const near = useNear(ref);
  const [seen, setSeen] = useState(false);
  if (near && !seen) setSeen(true);
  return seen || near;
}
