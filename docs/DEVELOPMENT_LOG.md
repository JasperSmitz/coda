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
