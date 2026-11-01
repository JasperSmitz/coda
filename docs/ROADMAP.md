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

## M7 — Keybindings and quality of life (proposed)
The owner is trying the current keys first; their notes set the scope.
- Play a whole album or playlist directly, instead of only adding it to the queue. Options: Enter plays the whole list while another key opens it, or a modifier plays it (Shift+Enter is reported only by terminals with the kitty keyboard protocol, so it needs a fallback).
- Settle the provisional keys from M4 and M5: `,` `S` `P` `N` `e` `D` `w`, and `l`, which is not bound yet.
- Configurable keybindings only once the defaults are settled.

## M8 — Cover art (proposed)
- Real images in terminals with an image protocol (Kitty, WezTerm, iTerm2, Ghostty), and a block-character fallback elsewhere.

## Postponed indefinitely
- **HEART integration.** The HEART project is on the back burner (owner, 2026-10-31). The earlier recommendation is kept for reference in `docs/plans/heart-integration.md`; Coda stays standalone.
