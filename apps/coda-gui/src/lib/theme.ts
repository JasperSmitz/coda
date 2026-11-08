// Theme helpers (pure): applying a resolved theme's tokens to <html>,
// hex input, the picker's search and custom theme names. The colours
// themselves are computed by the backend (src-tauri/src/theme/).

import type { ResolvedTheme, ThemeSummary, VisualizerColors } from "./types";

/** Fired on `window` after tokens change (the visualizer re-reads them). */
export const THEME_EVENT = "coda-theme";

/** Sets every token of `theme` on `root` as `--key`, plus the scheme. */
export function applyTokens(root: HTMLElement, theme: ResolvedTheme, visualizer: VisualizerColors): void {
  for (const token of theme.tokens) root.style.setProperty(`--${token.key}`, token.value);
  root.style.colorScheme = theme.scheme;
  root.dataset.scheme = theme.scheme;
  root.dataset.visualizer = visualizer;
  window.dispatchEvent(new Event(THEME_EVENT));
}

/** The token's value, or `""`. */
export function tokenValue(theme: ResolvedTheme, key: string): string {
  return theme.tokens.find((t) => t.key === key)?.value ?? "";
}

/**
 * `#rrggbb` (or `#rrggbbaa` when `alpha`) from what was typed: `#` is
 * optional, `#rgb` expands. `null` if it is not a hex colour.
 */
export function normalizeHex(text: string, alpha = false): string | null {
  let hex = text.trim().replace(/^#/, "").toLowerCase();
  if (!/^[0-9a-f]+$/.test(hex)) return null;
  if (hex.length === 3) hex = [...hex].map((c) => c + c).join("");
  if (hex.length === 6 || (alpha && hex.length === 8)) return `#${hex}`;
  return null;
}

/** The `#rrggbb` part (what `<input type=color>` takes). */
export function opaquePart(hex: string): string {
  return hex.slice(0, 7);
}

/** `rgb` (`#rrggbb`) with the alpha of `previous`, if it had one. */
export function keepAlpha(rgb: string, previous: string): string {
  return previous.length === 9 ? rgb + previous.slice(7) : rgb;
}

/** Themes whose name contains every word of `query` (any case). */
export function filterThemes(themes: ThemeSummary[], query: string): ThemeSummary[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return themes;
  return themes.filter((t) => {
    const name = t.name.toLowerCase();
    return words.every((w) => name.includes(w));
  });
}

/** `"<base> (custom)"`, numbered if that name is taken. */
export function customName(base: string, taken: Iterable<string>): string {
  const names = new Set(taken);
  const stem = base.replace(/ \(custom(?: \d+)?\)$/, "");
  let name = `${stem} (custom)`;
  for (let n = 2; names.has(name); n++) name = `${stem} (custom ${n})`;
  return name;
}

export const SOURCE_LABEL = { builtIn: "Built-in", ghostty: "Ghostty", custom: "Custom" } as const;
