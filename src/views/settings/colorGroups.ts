// The colour editor's groups and plain-word labels, by token key (the
// keys are the backend's: src-tauri/src/theme/derive.rs).

export interface ColorItem {
  key: string;
  label: string;
}

export interface ColorGroup {
  title: string;
  /** Base colours are the theme's own; the rest are computed from them. */
  base: boolean;
  items: ColorItem[];
}

const PALETTE_NAMES = [
  "Black",
  "Red",
  "Green",
  "Yellow",
  "Blue",
  "Magenta",
  "Cyan",
  "White",
  "Bright black",
  "Bright red",
  "Bright green",
  "Bright yellow",
  "Bright blue",
  "Bright magenta",
  "Bright cyan",
  "Bright white",
];

export const COLOR_GROUPS: ColorGroup[] = [
  {
    title: "Base colours",
    base: true,
    items: [
      { key: "bg", label: "Background" },
      { key: "fg", label: "Foreground (the theme's text)" },
      { key: "accent", label: "Accent" },
      { key: "danger", label: "Danger" },
      { key: "selection", label: "Text selection" },
    ],
  },
  {
    title: "Terminal palette",
    base: true,
    items: PALETTE_NAMES.map((label, n) => ({ key: `palette-${n}`, label: `${n} · ${label}` })),
  },
  {
    title: "Sidebar & panels",
    base: false,
    items: [
      { key: "surface-1", label: "Sidebar & panels" },
      { key: "surface-2", label: "Raised surfaces (cards, menus, fields)" },
      { key: "surface-3", label: "Buttons & controls" },
      { key: "border", label: "Borders & dividers" },
    ],
  },
  {
    title: "Text",
    base: false,
    items: [
      { key: "text", label: "Main text" },
      { key: "text-2", label: "Secondary text" },
      { key: "text-3", label: "Faint text (hints, numbers)" },
      { key: "accent-text", label: "Text on accent" },
      { key: "danger-text", label: "Text on danger" },
      { key: "selection-text", label: "Selected text" },
    ],
  },
  {
    title: "Highlights",
    base: false,
    items: [
      { key: "hover", label: "Row under the pointer" },
      { key: "selected", label: "Selected row" },
      { key: "accent-soft", label: "Accent tint (playing row, chips)" },
      { key: "danger-soft", label: "Danger tint" },
      { key: "success", label: "Connected" },
    ],
  },
  {
    title: "Shadows & overlays",
    base: false,
    items: [
      { key: "shadow-color", label: "Menu & dialog shadow" },
      { key: "cover-shadow-color", label: "Cover shadow" },
      { key: "shadow-strong", label: "Large cover shadow" },
      { key: "backdrop", label: "Behind dialogs" },
    ],
  },
  {
    title: "Now Playing",
    base: false,
    items: [
      { key: "np-bg", label: "Now Playing background" },
      { key: "np-text", label: "Now Playing text" },
      { key: "np-scrim", label: "Dimming over the cover" },
    ],
  },
  {
    title: "Covers & visualizer",
    base: false,
    items: [
      { key: "viz", label: "Visualizer bars" },
      { key: "on-cover", label: "Text on covers" },
      { key: "on-cover-bg", label: "Buttons on covers" },
      { key: "on-cover-bg-hover", label: "Buttons on covers, pointed at" },
      ...[1, 2, 3, 4, 5, 6].map((n) => ({ key: `cover-${n}`, label: `Cover placeholder ${n}` })),
    ],
  },
];

const LABELS = new Map(COLOR_GROUPS.flatMap((g) => g.items.map((i) => [i.key, i.label] as const)));

/** A token's label (for contrast warnings: "against Background"). */
export function colorLabel(key: string): string {
  return LABELS.get(key) ?? key;
}

/** Base colours whose "reset" means the saved theme's value (no override). */
export function isFileColor(key: string): boolean {
  return key === "bg" || key === "fg" || key.startsWith("palette-");
}

/** Tokens that may carry alpha (`#rrggbbaa`). */
export function allowsAlpha(key: string): boolean {
  return !isFileColor(key) && key !== "accent";
}
