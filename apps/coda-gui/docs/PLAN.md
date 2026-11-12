# Coda App — Plan

Status: MVP built (M0–M4), awaiting the owner's first hands-on smoke test; Linux packaging/CI in review (2026-11-05).

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

### Later
- Drag tracks/albums from browse views onto the queue.
- Letter-key jump in the artist list; Navidrome album headers with year when opened via "Go to album".
- Fixes made in Coda's engine don't flow here automatically; port them by hand when they matter.
