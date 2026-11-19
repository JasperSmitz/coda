# Coda App — Plan

Status: **1.0.0 (Classic) released** (tag `coda-gui-v1.0.0`, 2026-11-19): M0–M4, lyrics, genres and mixes; frozen except critical fixes. **2.0 (Neo-Coda)** is in progress: the redesign in `DESIGN.md`, ADR 0009, slices under "2.0" below.

## Stack (decided)
- **Tauri 2** desktop shell (system webview: WebKitGTK 4.1 on Linux, WKWebView on macOS).
- **Rust backend** in `src-tauri/`, on Coda's shared engine `coda-core` (`crates/coda-core` at the monorepo root, also used by the terminal player).
- **React 19 + TypeScript + Vite** frontend in `src/`.
- Playback stays **mpv via Coda's IPC backend**; mpv is looked up on `PATH`, then in `/opt/homebrew/bin`, `/usr/local/bin` and `/usr/bin` (the TUI uses `PATH` only).

## Architecture (proposed)

```text
React UI ──invoke(intent)──▶ Tauri command ──mpsc──▶ Engine actor (tokio task)
   ▲                                                   │ Player + resolve::dispatch
   └──── events: snapshot / position / visualizer ◀────┤ PlaybackHandle (mpv)
                                                       │ navidrome::Client, Library
                                                       └ session save (debounced)
```

- `engine.rs`: the actor; `select!` over intents, `PlaybackEvent`s, scan events, timers.
- `dto.rs`: serde types for the UI (`Snapshot`, `QueueItem`, `TrackDto`, `Progress`, ...).
- `commands.rs`: thin Tauri commands that forward intents.
- Later: cover art served through a custom `coda://cover/...` URI scheme (bytes from Coda's cover loaders and disk cache), so the webview gets real images without data URLs.

## Milestones

### M0 — Engine vendored (done; since the monorepo migration the shared `crates/coda-core` at the repository root)
- `coda-core`: Coda's UI-free modules without `app`/`setup`/`ui`/`main`, plus `Player::seek_absolute`, public `Player::set_volume`, `logging`, `setup_actions` (Navidrome save/ping) and a config bootstrap. 532 unit + integration tests pass.

### M1 — A window that plays the queue (done)
- Backend: config load, logging, Navidrome client, mpv backend, session restore (never auto-plays), engine actor, debounced session save, final save + mpv shutdown on quit.
- UI: now-playing bar (title/artist/album, play/pause, previous/next, seek bar, volume slider, repeat), queue list (current track highlighted, double-click/Enter plays an entry, remove), status/notice line for player messages and errors (mpv missing, no config).
- Keyboard: space, n/p (or arrows), +/-, r.
- Acceptance: launches with the TUI's existing config and session; controls work for local and Navidrome tracks; gapless still works; quitting leaves no mpv process.

### M2 — Browse and play (done)
- Local library (scan progress; artists → albums → tracks; playlists; folders) and Navidrome (artists, albums with the six sorts, playlists), search.
- Play in context, shuffle, enqueue/enqueue next, shuffle all.
- Cover art (current track + album grids) via the URI scheme and Coda's disk cache.
- Scrobbling.

### M3 — Queue and playlists (done)
- Drag-and-drop queue reordering, multi-select, clear, shuffle upcoming.
- Navidrome playlist create/rename/reorder/delete/save queue as playlist, with the hidden-entries confirmation.

### M4 — Polish and platform (done except packaging/CI, see `docs/plans/platform.md`)
- Onboarding and settings (Navidrome connect/disconnect, music folder) in the GUI.
- Full-window now-playing view with large cover and visualizer.
- Media keys / MPRIS on Linux, Now Playing on macOS.
- Packaging: AppImage/`.deb` and an Arch `PKGBUILD`; macOS `.dmg`. CI on Linux and macOS.

### Lyrics mode (2026-11-12, `docs/plans/lyrics.md`)
- Owner-approved scope change (VISION.md): lyrics in Now Playing, from the file's tags or a sidecar `.lrc` and from Navidrome. Slice 1 (core): `coda_core::lyrics` and `Client::lyrics`. Slice 2 (GUI): `now_playing_lyrics`, `useLyrics`, the lyrics-mode layout and `l`. Runtime checks: `scripts/smoke.md` "Lyrics mode".
- Not wanted: word-level highlighting, ID3 `SYLT`, editing lyrics, online providers, choosing a language, a TUI lyrics screen, remembering the mode across launches.

### Lyrics search (2026-11-12, `docs/plans/lyrics-search.md`)
- Slice 1 (local): `coda_core::lyrics::index`, lyrics from the scan, `local_lyrics_search`, the search page's Lyrics section. Slice 2 (Navidrome): `coda_core::lyrics::cache` (a per-server cache file and its background sync; ADR 0007), `navidrome_lyrics_search`, the `lyrics_index_*` commands and the Settings › Navidrome "Lyrics search" block. Runtime checks: `scripts/smoke.md` "Lyrics search" (LS1–LS10).

### Genres (2026-11-15, `docs/plans/genres.md`, M11 slice 2)
- Owner-approved (VISION.md "Direction"): a Genres sidebar entry for both sources after Albums, the genre list (A–Z, album and track counts), a genre page with its albums and Play/Shuffle (`s` shuffles the genre), and an album's genres as links on its page. Backend: `local_genres`, `local_genre`, `navidrome_genres`, `navidrome_genre_albums`, `navidrome_genre_random_songs`, `GenreDto`, `LocalContextDto::Genre`; `TrackDto`/`TrackRef` carry genres, so Navidrome tracks keep them in the queue. Navidrome genre pages offer Shuffle only (the server gives random songs of a genre; playing a whole genre in order would need `getSongsByGenre`) and list their albums in the server's order; an album's genre links are resolved against the server's genre listing so they open its exact value. Runtime checks: `scripts/smoke.md` "Genres" (G1–G9).
- Not in it: genres in search, the play history or Now Playing; combining sources (Mixes).

### Mixes (2026-11-16, `docs/plans/mixes.md`, M12 slice 2)
- Part 2a (backend, reviewed): the engine plays mixes (`playMix`, `playQuickMix`, local and Navidrome top-ups, session restore); the mixes and editor-search commands; `snapshot.mix`.
- Part 2b (screens, done): a **Mixes** sidebar entry per source after Playlists (cards with ingredient chips, Play, Edit…/Delete… in the card menu, **New mix…**, a one-line empty state; the playing mix's card marked); the mix editor dialog (name, one search over genres, artists and albums of the source, chips, Play as a quick mix, Save, typed errors inline); **Add to mix…** in the genre and artist rows' menus, the album menu and the genre, artist and album page headers; "Mix: <name>" / "Quick mix: <name>" with a loading state in Now Playing and the queue panel. Pure helpers in `lib/mixRecipe.ts` (`scripts/mixes.test.mjs`). Runtime checks: `scripts/smoke.md` "Mixes" (M1–M14).
- Not in it: mixes on Home or in the play history, multi-select to build a mix, exclusions and weights, combining sources.

### 1.0.0 — Classic (released 2026-11-19)
- The design above, frozen (ADR 0009). Critical fixes only, on a `gui-1.x` branch created from `coda-gui-v1.0.0` when the first one is needed, released as `coda-gui-v1.0.x`. 1.x's design document stays at the tag.

## 2.0 — Neo-Coda (in progress; ADR 0009, `DESIGN.md`)

A design-led redesign on `main`: Home / Search / Library navigation, Home built around a carousel of recent listening and daily mood mixes, a floating player with the queue rising out of it, colour taken from the music, one look with no themes. Each slice is a vertical slice that leaves the app working and releasable from `main`; each gets a brief in `docs/plans/` (`n1-shell.md`, …) before implementation, a review, and runtime checks in `scripts/smoke.md`. The engine, commands, mixes, lyrics, history and Now Playing carry over.

### N1 — Shell and navigation (done 2026-11-19, `docs/plans/n1-shell.md`; macOS button placement awaits smoke N1-1)
- The top bar (coda sign and wordmark, the Home / Search / Library pill with its gliding indicator, Preferences) and the macOS window buttons in its row; the sidebar and the queue column go. Library as one root with sections (Albums, Artists, Genres, Playlists, Mixes, Folders for local), the section remembered; a back stack per root (and per source, as today). The source switch moves to Preferences (`Ctrl/Cmd+1`/`2` stay). The 1.x bar stays for now, docked; the queue opens as a temporary overlay until N4.
- Acceptance: every 1.x view is reachable; Back, keyboard shortcuts and "Go to album/artist" across sources still work; focus is visible everywhere; nothing hides under the bar.

### N2 — The look: tokens, type, glass; themes retired (done 2026-11-19, `docs/plans/n2-look.md`; reviewed; runtime smoke N2-1–N2-12 for the owner)
- The 2.0 token set (base, ink levels, glass, line, shadows, house accent; `tokens.css`, `check:colors` kept), bundled fonts (provisional pair; licences shipped), shapes and buttons per DESIGN.md, applied to every existing view. Preferences page (Accent: house colours and custom; Music source; Lyrics search; the server and folder settings) replaces Settings; the Library default view comes with Covers / List in N5. Theme removal per ADR 0009 (backend theme module except the prefs file and the colour maths, built-in themes, Ghostty loading, the editor and their commands and capabilities); `settings.toml` gains `accent`.
- Acceptance: no theme code or command left; the prefs file keeps source and lyrics-index; the accent choice applies live and persists; contrast rules hold on base and glass; Linux (WebKitGTK) and macOS both legible with and without `backdrop-filter`.

### N3 — Colours from the music (done 2026-11-19, `docs/plans/n3-music-colours.md`; reviewed; runtime smoke N3-1–N3-10 for the owner)
- A per-cover palette (tint, accent, accent ink, glow) with contrast limits, computed off the engine actor when a cover is loaded and cached with it (proposed: `coda_core::cover::palette`, pure and tested; the GUI exposes it per cover token). Scoped colours: player/queue/Now Playing follow the playing cover; a page follows what it shows (`#view`), with its glow; grid tiles and mood cards their album's accent; registered properties for the slow blends, with an instant fallback.
- Acceptance: palettes are deterministic and tested (contrast on base and own tint, greyscale and near-black covers, missing covers); no colour change blocks rendering; a fixed accent overrides everywhere; reduced motion switches instantly.

### N4 — Player, queue and Now Playing (done 2026-11-19, `docs/plans/n4-player.md`; reviewed; runtime smoke N4-1–N4-13 for the owner)
- The floating player (cover background, the small visualizer, links, repeat, volume), the queue rising out of it (anchored to its right edge; Esc / `q` / click elsewhere closes; all 1.x queue features), the player's cover morphing into Now Playing's and back; Now Playing on the 2.0 tokens (its side-panel queue unchanged).
- Acceptance: every 1.x player and queue behaviour (selection, drag-and-drop, header buttons, mix line) works in the new placement; the morph degrades to a crossfade without View Transitions; the 1.x Now Playing smoke checks still pass.

### N5 — Library, pages and Search
- Sections with Covers / List per section (remembered; the Preferences default), sorts (Albums: A–Z, Most played, Recently added, Year; Navidrome's server sorts); page headers for album, artist, genre, playlist and mix with their glows; the artist page's **Your most played**; Search's large field, genre chips, Top result + Songs, Albums, Artists, Lyrics (1.x rules). **Pages scroll under the top bar** (from N1: views still have a fixed header with a scrolling body below the bar; the page becomes one scroll container whose header scrolls away, with the top bar floating over it).
- Open for the brief: local per-song play counts (the history counts listens per context today) and how Navidrome's per-song `playCount` is gathered for an artist without fetching every album at once.
- Acceptance: large libraries stay fast in List (virtualized); sorts match the source's counts; Search keeps the lyrics behaviour.

### N6 — Home
- **Where you left off**: the carousel from the play history (contexts with covers, newest first), the meta line, **Resume** (the history records where a context stopped: the entry and position, per context), Open, the page glow following the focus; keyboard and trackpad. **Moods for today**: three genres from the history, three albums each (not played for longest; never this week), the same all day and cached per source and day (the daily-picks cache's successor), **Other moods**, Play as a quick mix (ADR 0008). The empty state for a new listener. The 1.x Home rows (Recently added, Most played, Daily Picks) go.
- Open for the brief: where genre-per-album comes from for the history's contexts (local tags; Navidrome album genres), and the exact pick and fallback rules (thin history, a genre with fewer than three albums).
- Acceptance: the picks are deterministic per day and tested; Home renders before history and picks arrive (no layout jump); Resume continues the right track; nothing blocks the engine.

### N7 — Motion and refinement
- Cover morphs between pages, page crossfades, the carousel's glide, the queue's rise; reduced motion everywhere. The type exploration (a few pairings compared live, then one chosen), the flourish decision (the record, or none), the owner's nitpick round, the onboarding restyle, release notes (themes gone, ADR 0009) and **2.0.0**.
- Acceptance: the full `scripts/smoke.md` for 2.0 passes on Linux and macOS; docs (`DESIGN.md` marked shipped, `ARCHITECTURE.md`, `README.md`) match the code.

### Later
- Drag tracks/albums from browse views onto the queue.
- Letter-key jump in the artist list; Navidrome album headers with year when opened via "Go to album".
- Fixes made in Coda's engine don't flow here automatically; port them by hand when they matter.
