# Coda — Roadmap

Status legend: proposed / in progress / completed. Do not mark work completed based on a plan alone.

## M0 — Project setup (completed)
- Agent contract, vision, architecture notes, roadmap and review workflow established.
- Cargo initialization moved into M1 slice 1.

## M1 — Local playback (completed; accepted 2026-10-29)
Brief: `docs/plans/m1-local-playback.md`. Delivered in two independently reviewed slices:
1. Domain models, queue, player coordinator, mpv backend, library scan, tests. **(completed, reviewed)**
2. TUI, navigation, playback interaction, terminal and process lifecycle. **(completed, reviewed)**

Acceptance:
- Music directory from a CLI argument or `CODA_MUSIC_DIR`; local discovery and metadata.
- TUI can browse folders, select a track and manage a queue.
- mpv plays/pauses, seeks, changes volume and advances tracks only on end-of-file or playback error.
- Current track/progress shown; input remains responsive during scans and loads.
- Handles empty/missing libraries, missing mpv and playback failures; terminal restores and mpv exits on every catchable exit path.
- Queue/player unit tests, mpv integration tests and format/clippy/test/build checks.

## M2 — Navidrome integration (completed, reviewed)
Brief: `docs/plans/m2-navidrome.md`. Slices:
1. Config file and credentials, Subsonic client, remote tracks in the unified queue, mpv streaming. **(completed, reviewed)**
2. Remote browsing, search and TUI integration. **(completed, reviewed)**

Acceptance:
- Configure and connect to personal Navidrome via Subsonic/OpenSubsonic API.
- Browse/search remote library and stream into the same queue as local songs.
- Network errors, credentials and stream failures are handled safely.
- Distinguish sources clearly without unnecessary catalog deduplication.

## M3 — Quality and convenience (completed, reviewed)
Brief: `docs/plans/m3-quality.md`. Chosen scope, in slices:
1. Queue controls (move, clear, shuffle upcoming), repeat modes, session persistence. **(completed, reviewed)**
2. Local search and Navidrome scrobbling. **(completed, reviewed)**
3. Playback robustness: hung-mpv watchdog, timing-robust mpv tests, structured volume rejection. **(completed, reviewed)**

Not chosen: artwork, favourites, gapless lookahead, resume-at-position, playlists, MPRIS.

## M4 — Onboarding (completed, reviewed)
Brief: `docs/plans/m4-onboarding.md`. Decision: `docs/decisions/0004-keychain-password-storage.md`.
1. First-run setup in the TUI: optional local music folder, optional Navidrome connection (URL → username → password, verified before saving), password stored in the OS keychain. Skipping both exits and asks again next launch. **(completed, reviewed)**
2. Connect, edit or disconnect Navidrome from within the app, and redo setup (`coda --setup` or from the app). Part A, `coda --setup`: **(completed, reviewed)**. Part B, in-app settings (`,`): **(completed, reviewed)**.

## M5 — Library home and playlists (completed, reviewed)
Brief: `docs/plans/m5-library-home.md`. Slices: 1. Navidrome home, playlists (read-only), album list with sort and paging, `A` play next. **(completed, reviewed)** 2. Local home with tag-based artists/albums and `.m3u` playlists. **(completed, reviewed)** 3. Navidrome playlist editing: save queue as playlist, add to playlist, remove, reorder, rename, delete. **(completed, reviewed)**
- A home menu per source: Playlists, Artists, Albums (Songs only if it proves useful; search covers finding songs).
- Navidrome playlists: read-only in slice 1; creating, editing and saving the queue as a playlist in slice 3.
- All-albums list (`getAlbumList2`, paged) with useful sort orders.
- Local `.m3u` playlists may follow the same layout.

## M6 — Audio visualizer (completed, reviewed)
Brief: `docs/plans/m6-visualizer.md`. Moved ahead of keybindings (owner, 2026-11-01) so the owner can try the current keys first.
1. Feasibility spike: which signals mpv can deliver over IPC (levels, frequency bands), at what rate and cost, and for streams as well as files. Ends with written findings and a recommendation. **(completed)**
2. A visualizer in the now-playing area, built on what the spike shows to be feasible. A level meter at minimum; frequency bands if mpv can provide them without a second decoder. **(completed, reviewed: 7 bands plus an L/R meter, always-stereo output)**

## M7 — Keybindings and quality of life (completed)
Brief: `docs/plans/m7-keybindings-qol.md`; owner notes: `docs/M7_QOL_Findings.md`. Play in context: Enter plays what you point at, the queue is temporary.
- **Slice 1 (completed, reviewed):** new keymap (`h`/`l` and arrows navigate, `,` `.` seek, `<` `>` previous/next, `r`/`R`, `D`, `O`); Enter replaces the queue with the album, playlist or view and starts at the selected track; `s` plays shuffled (tracks, albums, playlists, artists, folders); playlist deletion asks y/n.
- **Slice 2 (completed, reviewed):** "Shuffle all" on each source's home (Navidrome via `getRandomSongs`, 500 songs).
- **Slice 3 (completed, reviewed):** the visualizer spreads across the width with interpolated bars; album rows show the running time before the year.
- Later, not in M7: endless play and context-aware autoplay; configurable keybindings.

## M8 — Gapless playback (completed, reviewed)
Brief: `docs/plans/m8-gapless.md`.
Consecutive tracks play without a pause, for local files and Navidrome streams.
- **Approach ("one ahead"):** Coda keeps owning the queue (ADR 0002, amended by ADR 0005) and keeps a copy of the next queue entry in mpv's playlist, so mpv's gapless audio and prefetching can move on by themselves.
- **Slice 1, spike (completed):** findings in the brief.
  - Measure today's gap (local and Navidrome).
  - Verify the approach on the minimum and current mpv versions: entry ids, events, a queue change just before a track ends.
  - Write down what the visualizer time code and scrobbling need at a seamless change.
- **Slice 2 (completed, reviewed):** the feature, briefed from the spike findings.
  - Every queue change updates mpv's next entry: remove, move, shuffle, play next, a context replace, and the repeat modes.
  - Track-change handling stays under the exhaustive event-ordering tests.
- **Related:** this also lays the groundwork for endless play, which needs the next track known ahead of time.

## M9 — Cover art (completed, reviewed; checked by the owner 2026-11-03)
Brief: `docs/plans/m9-cover-art.md`.
- The cover of the track you hear, in the now-playing bar: a real image with the Kitty graphics protocol (Ghostty, Kitty), half-block characters elsewhere (Alacritty, Terminal.app, WezTerm, iTerm2, tmux).
- Sources: Navidrome `getCoverArt` (size-capped disk cache) and local embedded art or folder images.
- **Slice 1, spike (completed):** findings in the brief.
- **Slice 2 (completed, reviewed):** the feature, as recommended by the spike.
- **Owner's check (2026-11-03):** seen in Ghostty (image) and macOS Terminal.app (half blocks, as intended).

## M10 — Now playing screen (completed, reviewed; checked by the owner 2026-11-03)
Brief: `docs/plans/m10-now-playing.md`. A display mode for when the queue is set and you just want to listen (owner, 2026-11-02): a toggle that swaps the panels for a large cover, a larger visualizer and the essential track info, without the text you don't need. Builds on M9.
- `f` opens and closes it; the global keys keep working, the list keys do nothing there.
- A large cover (1024 px, cached apart), Next up near the end of a track and after a change, a passing volume, a repeat mark.
- **Owner's check (2026-11-03):** seen in Ghostty and macOS Terminal.app.

## 1.0 — First release (in progress)
Brief: `docs/plans/release-1.0.md`. Version 1.0.0 covers M1–M10; what it contains is in `CHANGELOG.md`.
- The owner has used it on macOS (Ghostty, Terminal.app) through M10, on Linux through M8, and with their own Navidrome server.
- CI (`.github/workflows/ci.yml`) checks every pull request and push to `main` on Linux and macOS, including the mpv tests on Linux.
- Pushing a `vX.Y.Z` tag builds Linux and macOS binaries and publishes a GitHub release (`.github/workflows/release.yml`; steps in `docs/RELEASING.md`).
- Not in 1.0: endless play and autoplay, Homebrew and crates.io, signed or notarised macOS binaries.

### After 1.0
- Endless play and autoplay.
- Split `crates/coda-tui/src/app.rs` (move its inline tests to `crates/coda-tui/src/app/tests.rs` first).
- Narrow `pub` to `pub(crate)` for items that are not API (see `docs/LESSONS.md`).
- Move `setup::Reply` and `ping` to a neutral module.
- A test that compares the help overlay with the keymap table.
- An in-repo pty test for the Now playing screen (today a script outside the repository).
- `take_edit_requests` returning `Option`.
- A `debug_assert` for `swap_client` dropping commands.

## M11 — Genres (completed, reviewed; owner checks pending)
Brief: `docs/plans/genres.md` (approved 2026-11-15). A Genres view for both sources, with a genre's albums and shuffle by genre. Genres are read as tagged, split on Navidrome's default separators (`;`, `/`, `,`); no hierarchy, no editing or fetching.
- **Slice 1, core (completed, reviewed):** genres on tracks (local tags and Navidrome), saved with the queue; the local genre index; Navidrome `getGenres`, albums by genre and random songs by genre; Genres views in both browsers, not yet on their Home menus.
- **Slice 2, GUI (completed, reviewed; owner smoke checks G1–G9 pending):** a Genres sidebar entry for both sources, the genre list and a genre's albums with Play/Shuffle (Navidrome: Shuffle, random songs of the genre), `s` shuffling the genre, an album's genres as links; Navidrome tracks keep their genres in the queue (`TrackRef.genres`).
- **Slice 3, TUI (completed, reviewed; pty smoke run passed):** Genres on both Home menus, after Albums; the genre list shows album and track (Navidrome: song) counts, a genre opens on its albums with their artists, Enter plays an album, `s` shuffles the genre and, locally, `a`/`A` add it. A local library without genre tags says "No genre tags found". No new keys.

## M12 — Mixes (completed, reviewed; owner checks pending)
Brief: `docs/plans/mixes.md` (approved 2026-11-16). A mix is a saved recipe of genres, artists and albums from one source, resolved each time it plays and played as an endless, balanced shuffle. Saved mixes live in `$XDG_DATA_HOME/coda/mixes.json`, shared by both apps (ADR 0008).
- **Slice 1, core (completed, reviewed):**
  - **Part 1a (completed, reviewed):** the `mix` module (model with caps, the shared mixes file, the balanced `Picker`, local resolution by name, `ActiveMix` with its top-up and cap trim), the queue's context counter, the active mix in the session, `navidrome::server_id` in core. Nothing user-visible yet.
  - **Part 1b (completed, reviewed):** Navidrome resolution in `mix::navidrome`: a genre by its exact value (`getRandomSongs`, fetched again once played through, never when the sample held the whole genre), an artist's albums two at a time (at most 200, kept while the mix plays), an album; the id fallback by name through `search3` (and the artist index); the `MixFetches` coordinator (pending, in flight, failed with retry and `next_retry`, generation and queue-replaced checks, also before the start; refuses another server, terminal); at most two requests in flight per mix; `Fetch::spawn` answers exactly once. Nothing user-visible yet.
- **Slice 2, GUI (completed, reviewed):**
  - **Part 2a, backend (completed, reviewed):** the engine owns the active mix (`engine/mix.rs`): `playMix{id}` (read again from the mixes file on a blocking thread) and `playQuickMix{mix}`; local pools resolved inline from the library (again when the library changed, so a mix restored before the scan tops up once it has songs); Navidrome fetches spawned by the loop with `Fetch::spawn` (current client, shared fetch limit), results back on a channel, a retry timer from `next_retry`; `wants_top_up` checked after every intent, playback event and fetch result; the mix ends when the queue is replaced or cleared, on another server or none; "No songs for …" and fetch failures as notices; the session saves the mix that owns the queue and startup restores it (a saved mix read again, gone → none). Commands: `list_mixes`, `add_mix`, `update_mix`, `delete_mix` and the editor's `mix_search_genres`/`_artists`/`_albums`; the snapshot's `mix` (name, id, loading). TypeScript types and `lib/mixes.ts`; no screens yet.
  - **Part 2b, screens (completed, reviewed; owner smoke checks M1–M20 pending):** a **Mixes** sidebar entry per source (after Playlists) listing the saved mixes as cards (name, ingredients as chips with kind icons, Play, a menu with Play, Edit…, Delete… after a confirmation) with **New mix…** and a one-line empty state; the mix editor dialog (name, one debounced, keyboard-navigable search over the source's genres, artists and albums, chosen ones as removable chips, **Play** as a quick mix and **Save**, typed errors inline, "Changes apply the next time this mix plays"); **Add to mix…** in the genre and artist rows' menus, the album menu and the genre, artist and album pages (a saved mix of that source, or "New mix with this…" in the editor); "Mix: <name>" (or "Quick mix: …", dimmed with "Loading…" while its songs load) in Now Playing and under the queue header. Owner checks: `apps/coda-gui/scripts/smoke.md` "Mixes" (M1–M14).
- **Slice 3, TUI (completed, reviewed; pty smoke run passed):** **Mixes** on both Home menus (after Genres) lists the saved mixes of that source (Navidrome: the current server's), read from the shared file on a blocking thread whenever the view opens or on `R`; Enter or `s` plays one. Quick mix: `m` marks (or unmarks) the selected genre, artist or album row (a `✓` before it; one set of marks per source, the Navidrome marks starting over with every account), `M` plays the marks as a quick mix named after them and clears them. The status bar shows `Mix: <name>` or `Quick mix: <name>` (with `(loading…)` while a Navidrome mix fetches its first songs). The app owns the mix like the GUI engine (`app/mix.rs`): local pools inline from the library (again as the scan adds songs), Navidrome fetches spawned by the loop with `Fetch::spawn` under the browser's fetch limit, a retry timer, a top-up check after every action, playback event, fetch result and scan batch; a replace or clear ends it, another account ends a Navidrome mix; the TUI's session saves and restores it. No editing of saved mixes in the TUI. Core: `HomeEntry::Mixes`/`View::Mixes`/`Resource::Mixes` and mix rows in both browsers, `Row::ingredient`, `mix::Marks`.

## Postponed indefinitely
- **HEART integration.** The HEART project is on the back burner (owner, 2026-10-31). The earlier recommendation is kept for reference in `docs/plans/heart-integration.md`; Coda stays standalone.
