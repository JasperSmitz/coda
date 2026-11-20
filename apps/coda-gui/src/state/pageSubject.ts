// The cover a page is about (DESIGN.md "Colour": "a page takes the colours
// of what it shows"): an album page its cover, an artist page its image
// (or first album's), a playlist its cover (or first track's), a genre
// page its first album's. The view reports it for its navigation entry
// (`undefined` while it is still loading, so the page's ground can wait
// for it instead of going to the base); App colours the page (`.main`)
// and draws its glow only while that entry is the top one, so other
// routes stay unscoped (the house accent).

import { useEffect, useSyncExternalStore } from "react";
import type { Entry } from "./nav";

interface Subject {
  entryId: number;
  /** `null`: none; `undefined`: not known yet (loading). */
  url: string | null | undefined;
  /** The page draws the full-width glow (pages with a cover bloom do not, 2.0 R3). */
  glow: boolean;
}

let subject: Subject | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * A view: `entry`'s page is about the cover at `url` (`null`: none;
 * `undefined`: still loading). `glow: false`: the page shows its own
 * bloom of the cover (Home, album and playlist pages), so App draws no
 * full-width glow behind its top.
 */
export function usePageSubject(
  entry: Entry,
  url: string | null | undefined,
  { glow = true }: { glow?: boolean } = {},
) {
  const id = entry.id;
  useEffect(() => {
    if (subject?.entryId === id && subject.url === url && subject.glow === glow) return;
    subject = { entryId: id, url, glow };
    listeners.forEach((l) => l());
  }, [id, url, glow]);
}

/** Whether the page shown for entry `entryId` wants the full-width glow. */
export function usePageGlow(entryId: number | null): boolean {
  return useSyncExternalStore(subscribe, () =>
    entryId !== null && subject?.entryId === entryId ? subject.glow : true,
  );
}

/**
 * The cover of the page shown for entry `entryId` (`null`: none, or not
 * that page; `undefined`: that page is still loading its subject).
 */
export function usePageCover(entryId: number | null): string | null | undefined {
  return useSyncExternalStore(subscribe, () =>
    entryId !== null && subject?.entryId === entryId ? subject.url : null,
  );
}
