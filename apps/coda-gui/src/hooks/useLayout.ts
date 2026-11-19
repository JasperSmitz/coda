// Layout helpers for the page and its virtualized lists: the page's
// scroll container (shared by every view), an element's size, the page's
// scroll position (restored from the navigation entry on Back), keeping a
// row in view clear of what floats over the page, and a library version
// that changes while the scan grows the library.

import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import { pageSettled } from "../lib/pageTransition";
import type { Entry } from "../state/nav";
import { useAppState } from "../state/store";

/**
 * The page's scroll container (App's `.main.page`): one per window, shared
 * by every view. The view's header, its tools and its lists all scroll in
 * it, under the top bar and the player.
 */
export const PageScroller = createContext<RefObject<HTMLElement | null>>({ current: null });

/** The page's scroll container (virtualized lists measure against it). */
export function usePageScroller(): RefObject<HTMLElement | null> {
  return useContext(PageScroller);
}

export function useElementSize(ref: RefObject<HTMLElement | null>): {
  width: number;
  height: number;
} {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize({ width: el.clientWidth, height: el.clientHeight });
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    measure();
    return () => observer.disconnect();
  }, [ref]);
  return size;
}

/**
 * What floats over `el`'s scrolling area at its top (the top bar, and a
 * page's sticky tool row) and at its bottom (the player): its
 * `scroll-padding` (app.css `.main.page`), which `scrollIntoView` honours
 * too.
 */
export function coveredEdges(el: HTMLElement): { top: number; bottom: number } {
  const style = getComputedStyle(el);
  return {
    top: parseFloat(style.scrollPaddingTop) || 0,
    bottom: parseFloat(style.scrollPaddingBottom) || 0,
  };
}

/** The height of `el`'s scrolling area that nothing floats over. */
export function clearHeight(el: HTMLElement): number {
  const { top, bottom } = coveredEdges(el);
  return Math.max(0, el.clientHeight - top - bottom);
}

/**
 * Scrolls `el` (the page) just enough that the row at `top` (in its
 * content) and `height` tall is in view, clear of what floats over its
 * edges (`coveredEdges`).
 */
export function revealRow(el: HTMLElement, top: number, height: number) {
  const covered = coveredEdges(el);
  const room = Math.max(height, el.clientHeight - covered.top - covered.bottom);
  if (top - covered.top < el.scrollTop) el.scrollTop = top - covered.top;
  else if (top + height > el.scrollTop + covered.top + room)
    el.scrollTop = top + height - room - covered.top;
}

/** Scrolls `el` so that content at `top` is just below what floats over its top edge. */
export function scrollToTop(el: HTMLElement, top: number) {
  el.scrollTop = top - coveredEdges(el).top;
}

/**
 * Tracks the page's scroll position in `entry` and restores it once the
 * content is tall enough (`ready`). The scroller is shared by every page:
 * a page starts at the top until its own position is restored, and only
 * then records where it is (the scroll events of the page it replaced,
 * or of its own loading state, are not its position).
 *
 * `settled`: the page has finished loading, whatever came of it (its
 * content, empty, or an error; `ready` implies it). A settled page tells
 * a page transition waiting for it (`lib/pageTransition.ts`
 * `pageSettled`), after the restore if there is one: a transition must
 * not wait for content an empty or failed page never gets.
 */
export function useScroll(
  ref: RefObject<HTMLElement | null>,
  entry: Entry,
  ready: boolean,
  settled: boolean = ready,
): number {
  const [scrollTop, setScrollTop] = useState(0);
  const restored = useRef(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || restored.current) return;
    if (ready) {
      restored.current = true;
      el.scrollTop = entry.scrollTop;
      // Virtualized lists follow the scroll event, which the browser
      // fires only at its next frame: tell them now, so the rows at the
      // restored place render before anything is painted (and before a
      // page transition takes its new screen).
      el.dispatchEvent(new Event("scroll"));
    } else {
      el.scrollTop = 0;
    }
    setScrollTop(el.scrollTop);
  }, [ref, entry, ready]);
  // After the restore above (same commit, declared after it).
  useLayoutEffect(() => {
    if (settled || ready) pageSettled(entry.id);
  }, [entry, settled, ready]);
  // A layout effect: its cleanup runs as the page is replaced, before the
  // browser reports the scroll that the shorter content causes.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onScroll = () => {
      if (restored.current) entry.scrollTop = el.scrollTop;
      setScrollTop(el.scrollTop);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [ref, entry]);
  return scrollTop;
}

/**
 * `ref`'s offset in the page's scrolling content, kept current after
 * every render (content above it may load later).
 */
export function useOffsetTop(ref: RefObject<HTMLElement | null>): number {
  const [offset, setOffset] = useState(0);
  useLayoutEffect(() => {
    const next = ref.current?.offsetTop ?? 0;
    if (next !== offset) setOffset(next);
  });
  return offset;
}

const SCAN_REFRESH_MS = 2000;

/**
 * Changes at most every 2 s while the local scan adds tracks, and once
 * when it ends: local views reload quietly on it.
 */
export function useLibraryVersion(): string {
  const { snapshot } = useAppState();
  const state = snapshot?.scan.state ?? "noLibrary";
  const tracks = snapshot?.scan.tracks ?? 0;
  const playlists = snapshot?.scan.playlists ?? 0;
  const [version, setVersion] = useState(`${state}:${tracks}:${playlists}`);
  const last = useRef(0);
  useEffect(() => {
    const next = `${state}:${tracks}:${playlists}`;
    if (next === version) return;
    const wait = state === "scanning" ? last.current + SCAN_REFRESH_MS - Date.now() : 0;
    const timer = window.setTimeout(
      () => {
        last.current = Date.now();
        setVersion(next);
      },
      Math.max(0, wait),
    );
    return () => window.clearTimeout(timer);
  }, [state, tracks, playlists, version]);
  return version;
}
