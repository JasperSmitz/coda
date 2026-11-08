# Brief: M3 — queue management and Navidrome playlists

Read first: `AGENTS.md`, `docs/DESIGN.md`, `docs/ARCHITECTURE.md` (M1 engine/intents, M2 browse: `BrowseHandle`, `TrackRef`, context menu with its "Add to playlist" placeholder, playlists views; settings client swap). Reference behaviour: `../coda/src/navidrome/edit.rs` and how `../coda/src/app.rs` drives it (`EditRunner`, `save_queue_as_playlist`, `queue_playlist_name`, the hidden-entries confirmation), `../coda/docs/USAGE.md` (read-only; never modify `../coda`).

## Queue
- Drag-and-drop reordering in the queue panel (pointer-based, works with virtualization, autoscroll near the edges; keyboard: `Alt+↑/↓` moves the selection). Backend: a `MoveEntry{key, to_index}` intent built on `Player`'s queue moves (add a `Player`/`Queue` move-to method in `coda-core` with tests if only up/down exist), then `dispatch` so gapless lookahead stays right.
- Multi-select (click, Shift/Cmd/Ctrl-click, Shift+↑/↓) with Remove, Play next, and drag of a selection.
- Clear queue (keeps the playing track? match Coda's semantics), Shuffle upcoming (`Player::shuffle_upcoming`), "Save queue as playlist" (Navidrome).
- Drag tracks/albums from browse views onto the queue panel (insert at drop position) — nice-to-have if it fits.

## Navidrome playlists (`coda_core::navidrome::edit`)
- One edit runs at a time (serialize like Coda's `EditRunner`), results come back as notices and refresh the affected browse views (invalidate the frontend cache for that playlist and the playlists list).
- Create (from the playlists view; from "Save queue as playlist" with Coda's default name rule), rename, delete (confirm dialog), add tracks (context menu "Add to playlist" → submenu/picker of editable playlists + "New playlist…"), remove tracks and reorder tracks on a playlist page (drag-and-drop, like the queue).
- Respect `Access::editable_by` (read-only playlists show no edit controls, with Coda's `read_only_reason`) and the **hidden entries** rule: when a playlist has hidden/unreadable entries (`Hidden::Some/Suspect`), any edit that would rewrite it needs an explicit confirmation dialog explaining what may be lost (Coda uses a second key press; the GUI uses a dialog).
- Stale edits (`EditResult::Stale`/`Partial`) surface clearly and refresh.

## UI
Dialogs (confirm, text input for names) as small reusable components matching DESIGN.md, keyboard-accessible (Enter/Esc, focus trap).

## Acceptance
All checks from AGENTS.md pass; tests for new engine/core logic (wiremock for Navidrome edits as in `coda-core`'s tests). No window opened; extend `scripts/smoke.md`. Update `docs/ARCHITECTURE.md`. Don't commit. Report to `/tmp/coda-app-m3-report.md`.
