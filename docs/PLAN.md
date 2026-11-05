# Coda App — Plan

Status: M0 done; building the MVP (M1–M4) (2026-11-05).

## Stack (decided)
- **Tauri 2** desktop shell (system webview: WebKitGTK 4.1 on Linux, WKWebView on macOS).
- **Rust backend** in `src-tauri/`, on `crates/coda-core`: Coda's engine, vendored (Coda stays TUI-only and is never modified).
- **React 19 + TypeScript + Vite** frontend in `src/`.
- Playback stays **mpv via Coda's IPC backend**; mpv must be on `PATH` (as for the TUI).

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

### M0 — Engine vendored (done)
- `crates/coda-core`: Coda's UI-free modules without `app`/`setup`/`ui`/`main`, plus `Player::seek_absolute`, public `Player::set_volume`, `logging`, `setup_actions` (Navidrome save/ping) and a config bootstrap. 532 unit + integration tests pass.

### M1 — A window that plays the queue
- Backend: config load, logging, Navidrome client, mpv backend, session restore (never auto-plays), engine actor, debounced session save, final save + mpv shutdown on quit.
- UI: now-playing bar (title/artist/album, play/pause, previous/next, seek bar, volume slider, repeat), queue list (current track highlighted, double-click/Enter plays an entry, remove), status/notice line for player messages and errors (mpv missing, no config).
- Keyboard: space, n/p (or arrows), +/-, r.
- Acceptance: launches with the TUI's existing config and session; controls work for local and Navidrome tracks; gapless still works; quitting leaves no mpv process.

### M2 — Browse and play
- Local library (scan progress; artists → albums → tracks; playlists; folders) and Navidrome (artists, albums with the six sorts, playlists), search.
- Play in context, shuffle, enqueue/enqueue next, shuffle all.
- Cover art (current track + album grids) via the URI scheme and Coda's disk cache.
- Scrobbling.

### M3 — Queue and playlists
- Drag-and-drop queue reordering, multi-select, clear, shuffle upcoming.
- Navidrome playlist create/rename/reorder/delete/save queue as playlist, with the hidden-entries confirmation.

### M4 — Polish and platform
- Onboarding and settings (Navidrome connect/disconnect, music folder) in the GUI.
- Full-window now-playing view with large cover and visualizer.
- Media keys / MPRIS on Linux, Now Playing on macOS.
- Packaging: AppImage/`.deb` and an Arch `PKGBUILD`; macOS `.dmg`. CI on Linux and macOS.

### Later
- Fixes made in Coda's engine don't flow here automatically; port them by hand when they matter.
