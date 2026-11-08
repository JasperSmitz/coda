// Layout helpers for virtualized lists: an element's size, its scroll
// position (restored from the navigation entry on Back), and a library
// version that changes while the scan grows the library.

import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { Entry } from "../state/nav";
import { useAppState } from "../state/store";

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
 * Tracks `ref`'s scroll position in `entry` and restores it once the
 * content is tall enough (`ready`).
 */
export function useScroll(
  ref: RefObject<HTMLElement | null>,
  entry: Entry,
  ready: boolean,
): number {
  const [scrollTop, setScrollTop] = useState(entry.scrollTop);
  const restored = useRef(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || restored.current || !ready) return;
    restored.current = true;
    el.scrollTop = entry.scrollTop;
    setScrollTop(el.scrollTop);
  }, [ref, entry, ready]);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onScroll = () => {
      entry.scrollTop = el.scrollTop;
      setScrollTop(el.scrollTop);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [ref, entry]);
  return scrollTop;
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
