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
- Dark first (`#0f1012`-ish background, raised surfaces a few steps lighter), light theme from the same tokens via `prefers-color-scheme`.
- One accent colour (warm amber, ~`#f0a83a`) for the playing track, progress, focus rings and primary buttons. Everything else is neutral greys.
- Typography: system UI font stack (`system-ui, -apple-system, "Inter", "Cantarell", sans-serif`); tabular numerals for times and track numbers. Sizes: 13px body, 12px secondary, 20–28px view titles.
- Covers: rounded 6px corners, subtle shadow; album grid tiles ~180px with title and artist below; a neutral generated placeholder (initials on a gradient derived from the album name) when there's no art.
- Track tables: #, title (with artist below when mixed), album, duration; the playing row shows an animated equaliser glyph in the accent colour. Row height 36px.
- Motion: short (120–180ms) and subtle; respect `prefers-reduced-motion`.
- Icons: inline SVG (one small icon set in `src/components/icons.tsx`), 16–20px, `currentColor`.

## Interaction
- Double-click or Enter on a track plays its context (album/playlist/list) from that track. Context menu: Play, Play next, Add to queue, Go to album, Go to artist, Add to playlist.
- Keyboard (global, unless typing in an input): `Space` play/pause, `n`/`p` or `Ctrl+→/←` next/previous, `←/→` seek ∓5s, `+`/`-` volume, `r` repeat, `s` shuffle context, `q` toggle queue, `f` Now Playing, `/` or `Ctrl+F` search, `Esc` close overlay/back.
- Errors and notices (player messages, Navidrome failures) appear as small toasts bottom-right above the bar; persistent problems (mpv missing, Navidrome unreachable) as a banner in the main view.
