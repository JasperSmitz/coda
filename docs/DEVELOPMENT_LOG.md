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

## 2026-10-29 — M3 slice 1: queue controls, repeat, persistence
- **Done:** queue `J`/`K` move, `C`×2 clear, `g` current, `s` shuffle upcoming, `R` repeat off/all/one; queue capped at 10k; `session` module persisting queue/current/volume/repeat to `state.json` with debounced atomic saves and safe restore; scrollable help overlay.
- **Decisions:** "proven playback" (≥ 1 s position) gates repeat to prevent hot loops; failures-in-a-row resets on proven playback or user loads; without Navidrome, runs of remote entries are skipped to the next local one in a single pass; restored local paths need not lie under the music root (state file is user-owned; mpv pre-check still applies); new `libc` dependency for `O_NONBLOCK` open.
- **Tests:** fmt, clippy, `cargo test --all-targets` (281 lib, 26 event-ordering, …) pass; ignored mpv tests: 15 pass excluding the 2 known device failures; pty smoke: quit/relaunch restores queue/volume/repeat, corrupt file quarantined with notice.
- **Review:** two rounds plus one regression fix. Fixed: 5 GB peak memory on hostile state file, repeat hot loop, `d` under All, quarantine overwrite, volume lost after backend exit, Saver poll race, UI freeze on huge unresolvable queue, uncapped live queue, unconfirmed clear, help overflow at 80×24, FIFO/TOCTOU on open, remote-skip regression.
- **Limitations:** tracks < ~1.25 s never repeat; abandoned exit saves may leave a `.tmp` file; quarantine name choice is check-then-rename.
- **Next:** M3 slice 2 (local search, scrobbling).

## 2026-10-29 — M3 slice 2: local search and scrobbling
- **Done:** `/` on Local searches title/artist/album/relative path (1,000 shown + total, snapshot overlay, Esc/h back); shared `line_input`; Navidrome scrobbling (now playing on start, submission at 50 %/4 min of real listening, once per load) via a bounded single-task worker; `[navidrome] scrobble` option; Esc leaves Navidrome search results too.
- **Decisions:** search stays synchronous (≈14 ms / 50k tracks in release; accepted deviation, documented in ARCHITECTURE); scrobble rule owned by `App` next to the credential-free `Player`; submission `time` = play start captured as `SystemTime`; server duration 0 treated as unknown.
- **Tests:** fmt, clippy, `cargo test --all-targets` (308 lib + integration) pass; ignored mpv: 15 pass excluding the 2 known device failures; pty smoke against a fake Subsonic server with scrobbling on and off.
- **Review:** one round; fixed zero-duration fallback, sleep-skewed timestamps, Esc inconsistency, a non-proving queue-bound test.
- **Limitations:** no Unicode normalization in search; queued scrobbles are lost at quit; listened time follows mpv's media clock.
- **Next:** M3 slice 3 (watchdog, timing-robust mpv tests, structured volume rejection).
