# Coda — Development log

Concise record of accepted slices, for resuming work. Newest last. Details live in briefs, ADRs and commits.

## 2026-10-22 – 2026-10-28 — M1 local playback
- Slices 1 and 2 implemented, reviewed and committed (08768e4, e6c510d, 5a9df7d, 5b6a5b4). Known limitations: `docs/plans/m1-local-playback.md` §7.
- M1 accepted by the owner on 2026-10-29 as part of the autonomous-session authorization.
- Next: M2 slice 1 (config, Subsonic client, streaming).

## 2026-10-29 — M2 slice 1: config, Subsonic client, streaming
- **Done:** TOML config + env overrides + `password_command`; `navidrome` client (token auth, artists/artist/album/search3, stream signing); `TrackSource::Navidrome`; `PlayerCommand` → `resolve::dispatch` → backend `PlaybackTarget::{File, Stream}`; mpv streams with `--network-timeout`; background ping status.
- **Decisions:** ADR 0003. Redirects disabled (Referer leak); relative `music_dir` in file rejected; overlong ids skip the item; reqwest `rustls` (aws-lc-rs, OS trust store).
- **Tests:** fmt, clippy -D warnings, `cargo test --all-targets` (194 unit + integration incl. 14 wiremock/raw-TCP client tests) pass. Ignored mpv: 14 pass incl. 3 new streaming tests; 2 pre-existing failures (`rapid_loads_end_on_the_last_requested_track`, `pause_volume_and_seek_are_reflected_in_events`) fail identically on 5b6a5b4 — this machine's Bluetooth CoreAudio device plays WAVs at the wrong speed. Not caused by M2.
- **Review:** no blocking findings. Fixed: streamed body-cap test, decode detail logging, 3xx message, `password_command` output cap + kill/reap proof, network errors log only kind, single `dispatch`, writable-config refusal for `password_command`.
- **Limitations:** no browse UI yet (slice 2); TLS failures report as "connect".
- **Next:** M2 slice 2 (remote browsing, search, TUI).

## 2026-10-29 — M2 slice 2: remote browsing, search, TUI (M2 complete)
- **Done:** `navidrome::browse` (pure Browser: nav stack, session cache, per-resource in-flight dedup, press-ordered enqueue jobs, search input); sources `1`/`2`; Artists → Albums → Songs; `/` search; `r` refresh; `☁` remote marker; bracketed paste into search only; README usage/config.
- **Decisions:** opens on Navidrome when no music dir; a successful fetch also marks the connection "connected"; job fetches limited to 2 of 4 permits so views are never starved; search from results replaces them; explicit runtime with 500 ms `shutdown_timeout` (also after panics); HTTP crates clamped to warn in the log regardless of `CODA_LOG`.
- **Tests:** fmt, clippy, `cargo test --all-targets` pass (221 unit + integration incl. 4 fetch-task tests). Ignored mpv: same two pre-existing device-related failures only. Pty smoke against a scripted fake Subsonic server: browse, enqueue, search, paste, exit and log hygiene verified.
- **Review:** two rounds. Fixed: hyper-util host logging, hidden batch failures, stuck in-flight on panicked/cancelled fetch, starvation by enqueue jobs, failed views not healing, stacked searches, multi-line paste executing commands, server text in logs, DNS hang at exit, panic-path shutdown bound, quit-time warnings.
- **Limitations:** cache never evicted within a session; connection state stays "connected" after later failures; a single scroll offset shared across remote views; not yet run against a real Navidrome server.
- **Next:** M3 slice 1 (queue controls, repeat, persistence).
