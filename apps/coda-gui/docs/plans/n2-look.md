# Brief N2 — The look: tokens, type, glass; themes retired (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Principles", "Colour" (base and house accent only; music colours are N3), "Type", "Shape and surfaces", "Preferences". ADR 0009 (what themes leave behind). Builds on N1 (the top bar, roots, the queue panel, Settings' "Music source" section).

## Goal
Every existing view in the 2.0 look, on one fixed token set, with bundled fonts and glass where things float; the theme system removed; Settings becomes **Preferences** with the accent choice. The views keep their structure (Library sorts/lists, page headers, Home, the floating player are later slices); this slice changes how everything looks, not what it does.

## Scope

### 1. Tokens (`src/styles/tokens.css`)
- One `:root` block, dark by design (`color-scheme: dark`), no `prefers-color-scheme` block. Keep the existing token **names** that views use (`--bg`, `--surface-1..3`, `--border`, `--text`, `--text-2`, `--text-3`, `--accent`, `--accent-text`, `--accent-soft`, `--danger*`, `--success`, `--hover`, `--selected`, `--selection*`, shadows, `--backdrop`, `--on-cover*`, `--cover-1..6`, `--viz`) but give them 2.0 values, so views follow without rewrites:
  - `--bg` the base, a dim slightly violet-tinted near-black (~`hsl(256 14% 7%)`); `--text` a warm white (~`#f6f3fb`), `--text-2` ~68% and `--text-3` ~44% of it (as solid or translucent colours, whichever keeps contrast rules); surfaces become **glass**: translucent white over the base (`--surface-1` ~5.5%, `--surface-2` ~8%, `--surface-3` ~11%), `--border` a 9% white hairline; `--hover`/`--selected` translucent white.
  - `--accent` the **house accent** (Coda amber ~`#f0a83a`) unless the listener chose another (§4); `--accent-text` black or white by contrast (computed in the backend for a custom accent); `--accent-soft` the accent at low alpha.
  - The Now Playing `--np-*` tokens become aliases of the base tokens where they now match (Now Playing was always dark); keep the ones Now Playing still needs (scrim, panel) with 2.0 values.
  - `--cover-1..6` (placeholders) a house set of deep jewel tones.
  - New: `--glass-blur` (e.g. `blur(28px) saturate(1.5)`), `--radius-*` per DESIGN.md (covers 14–18px, cards/panels 22–30px, pills `999px`), `--font-display`, `--font-ui`, `--spring` (cubic-bezier(.34,1.56,.64,1)), and a type scale (body 14.5px, small 13px, section 30px, title clamp). Remove tokens nothing uses any more (`--palette-*`, `--sidebar-width`, `--queue-width` if N1 left them, …).
- Where `backdrop-filter` is unsupported, glass must stay legible: use `@supports not (backdrop-filter: blur(1px))` (and `-webkit-` for WebKitGTK) to swap in more opaque surfaces.
- `scripts/check-colors.mjs` keeps working (update its allowlist if it names token files/keys that change).

### 2. Type
- Bundle **Fraunces** (variable, with italic; axes opsz, wght, SOFT, WONK) and **Figtree** (variable) as font files in the frontend (e.g. `src/assets/fonts/`), with their `OFL.txt` licences, via `@font-face` (`font-display: swap`). Source: the `google/fonts` GitHub repository (`ofl/fraunces/`, `ofl/figtree/`) — download the variable TTFs (or woff2 if available there) with curl; check the files are fonts (`file`), note sizes. Never load fonts from the web at runtime; the CSP stays strict.
- `--font-display`: Fraunces with `font-variation-settings: "SOFT" 100, "WONK" 1`, weights ~620–640, for page titles, section headings, album/genre/mix names in headers, card titles where DESIGN.md says so, the wordmark; `--font-ui`: Figtree for everything else. Small labels above titles ("eyebrows") become the display face in italic ~15px, sentence case; remove tracked-out uppercase label styles.
- Tabular numerals stay for times and counts.

### 3. Surfaces and components
- Glass (`--surface-*` + `--border` + `--glass-blur`) for things that float: the top bar's pill and buttons, the queue panel, the docked 1.x bar (a glass strip for now; N4 replaces it), context menus, dialogs, toasts, the banner, Now Playing's panels.
- Buttons per DESIGN.md: primary = accent pill with `--accent-text`; secondary = glass pill; icon buttons round. Pressed state scales slightly with `--spring` (reduced motion: none).
- Covers: larger radii, soft long shadows. Track tables and lists: keep their layout, new colours and fonts; row hover/selected use the translucent tokens; the playing row in the accent.
- Fields (search, server fields, filters): rounded, glass, accent focus ring. Focus stays visible everywhere.
- Now Playing: switch to the base tokens and fonts (title in the display face); its layout and motion are unchanged.
- Onboarding: the same tokens and fonts (its layout unchanged).

### 4. Themes retired, Preferences (ADR 0009)
- **Backend**: remove the theme system: theme sources, Ghostty loading/following, parse, derive, custom theme files, the editor's commands, `theme_list`/`theme_resolve`/`theme_save_custom`/`theme_rename`/`theme_duplicate`/`theme_delete`/`theme_reveal_folder`/`appearance_*` as they are, their capabilities, `src-tauri/themes/` and the credits file, the theme tests. **Keep and move** out of `theme/`: the prefs file (`settings.toml`: `source`, `lyrics_index`, and a new `accent`), its atomic/symlink-preserving writer (`fsio`), `ui_source_get/set`, and the colour maths needed for contrast (`color.rs` subset: parsing hex, relative luminance/contrast, OKLab/OKLCH if used by the contrast nudge) for N3 to reuse. A suggested home: `src-tauri/src/prefs.rs` (+ `prefs/`), and `src-tauri/src/palette/color.rs` (N3 grows `palette/`). Old theme keys in `settings.toml` are ignored on read (serde default + ignore unknown); a write drops them (ADR 0009 accepts this).
- `accent`: `"house"` (default until N3 adds `"music"` and makes it the default) or `#rrggbb`. Commands: `prefs_get` → `{accent, accentText, source, lyricsIndex, …}` and `prefs_set_accent{accent}` (validates hex; computes `accentText` by contrast). The chosen accent applies **before first paint** (keep a small initialization script that sets `--accent`, `--accent-text`, `--accent-soft` on `<html>` when it differs from the house accent) and live on change.
- The native window and webview background: the base colour (a Rust constant matching `--bg`; one comment in each place pointing at the other).
- **Frontend**: remove `views/settings/` (Appearance, ThemeList, ColorEditor, colorGroups), `state/theme.ts`'s theme switching (keep a tiny accent store if useful), `lib/theme.ts` helpers that only served themes (keep `normalizeHex` if the custom accent field uses it), `styles/appearance.css`, the visualizer's palette mode (`data-visualizer`; the visualizer uses `--viz`/the accent).
- **Settings → Preferences**: the view and its entry are titled "Preferences" (page title "Make it yours", eyebrow "Preferences"). Sections as glass cards in this order: **Accent** (six curated swatches including the house amber — amber, rose ~`#ff7d8e`, mint ~`#7fd8c4`, periwinkle ~`#8ea9ff`, lilac ~`#c99cff`, bone ~`#e9e3d2` — plus a custom colour: hex field + native colour picker; a small contrast note when a custom accent is under 3:1 on the base; changes apply live and persist), **Music source** (from N1), then the existing Navidrome, music folder, playback/cover art/scrobbling and lyrics-search sections, restyled. The swatch values live in `tokens.css` (as tokens) so `check:colors` stays clean, or in the backend if they must be validated there; decide and say which.

## Out of scope
Music colours, "From the music", glows (N3); the floating player (N4); Library sorts, Covers/List, page headers (N5); Home (N6); motion beyond the press feedback (N7). No change to coda-core.

## Acceptance
- No theme code, theme files, theme commands or capabilities remain; the app starts with an old `settings.toml` that has theme keys (test it in the prefs tests) and keeps `source` and `lyrics_index`.
- The accent choice persists across launches and applies before the first paint (no flash of amber when a custom accent is set).
- Every view renders in the new tokens and fonts; text contrast ≥4.5:1 on the base and on each glass surface (state the computed ratios in the report for `--text`, `--text-2`, `--text-3`, the house accent and the six swatches against `--bg`); legible without `backdrop-filter`.
- Checks (report the commands run): `pnpm build`, `pnpm check:colors` (from `apps/coda-gui`); `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo clippy -p coda-app -p coda-core --all-targets --all-features -- -D warnings`, `cargo test -p coda-app -p coda-core` (repo root).
- Docs: `docs/ARCHITECTURE.md` (theme module gone; prefs and accent; fonts), `docs/DESIGN.md` (mark Type, Shape and surfaces, Preferences, the base/house parts of Colour *(shipped)* as true), `apps/coda-gui/README.md` (themes and Ghostty mentions), `scripts/smoke.md` (a "2.0 N2 — Look" section; retire the theme smoke items or mark them 1.x only), the font licences.
- Do not commit; report changed files, decisions, checks run, contrast numbers, and anything open.
