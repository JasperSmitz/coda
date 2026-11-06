# Coda App — Design

A calm, cover-first desktop music player. It should feel like a native app, not a web page: no page scroll, no link styling, no hover-underlines, text not selectable except in inputs, the default cursor on controls, right-click context menus where they help.

## Layout

```text
┌────────────┬───────────────────────────────────────┬──────────────┐
│ Sidebar    │ Main view                             │ Queue        │
│            │                                       │ (toggle, q)  │
│ Search     │  header: title, sort, actions         │              │
│ Library    │  ─────────────────────────────        │ Now playing  │
│  Local     │  album grid / track table / artist    │ Up next ...  │
│   Artists  │  (scrolls on its own)                 │              │
│   Albums   │                                       │              │
│   Folders  │                                       │              │
│   Playlists│                                       │              │
│  Navidrome │                                       │              │
│   Artists  │                                       │              │
│   Albums   │                                       │              │
│   Playlists│                                       │              │
│ Settings   │                                       │              │
├────────────┴───────────────────────────────────────┴──────────────┤
│ Now-playing bar: cover · title/artist · ⏮ ⏯ ⏭ · seek · repeat · vol │
└────────────────────────────────────────────────────────────────────┘
```

- Sidebar ~220px, queue ~320px (collapsible), bar ~76px. Minimum window 900×600.
- The full-screen **Now Playing** view (`f`) replaces the main view: large cover, title/artist/album, timeline, visualizer bars below; background a heavily blurred, darkened copy of the cover.
- Navigation keeps a back stack (Back with `Backspace`/`Alt+←`/mouse back button): Artists → Artist (albums) → Album (tracks).

## Visual language
- Every colour is a theme token (see Theming). The default look, the built-in **Coda** theme: dark first (`#0f1012`-ish background, raised surfaces a few steps lighter); **Coda Light** for a light system appearance.
- One accent colour (Coda: warm amber, ~`#f0a83a`) for the playing track, progress, focus rings and primary buttons. Everything else is the theme's neutrals.
- Typography: system UI font stack (`system-ui, -apple-system, "Inter", "Cantarell", sans-serif`); tabular numerals for times and track numbers. Sizes: 13px body, 12px secondary, 20–28px view titles.
- Covers: rounded 6px corners, subtle shadow; album grid tiles ~180px with title and artist below; a generated placeholder (initials on a gradient of two of the theme's cover colours, picked from the album name) when there's no art.
- Track tables: #, title (with artist below when mixed), album, duration; the playing row shows an animated equaliser glyph in the accent colour. Row height 36px.
- Motion: short (120–180ms) and subtle; respect `prefers-reduced-motion`.
- Icons: inline SVG (one small icon set in `src/components/icons.tsx`), 16–20px, `currentColor`.

## Theming
Terminal-style palettes, read straight from **Ghostty theme files**, so every theme the owner already uses in Ghostty works here.

- **Sources** (merged by name, later wins; the picker shows each one's source): built-in (Coda, Coda Light, Kanagawabones, Kanso Zen, Kanso Pearl, Catppuccin Mocha, Catppuccin Latte; credits in `src-tauri/themes/THEMES-CREDITS.md`), Ghostty's installed themes, the user's `~/.config/ghostty/themes`, and the user's own `~/.config/coda-app/themes/` (custom; `$XDG_CONFIG_HOME` respected).
- **Theme file**: Ghostty's keys (`background`, `foreground`, `palette = N=#rrggbb`, `selection-*`, `cursor-*`), plus optional Coda keys Ghostty ignores: `coda-accent = #rrggbb` or `palette:N`, and `coda-<token> = #rrggbb[aa]` for any token below (`coda-surface`, `coda-border`, `coda-danger`, …).
- **Tokens**: a theme sets a few base colours: `--bg`, `--fg`, `--accent` (default palette 3, else 4/5 when 3 is too faint), `--danger` (palette 1), `--selection`, `--palette-0..15`. Everything else is derived by mixing in OKLab: surfaces 1–3 and border step from the background toward the text (light themes keep raised surfaces a touch whiter), text-2/text-3 fade toward the background, hover/selected are translucent text, accent-soft/danger-soft are translucent accent/danger, shadows are darker on dark themes, accent-text/danger-text are black or white by contrast, the Now Playing view stays dark over the blurred cover, cover placeholders are palette 1–6 darkened, the visualizer uses the accent (or palette 1→6 across the bars). `<html data-scheme="light|dark">` comes from the background's lightness.
- **Contrast**: body text ≥4.5:1 on the background and every surface, secondary text ≥4.5:1, faint text, accent, danger and success ≥3:1. Computed colours (and the colours of themes the user did not write) are nudged in OKLCH lightness until they pass; a colour the user set is never changed, it shows a small contrast warning instead.
- **No colour literals outside the token layer**: components and stylesheets use `var(--…)` only; the hex values live in `src/styles/tokens.css` (the built-in Coda themes, as fallbacks) and in theme files. `pnpm build` fails otherwise (`scripts/check-colors.mjs`).
- **Settings → Appearance**: mode (one theme; light + dark following the system; follow Ghostty's `theme = …`, incl. `light:A,dark:B`, re-read when the window regains focus), a searchable list of every theme with a swatch strip (background, text, accent, palette 1–6) and a source badge; moving through it previews live, Enter/click applies, Escape reverts. An accent picker (palette swatches, custom hex, reset to the theme's), the visualizer's colours, "Reveal themes folder", and Duplicate / Rename / Delete (custom themes; delete asks first). Saved in the GUI's own `~/.config/coda-app/settings.toml`, not Coda's `config.toml`.
- **Customise colours**: every base colour and derived token, grouped in plain words ("Sidebar & panels", "Secondary text", …), each with a swatch, hex field, colour picker and Reset; derived ones show their computed value until set. Edits preview live. Saving writes a custom theme (default "<Theme> (custom)", renameable) as a Ghostty file with `coda-*` keys for the overrides, and selects it; built-in and Ghostty themes are never changed.
- The chosen theme is applied **before the first paint** (the backend injects its tokens when it creates the window), so there is no flash of the default theme.

## Interaction
- Double-click or Enter on a track plays its context (album/playlist/list) from that track. Context menu: Play, Play next, Add to queue, Go to album, Go to artist, Add to playlist.
- Keyboard (global, unless typing in an input): `Space` play/pause, `n`/`p` or `Ctrl+→/←` next/previous, `←/→` seek ∓5s, `+`/`-` volume, `r` repeat, `s` shuffle context, `q` toggle queue, `f` Now Playing, `/` or `Ctrl+F` search, `Esc` close overlay/back.
- Errors and notices (player messages, Navidrome failures) appear as small toasts bottom-right above the bar; persistent problems (mpv missing, Navidrome unreachable) as a banner in the main view.
