# Brief: colour themes from terminal palettes

The owner wants terminal-style colour palettes ("Kanagawabones", "Kanso Zen", "Catppuccin"...). Approach: read **Ghostty theme files** directly, so every terminal theme the owner already has works, and derive the whole UI from them.

Read first: `AGENTS.md`, `docs/DESIGN.md`, `docs/ARCHITECTURE.md`, `src/styles/tokens.css`. Nothing else is being built in parallel; work in the main checkout.

## Theme format and sources
- Ghostty format: `key = value` lines; `palette = N=#rrggbb` (0–15), `background`, `foreground`, `cursor-color`, `selection-background`, `selection-foreground`; `#` comments. Parse strictly and defensively (untrusted files): only hex colours, cap file size (e.g. 64 KiB) and theme count, ignore unknown keys, skip invalid files with a log line.
- Optional Coda extension keys in the same file (ignored by Ghostty): `coda-accent = #rrggbb` or `coda-accent = palette:4`, and optional overrides `coda-surface`, `coda-border`, `coda-danger`.
- Sources, merged by name (later wins): (1) **built-in** themes compiled into the app; (2) Ghostty's installed themes if present: macOS `/Applications/Ghostty.app/Contents/Resources/ghostty/themes`, Linux `/usr/share/ghostty/themes` (and `$GHOSTTY_RESOURCES_DIR/themes` when set), plus the user's `~/.config/ghostty/themes`; (3) the user's `~/.config/coda-app/themes/` (XDG_CONFIG_HOME aware). Each entry records its source for the picker.
- Built-in set (copy the Ghostty files' colours, keep a `THEMES-CREDITS.md` with each theme's origin/licence): **Coda** (the current dark amber look, expressed as a theme) and **Coda Light** (current light look), **Kanagawabones** (accent `#e5c283`), **Kanso Zen** (accent `#8ba4b0`), **Kanso Pearl** (accent `#4d699b`), **Catppuccin Mocha** (accent `#cba6f7`, mauve), **Catppuccin Latte** (accent `#8839ef`). For themes without a `coda-accent`, default to palette 3 (yellow), falling back to 4/5 when that fails the contrast rule below.

## Mapping to tokens
- Every colour in the UI comes from tokens. First move the ~20 hard-coded colours outside `tokens.css` (Now Playing overlay/backdrop, Cover placeholder gradients, Visualizer, browse/app/edit styles) into tokens.
- Derive tokens from the theme with `color-mix(in oklab, …)` in CSS where possible, so a theme only sets a few base variables: `--bg`, `--fg`, `--accent`, `--danger` (palette 1 unless overridden), `--selection`, and `--palette-0..15`. Surfaces 1–3 = bg mixed toward fg (light themes: toward fg slightly, or toward white — pick what looks right), border, text-2/text-3 = fg mixed toward bg, hover/selected overlays, accent-soft, danger-soft, shadows (darker for dark themes), accent-text = black or white by contrast.
- Light vs dark is detected from the background's luminance and exposed as `data-scheme="light|dark"` so shadows/overlays can differ.
- Contrast guard: if the accent's contrast against bg is < 3:1, adjust its lightness in oklch until it passes (do this in Rust or TS when the theme is loaded, not per frame). Body text vs surfaces should stay ≥ 4.5:1; if a theme fails, nudge `--text` slightly.
- Visualizer bars: accent by default; option "palette gradient" (palette 1→6 across bars). Cover placeholders: gradients from palette colours.

## Settings and persistence
- GUI-only preferences file (not Coda's shared `config.toml`): e.g. `~/.config/coda-app/settings.toml` (atomic write). Stores: mode (`single` theme name | `system` light+dark pair | `ghostty`), the theme name(s), accent override per theme (optional), visualizer colour mode.
- `ghostty` mode reads the `theme = ...` line from Ghostty's config (`~/.config/ghostty/config`, macOS also `~/Library/Application Support/com.mitchellh.ghostty/config`), supporting `theme = Name` and `theme = light:A,dark:B`; follows it on start and when the window regains focus.
- `system` mode follows `prefers-color-scheme` live.
- Settings view gains an **Appearance** section: mode selector; a searchable theme list (virtualized; ~800 entries) with a swatch strip per theme (bg, fg, accent, palette 1–6) and source badge; **live preview** while moving through the list (arrow keys / hover), applied on Enter/click, reverted on Escape; accent picker (palette swatches + "custom" hex input + "reset to theme default"); visualizer colour mode. Persist on apply.
- The theme must be applied **before first paint** (no flash of the default theme): the backend provides the resolved theme with the initial snapshot/command, or inject it at window creation.
- New Tauri commands go in `build.rs` AppManifest and `capabilities/default.json`.

## Acceptance
- All checks in AGENTS.md pass; Rust tests for the parser (valid, malformed, oversized, overrides, palette refs), source merging, Ghostty config `theme` parsing (incl. light:/dark:), and the contrast adjustment; TS logic tested if a test runner exists, else keep it in small pure functions.
- No window opened; add smoke steps. Update `docs/DESIGN.md` (theming section) and `docs/ARCHITECTURE.md`. Don't commit. Report to `/tmp/coda-app-themes-report.md`.
