// Page changes as View Transitions (DESIGN.md "Motion"): navigating
// between pages (a push, Back, a top bar entry, a Library section)
// crossfades the page while the top bar, the player and an open queue
// panel stay put (experience.css, `data-vt="page"`). Opening an album,
// artist or playlist from its cover morphs that cover into the page
// header's (`#page-cover`); Back morphs the header's cover back into the
// same cover when it is on screen. Without View Transitions, or with
// reduced motion, pages change at once (`viewTransition` applies the
// update directly).
//
// The navigation store (`state/nav.ts`) calls `pageTransition` when the
// shown page changes; it applies the change in the transition's update.
// The screen is frozen until the new screen is taken, so the update waits
// only for what it needs, against one shared deadline (`SETTLE_MS`): the
// new page to settle (`pageSettled`, from `useScroll`: loaded, empty or
// failed, its scroll restored) only when it has a place to restore or the
// cover to morph into is not there yet, then that cover's images to
// decode. Nothing jumps after the transition in the usual case; past the
// deadline the page goes on loading under the crossfade.
//
// The morph's name: one element at a time carries `data-vt-morph`, which
// experience.css names `page-cover` only while a page transition runs.
// It is set on the old cover just before the transition starts, moved to
// the new one in the update, and cleared when the transition ends, is
// skipped, or a newer one starts. Covers that can morph carry
// `data-morph` (`morphKey`): the kind, source and id of the page they open.

import { decodedWithin, viewTransition } from "./motion";
import type { Entry, PageChange, Route } from "./navModel";

/** The longest the new screen waits, for the new page and the cover's decode together. */
const SETTLE_MS = 150;

const MORPH_ATTR = "data-vt-morph";

/** A cover's `data-morph` value: the page it opens (album, artist or playlist). */
export function morphKey(kind: "album" | "artist" | "playlist", source: string, id: string): string {
  return `${kind}:${source}:${id}`;
}

/** The `data-morph` value of the covers that open `route` (`null`: none morph into it). */
export function routeMorphKey(route: Route): string | null {
  switch (route.kind) {
    case "album":
    case "artist":
    case "playlist":
      return morphKey(route.kind, route.source, route.id);
    default:
      return null;
  }
}

// ----- the new page settling -----

let shownId: number | null = null;
const waiters = new Set<{ id: number; resolve: () => void }>();

/**
 * The page of `entry` has settled: loaded (its scroll restored), empty,
 * or failed (`useScroll`). Not "has content": an empty or failed page
 * never gets any, and a wait for it would always run to its deadline.
 */
export function pageSettled(entryId: number) {
  shownId = entryId;
  for (const waiter of waiters) {
    if (waiter.id === entryId) {
      waiters.delete(waiter);
      waiter.resolve();
    }
  }
}

/** Resolves once `entryId`'s page has settled, or after `ms`. */
function whenSettled(entryId: number, ms: number): Promise<void> {
  if (shownId === entryId || ms <= 0) return Promise.resolve();
  return new Promise((resolve) => {
    const waiter = { id: entryId, resolve };
    waiters.add(waiter);
    window.setTimeout(() => {
      waiters.delete(waiter);
      resolve();
    }, ms);
  });
}

// ----- which cover was used -----

/** The element last pressed or typed into: where to look for the cover that opened a page. */
let activated: Element | null = null;

// Installed when the module loads (with the navigation store), so even
// the first navigation knows its cover. Capture: before any handler navigates.
if (typeof window !== "undefined") {
  const note = (e: Event) => {
    if (e.target instanceof Element) activated = e.target;
  };
  window.addEventListener("pointerdown", note, true);
  window.addEventListener("keydown", note, true);
}

/** Selects covers by `key` (keys hold only ids from our own DTOs; escaped anyway). */
function selector(key: string): string {
  return `[data-morph="${CSS.escape(key)}"]`;
}

/** On screen, clear of the top bar and the player, and not a hidden carousel neighbour. */
function onScreen(el: Element): boolean {
  // A hidden carousel neighbour hides its cover; the cover itself may
  // carry `aria-hidden` (a placeholder, a mosaic): that is not hiding it.
  if (el.parentElement?.closest('[aria-hidden="true"]')) return false;
  const rect = el.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return false;
  const page = document.querySelector<HTMLElement>(".main.page");
  let top = 0;
  let bottom = window.innerHeight;
  if (page) {
    const box = page.getBoundingClientRect();
    const style = getComputedStyle(page);
    top = box.top + (parseFloat(style.scrollPaddingTop) || 0);
    bottom = box.bottom - (parseFloat(style.scrollPaddingBottom) || 0);
  }
  return rect.bottom > top && rect.top < bottom;
}

/**
 * The cover that opened `key`'s page: the one nearest the element last
 * pressed (a tile's cover, a carousel's focused cover for its Open
 * button, a list row's for Enter on the list), else the only one on the
 * page.
 */
function usedCover(key: string): HTMLElement | null {
  const sel = selector(key);
  let el: Element | null = activated?.isConnected ? activated : null;
  while (el && el !== document.body) {
    const hit = el.matches(sel) ? el : el.querySelector(sel);
    if (hit) return hit instanceof HTMLElement && onScreen(hit) ? hit : null;
    if (el.classList.contains("page-content")) break;
    el = el.parentElement;
  }
  const all = [...document.querySelectorAll<HTMLElement>(sel)].filter(onScreen);
  return all.length === 1 ? all[0] : null;
}

/** The first cover for `key` on screen (Back's landing place). */
function coverOnScreen(key: string): HTMLElement | null {
  return [...document.querySelectorAll<HTMLElement>(selector(key))].find(onScreen) ?? null;
}

function headerCover(): HTMLElement | null {
  const el = document.getElementById("page-cover");
  return el && onScreen(el) ? el : null;
}

// ----- the one morphing element -----

let named: HTMLElement | null = null;

function name(el: HTMLElement | null) {
  unname();
  if (!el) return;
  el.setAttribute(MORPH_ATTR, "");
  named = el;
}

function unname() {
  named?.removeAttribute(MORPH_ATTR);
  named = null;
}

/**
 * Development builds: no two elements may carry the same
 * `view-transition-name` (the browser would skip the transition).
 */
function assertUniqueNames(when: string) {
  if (!import.meta.env.DEV) return;
  // Before the transition starts its names are not on yet: put them on for the count.
  const root = document.documentElement;
  const was = root.dataset.vt;
  root.dataset.vt = "page";
  const seen = new Map<string, number>();
  for (const el of document.body.querySelectorAll("*")) {
    const value = getComputedStyle(el).getPropertyValue("view-transition-name");
    if (value && value !== "none") seen.set(value, (seen.get(value) ?? 0) + 1);
  }
  if (was === undefined) delete root.dataset.vt;
  else root.dataset.vt = was;
  for (const [value, count] of seen) {
    if (count > 1) console.error(`view-transition-name "${value}" on ${count} elements (${when})`);
  }
}

/** Bumped per page transition: an older one's late steps do nothing. */
let generation = 0;

/**
 * Shows a page change as a transition: `apply` makes it (the navigation
 * store publishes its state) and returns the entry shown then. `from`
 * is the page shown now, `to` the one asked for, `change` how they
 * relate: a push of `to` onto `from` morphs the cover that opened it,
 * Back to `to` from `from` morphs the header's cover back.
 */
export function pageTransition(from: Entry, to: Entry, change: PageChange, apply: () => Entry) {
  const id = ++generation;
  // A newer transition takes over: whatever an older one named goes.
  unname();
  const pushKey = change === "push" ? routeMorphKey(to.route) : null;
  const backKey = change === "back" ? routeMorphKey(from.route) : null;
  const old = pushKey ? usedCover(pushKey) : backKey ? headerCover() : null;
  name(old);
  assertUniqueNames("old page");
  let shown: Entry | null = null;
  viewTransition(
    () => {
      // Only what the new page reports from here on counts (an entry
      // shown before, now shown again, must restore its place again).
      shownId = null;
      shown = apply();
    },
    {
      kind: "page",
      settle: async () => {
        // The old cover is gone with its page (or must not be named twice).
        if (id === generation) unname();
        const page = shown;
        if (!page) return;
        const until = performance.now() + SETTLE_MS;
        const left = () => Math.max(0, until - performance.now());
        // Coalesced changes may have landed elsewhere: then no morph.
        const morphing = old !== null && page.id === to.id;
        const find = () =>
          !morphing ? null : pushKey ? headerCover() : backKey ? coverOnScreen(backKey) : null;
        // Wait for the page only for a place to restore or a cover still to come.
        if (page.scrollTop > 0 || (morphing && !find())) await whenSettled(page.id, left());
        if (id !== generation) return;
        const target = find();
        if (!target) return;
        const images = target.matches("img") ? [target] : [...target.querySelectorAll("img")];
        await decodedWithin(images, left());
        if (id !== generation) return;
        name(target);
        assertUniqueNames("new page");
      },
      onDone: () => {
        if (id === generation) unname();
      },
    },
  );
}
