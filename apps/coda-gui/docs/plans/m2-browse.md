# Brief: M2 browse — local + Navidrome library, search, play in context

Read first: `AGENTS.md`, `docs/DESIGN.md`, `docs/ARCHITECTURE.md` (what M1 built: engine actor, intents, DTOs, `TrackRef`, cover registry). Follow M1's patterns. Reference for behaviour: `../coda/src/library/browse.rs`, `../coda/src/navidrome/browse.rs`, `../coda/src/app.rs` (read-only; never modify `../coda`).

A parallel slice (Now Playing view, settings, media keys) is being built on another branch and also touches `engine.rs`, `dto.rs`, `types.ts`, `Sidebar.tsx`, `App.tsx`. To keep merges easy: put new backend code in new modules (`browse.rs`, `browse/…`), add to shared files in small, separate blocks, and don't reformat or reorganise code you don't need to change.

## Backend
1. **Local refs by id:** change `TrackRef`'s local variant to the library's `TrackId` plus a library generation (bumped when the library is replaced/rescanned), instead of a path, so non-UTF-8 paths play. Stale generations are rejected with a notice.
2. **Browse commands** (async Tauri commands returning DTOs; they don't touch `Player`):
   - Local: artists, albums (all, and per artist), album tracks, folders (tree level by level), playlists (`.m3u`) and their tracks, scan state. Read from the engine's `Library` — get a snapshot/answer through the actor or a cheap shared read-only handle; never block the actor on long work.
   - Navidrome: artists, artist → albums, album lists with the six `AlbumSort`s (paged, infinite scroll), album tracks, playlists, playlist songs (include the `Hidden` warning data), random songs. Use the current `Client` (cheap `Clone`) obtained from the engine with its generation; drop results from an old generation. Limit concurrency (as Coda's `MAX_CONCURRENT_FETCHES`).
   - Search: local (`library::search`) and Navidrome (`Client::search`), returning artists/albums/tracks.
   - Cover URLs for albums/artists/playlists through M1's cover registry.
3. **Play in context:** reuse M1's `PlayTracks`/`Enqueue` intents (fix them if needed); add "shuffle all" for local and Navidrome (random songs) matching Coda's behaviour, and shuffle-context (`s`), which for an artist means all of the artist's tracks.
4. Tests for the browse layer (wiremock for Navidrome as `coda-core` tests do) and the id/generation rule.

## Frontend
1. Navigation with a back stack (Back: `Backspace`, `Alt+←`, mouse back button) and the sidebar entries from DESIGN.md wired to real views; source sections hidden when not configured.
2. Views: album grid (virtualized, covers, title/artist, sort selector for Navidrome), artist list (alphabetical with letter index), artist page (album grid + "Play"/"Shuffle"), album page (header with large cover, year, track count, duration, Play/Shuffle; track table with disc grouping), playlists list + playlist page, folders view, search view (`/` or `Ctrl+F`, debounced, grouped results).
3. Track table per DESIGN.md (playing row marked), keyboard navigation in lists (↑/↓, Enter plays context), double-click plays context.
4. Context menu (right-click and a "…" button): Play, Play next, Add to queue, Go to album, Go to artist. Leave a clearly marked spot for "Add to playlist" (built in a later slice).
5. Loading skeletons, empty states, error states with retry; scan progress for the local library.

## Acceptance
Checks from AGENTS.md all pass (`cargo fmt`, clippy `-D warnings`, `cargo test --workspace`, `pnpm build`). No window opened on the owner's desktop; add manual steps to `scripts/smoke.md`. Update `docs/ARCHITECTURE.md`. Don't commit. Report to `/tmp/coda-app-browse-report.md`.
