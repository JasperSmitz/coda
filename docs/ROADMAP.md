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

## M4 — HEART integration (proposed)
Recommendation (not approved for implementation): `docs/plans/m4-heart-integration.md`.
Define a small command/state API that HEART can call while Coda remains independently operable. Choose library embedding or a process protocol based on real needs.
