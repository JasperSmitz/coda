// Music colours on an element (DESIGN.md "Colour"), pure helpers: which
// tokens a colour scope (the player, the queue panel, Now Playing, the page)
// sets for a cover's palette, and writing them. The palettes come from
// the backend (`cover_palettes`, through state/palettes.ts); the scope
// hook is `useColourScope` there.
//
// A scope always takes the cover's tint (and its glow image, drawn by
// the components). Its accent tokens follow the cover only with "From the
// music" and a colourful cover; otherwise they are left to inherit (the
// house accent, or the listener's fixed accent on <html>). The tokens are
// registered with `@property` in tokens.css and transition on
// `.colour-scope` elements, so a change blends.

import type { Palette } from "./types";

/** The `prefs.accent` value for "From the music" (the default). */
export const MUSIC = "music";

/** Fired on `window` after a scope's colours change (the visualizer re-reads them). */
export const PALETTE_EVENT = "coda-palette";

/** The class that makes an element's colour tokens transition (tokens.css). */
export const SCOPE_CLASS = "colour-scope";

export const SCOPE_TOKENS = [
  "--tint",
  "--accent",
  "--accent-text",
  "--accent-soft",
  "--selection",
  "--glow",
  "--viz",
] as const;
export type ScopeToken = (typeof SCOPE_TOKENS)[number];

/** `#rrggbb` at `alpha` (0–1) as `#rrggbbaa`. */
export function withAlpha(hex: string, alpha: number): string {
  const byte = Math.round(Math.min(1, Math.max(0, alpha)) * 255);
  return `${hex.slice(0, 7)}${byte.toString(16).padStart(2, "0")}`;
}

/**
 * The tokens a scope sets for `palette` (`null`: none, inherit). The
 * accent tokens only when `musicAccent` (the listener chose "From the
 * music") and the cover is colourful. `--accent-soft` and `--selection`
 * mirror their definitions in tokens.css (16% and 34% of the accent).
 */
export function scopeTokens(palette: Palette | null, musicAccent: boolean): Record<ScopeToken, string | null> {
  const accent = palette && musicAccent && palette.vivid ? palette : null;
  return {
    "--tint": palette?.tint ?? null,
    "--accent": accent?.accent ?? null,
    "--accent-text": accent?.accentText ?? null,
    "--accent-soft": accent ? withAlpha(accent.accent, 0.16) : null,
    "--selection": accent ? withAlpha(accent.accent, 0.34) : null,
    "--glow": accent?.glow ?? null,
    "--viz": accent?.accent ?? null,
  };
}

/** Writes `tokens` on `element` (inline), marks it a scope; tells listeners if anything changed. */
export function applyScope(element: HTMLElement, tokens: Record<ScopeToken, string | null>): void {
  element.classList.add(SCOPE_CLASS);
  let changed = false;
  for (const name of SCOPE_TOKENS) {
    const value = tokens[name];
    const was = element.style.getPropertyValue(name);
    if (value === null) {
      if (was !== "") {
        element.style.removeProperty(name);
        changed = true;
      }
    } else if (was !== value) {
      element.style.setProperty(name, value);
      changed = true;
    }
  }
  if (changed) window.dispatchEvent(new Event(PALETTE_EVENT));
}
