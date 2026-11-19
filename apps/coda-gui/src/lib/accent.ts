// Accent helpers (pure). The colours and their contrast are computed by
// the backend (src-tauri/src/prefs.rs); this only reads what was typed.

/** Fired on `window` after the accent changes (the visualizer re-reads it). */
export const ACCENT_EVENT = "coda-accent";

/** What Preferences saves for the house accent. */
export const HOUSE = "house";

/** Below this contrast with the base, Preferences notes a custom accent is hard to see. */
export const MIN_ACCENT_CONTRAST = 3;

/**
 * `#rrggbb` from what was typed: `#` is optional, `#rgb` expands. `null`
 * if it is not a hex colour.
 */
export function normalizeHex(text: string): string | null {
  let hex = text.trim().replace(/^#/, "").toLowerCase();
  if (!/^[0-9a-f]+$/.test(hex)) return null;
  if (hex.length === 3) hex = [...hex].map((c) => c + c).join("");
  return hex.length === 6 ? `#${hex}` : null;
}

/**
 * Whether black text reads better than white on `hex` (`#rrggbb`), by
 * WCAG contrast: the backend's rule, used only to show a choice before
 * its answer arrives.
 */
export function prefersDarkInk(hex: string): boolean {
  const channel = (i: number) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  // Contrast with black (l + 0.05) / 0.05 against white 1.05 / (l + 0.05).
  return (l + 0.05) / 0.05 >= 1.05 / (l + 0.05);
}
