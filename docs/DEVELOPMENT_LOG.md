# Coda — Development log

Concise record of accepted slices, for resuming work. Newest last. Details live in briefs, ADRs and commits.

## 2026-10-22 – 2026-10-28 — M1 local playback
- Slices 1 and 2 implemented, reviewed and committed (08768e4, e6c510d, 5a9df7d, 5b6a5b4). Known limitations: `docs/plans/m1-local-playback.md` §7.
- M1 accepted by the owner on 2026-10-29 as part of the autonomous-session authorization.
- Next: M2 slice 1 (config, Subsonic client, streaming).

## 2026-10-29 — M2 slice 1: config, Subsonic client, streaming
- **Done:** TOML config + env overrides + `password_command`; `navidrome` client (token auth, artists/artist/album/search3, stream signing); `TrackSource::Navidrome`; `PlayerCommand` → `resolve::dispatch` → backend `PlaybackTarget::{File, Stream}`; mpv streams with `--network-timeout`; background ping status.
- **Decisions:** ADR 0003. Redirects disabled (Referer leak); relative `music_dir` in file rejected; overlong ids skip the item; reqwest `rustls` (aws-lc-rs, OS trust store).
- **Tests:** fmt, clippy -D warnings, `cargo test --all-targets` (194 unit + integration incl. 14 wiremock/raw-TCP client tests) pass. Ignored mpv: 14 pass incl. 3 new streaming tests; 2 pre-existing failures (`rapid_loads_end_on_the_last_requested_track`, `pause_volume_and_seek_are_reflected_in_events`) fail identically on 5b6a5b4 — device-related, not caused by M2 (root cause found in M3 slice 3: mpv reports EOF early on this Mac's audio output).
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

## 2026-10-29 — M3 slice 3: playback robustness (M3 complete)
- **Done:** hung-mpv reply watchdog (10 s, gap-excused, crashed-backend path, bounded 2 s reap); `PlaybackEvent::VolumeRejected` with `preferred_volume` persisted instead of fallbacks; timing-dependent real-mpv tests on `--ao=null`; test-only `MpvOptions` hidden from docs.
- **Root cause of the long-standing ignored-test failures:** mpv's `end-file eof` arrives early by the audio-output buffer (8 kHz mono WAVs fall back from CoreAudio to avfoundation with ≈4 s buffering on this Mac). Not a Coda bug; buffered audio still plays out (mpv drains before processing the next load). Recorded in LESSONS.
- **Tests:** fmt, clippy, build, `cargo test --all-targets` (319 lib + integration) pass; **all 20 ignored real-mpv tests pass** on this machine, including the watchdog SIGSTOP test and an end-of-track drain test.
- **Review:** one round; fixed unbounded reap, wrong "replies are immediate" premise (+ drain test), restored volume overwritten by rejection, a tight timing bound in a default-suite test.
- **Limitations:** an idle hung mpv is detected only at the next command; no mpv auto-restart (owner decision from M1); the bounded-reap timeout path has no automated test (needs an unkillable child).
- **Next:** M4 needs owner input — see `docs/plans/heart-integration.md` (since postponed indefinitely) open questions. Candidate follow-ups: gapless lookahead (ADR 0002), a real-server smoke test by the owner, a Linux TUI run.

## 2026-10-31 — M4 slice 1: first-run setup
- **Owner input:** HEART postponed indefinitely. New roadmap: M4 onboarding, M5 library home and playlists, M6 keybindings, M7 visual extras. The owner's first real-server run (macOS, `password_command` with the Keychain) worked.
- **Delivered:**
  - First-run setup in the TUI when no source is configured and Coda runs in a terminal: an optional music folder, then optional Navidrome (URL → username → masked password, checked with `ping`, a Ready confirmation), then the password goes into the OS keychain and the config file is written. Skipping both exits 0 without writing anything.
  - Modules: `onboarding` (pure), `setup` (driver), `config_writer`, `secret_store`, `ui::onboarding`.
  - `password_keychain = true` in the config (ADR 0004).
- **Crates:** `keyring-core` with `apple-native-keyring-store` and `zbus-secret-service-keyring-store`. The Linux backend is pure Rust, with no libdbus or OpenSSL.
- **Tests:**
  - On macOS: fmt, clippy with and without `--all-features`, and `cargo test --all-targets` (370 lib + integration) pass, as do all 20 ignored mpv tests.
  - In a Linux container (`rust:1-bookworm`): clippy (both) and `cargo test --all-targets` pass.
  - PTY smoke runs with the mock store and a fake Subsonic server cover skip, folder, Navidrome with a wrong then right password, Ctrl-C while saving, signals and hangup.
- **Review:** one round, no blocking defects. Fixed:
  - quit or a signal during a save abandoning it
  - the mock-store feature being indistinguishable from the real store
  - an orphaned keychain entry after a failed config write
  - a relative `CODA_CONFIG`
  - plus polish: back navigation, hints, the non-UTF-8 backup name, URL truncation, the `zbus` log clamp
- **Limitations:**
  - A signal between setup and the player loop is lost.
  - No timeout on keychain reads.
  - A cancelled macOS prompt shows a generic error.
  - A missing keychain entry means editing the config by hand (slice 2 replaces this).
  - A symlinked `config.toml` is replaced by a regular file (documented).
  - Not yet run against the real macOS Keychain, or as a TUI on Linux.

## 2026-10-31 — M4 slice 2 Part A: `coda --setup`
- **Delivered:**
  - `coda --setup` reruns setup, prefilled from the file. It never reads the keychain or runs `password_command`.
  - Skipping a source removes it, and skipping both is refused on a redo.
  - A Coda-owned keychain entry is deleted only after a successful write that changes or removes the account.
  - Settings set outside Coda are named on the screens they affect.
  - With `CODA_NAVIDROME_URL`/`_USER` set and no password variable, the server form is blocked: `k` keeps `[navidrome]` verbatim, `n` removes it.
  - The keychain error remedy now points to `coda --setup`.
- **Tests:**
  - macOS: fmt, clippy with and without `--all-features`, and `cargo test --all-targets` (404 lib tests plus the integration suites) pass, as do the 20 ignored mpv tests.
  - Linux container: three runs of clippy and the full tests passed, with output kept.
  - PTY smokes: account change, removing Navidrome, skip-both refused, no TTY, and the env-override keep and remove cases.
- **Review:** two rounds plus a follow-up. Fixed:
  - the env-override setup loop
  - a userinfo password shown in a prefilled URL
  - prefill truncation
  - plaintext-backup notes
  - the backup note rule
  - early validation of the folder argument
  - authority parsing via `Url`
  - the blocked-state refusal text
  - Keep failing loudly when the file changed
  - two timing-fragile tests (`password_command` timeout, scrobbler queue)
- **Earlier Linux exit 101:** most likely the container's OOM killer stopping the linker, which was observed once. Not confirmed, since the original output was not kept.
- **Limitations:** every redo save leaves a `.bak.<secs>` behind, so backups accumulate. With `CODA_NAVIDROME_PASSWORD` set, the form pings the typed values, not the env ones (noted on screen).

## 2026-10-31 — M4 slice 2 Part B: settings inside the app (M4 complete)
- **Delivered:**
  - A `,` Settings overlay. It shows the folder and the Navidrome status. Connect, Edit and Disconnect (`d d`) are available for Coda-owned settings; anything set outside Coda is read-only and named.
  - `2` without Navidrome offers "Press Enter to connect".
  - A hot client swap that leaves mpv alone: client generations on fetches and pings, old fetches aborted, the browser reset, the scrobbler replaced, and the load in flight at the swap never scrobbled.
  - Disconnect is refused unless the config file keeps a music folder.
  - A second Ctrl-C quits during a save.
  - No backup for files that are exactly what Coda writes.
- **Tests:**
  - macOS: fmt, clippy with and without `--all-features`, and `cargo test --all-targets` (448 lib tests plus integration) pass, as do the 20 ignored mpv tests.
  - Linux container (`-j 2`, output kept): clippy and the tests pass.
  - PTY smokes: connect while a local track plays (same mpv process, playback continues), edit to another account, disconnect, the `2` prompt, a server set outside Coda, and disconnect refused with a folder given only on the command line.
- **Review:** two rounds. Fixed:
  - disconnect leaving no source in the file
  - a load in flight scrobbled to the new server
  - old fetches holding permits after the swap
  - an old ping racing the swap
  - j/k under the prompt
  - backups accumulating
  - a hung keychain blocking quit
  - the help text
- **Limitations:**
  - An old ping is not aborted at the swap; it sends one request and its answer is ignored.
  - Quitting mid-save can leave an unused keychain entry.
  - The folder can only be changed with `coda --setup`.
  - The setup TUI has not been run on Linux.
  - Not run against the real macOS Keychain or a real server by the agents.

## 2026-11-01 — M5 slice 1: Navidrome home, playlists, albums, play next
- **Delivered:**
  - Navidrome Home (Playlists / Artists / Albums), with path titles.
  - Read-only playlists, with truncation shown ("first 10,000 of N").
  - An album list with six sort orders (`o`). Pages of 500 load near the end, deduplicated by id; random is a single page.
  - `A` plays next everywhere `a` works: press order is kept, the insertion point lives on the queue, and a job is inserted with one splice.
  - New API calls: `getAlbumList2`, `getPlaylists`, `getPlaylist` (24 MiB body cap).
- **Parser rewrite:**
  - Navidrome JSON is parsed without `serde_json::Value`, through per-endpoint reply types, so other sections are drained.
  - One object budget per response (100,000), and bounded string and id copies.
  - A hostile 24 MiB body went from 1.31–1.57 GB peak to about 19 MB extra.
- **Tests:**
  - macOS: fmt, clippy (both), and `cargo test --all-targets` (489 lib + integration) pass, as do the 20 ignored mpv tests.
  - Linux container: clippy and tests pass.
  - PTY smoke against a fake Subsonic server (1,234 albums, playlists): 36/36.
- **Review:** three rounds. Fixed:
  - `Value` buffering per item, then per list
  - duplicates across pages, and servers that ignore `offset`
  - silent playlist truncation
  - the play-next anchor on move, remove and remove-current
  - the "Added" status when nothing is current
  - O(n²) insertion
  - a redraw after paging
- **Limitations:**
  - A legitimate response beyond 100,000 objects (e.g. more than about 100k artists) loses the excess silently.
  - A duplicate key Coda reads fails the response.
  - Browse position is not persisted; Navidrome opens on Home.

## 2026-11-01 — M5 slice 2: local home, tag-based artists and albums, `.m3u` playlists
- **Delivered:**
  - A local Home (Playlists / Folders / Artists / Albums) with path titles and a "scanning…" note.
  - Artists and albums grouped from tags: album artist, else artist; case- and whitespace-insensitive keys; unknowns last. `o` sorts albums A–Z or by artist.
  - `.m3u`/`.m3u8` playlists collected by the scan and resolved without touching the file system, inside the canonical root only. Windows `\` separators are supported, and the rows show missing counts.
  - Each track is stored once in `LibraryTree` (`TrackId`).
  - `album_artist` is optional in `state.json`, which stays version 1.
- **Tests:**
  - macOS: fmt, clippy (both) and `cargo test --all-targets` (523 lib + integration) pass. On the pre-polish tree the developer ran the 20 ignored mpv tests and the Debian container (clippy and tests), all passing.
  - PTY smoke against generated tagged FLACs and hostile playlists: 36/36.
  - 50k tracks: worst batch about 2.8 ms on the input loop.
- **Review:** one round, nothing blocking; 400k-entry randomised resolve check against a real tree with symlinks found no escape. Polish fixed:
  - a too-broad URL detection
  - Windows separators
  - `O_NOCTTY`
  - a symlinked-folder integration test
- **Limitations:**
  - Playlist matching has no Unicode normalisation and is case-sensitive.
  - Absolute entries must use the canonical root.
  - Folders holding only playlists don't appear under Folders.
  - `l` is unbound (M6).

## 2026-11-01 — M5 slice 3: Navidrome playlist editing (M5 complete)
- **Delivered** (provisional keys until M6):
  - Save the queue as a playlist (`S`).
  - Add to a playlist through a picker (`P`).
  - New playlist (`N`), rename (`e`), delete (`D D`).
  - Remove a song (`d d`).
  - Reorder (`J`/`K`, then `w` to save or `Esc` to discard).
- **How edits work:**
  - Edits run as sequential jobs in a loop-level runner, each answering exactly once, with generation drops.
  - Removal and reorder are checked replaces: a refetch compares ids in order and the hidden-entry count.
  - Partial batches are reported honestly.
  - Writes go as POST form when `formPost` is advertised, otherwise as GET under 8 KiB.
- **Tests:**
  - macOS: fmt, clippy (both) and `cargo test --all-targets` (563 lib, 20 edit tests, plus the rest) pass; the 20 ignored mpv tests pass.
  - Linux container: fmt, clippy and three test runs pass.
  - PTY smoke against a recording fake Subsonic server: 59/59 in both the GET and the POST mode.
- **Review:** three rounds. Fixed:
  - Esc racing a save
  - the picker selecting by index
  - quitting mid-edit
  - edits overlapping across account swaps
  - unsafe index removal (Navidrome's hidden entries)
  - silent loss of hidden entries (now a warning with confirmation and a recheck)
  - detection caching
  - a long-flaky settings test that matched "pw" in a temp path
- **Limitations:**
  - A few-millisecond window between the stale check and the write remains, since Subsonic has no conditional write.
  - Queued edits that never started are dropped at quit, counted in the log.
  - On servers without `formPost`, playlists over about 250 songs can't be reordered or have songs removed.
  - Not yet run against the owner's real server.

## 2026-11-01 — M6: audio visualizer (complete)
- **Slice 1, spike:**
  - Levels and frequency bands are available from mpv over IPC (lavfi `astats` on a split, band-passed copy). The played audio stays bit-identical when the output is forced to stereo.
  - Analysis runs about 0.25 s ahead of the speaker; a time-code channel measures the lead exactly.
  - A bad graph passed at spawn or while idle breaks every file, so the filter is attached lazily.
  - Findings are in `docs/plans/m6-visualizer.md`.
- **Owner decisions:** the full visualizer with always-stereo output (`--audio-channels=stereo`), and the graph stays attached while hidden until the next track.
- **Slice 2:**
  - `v` toggles 7 bands plus an L/R meter in the now-playing box. It is off by default and persisted.
  - The filter is attached lazily with `af add`, polled at up to 30 Hz with one request in flight, and aligned by time code.
  - Smoothing is fast attack, slow decay, and the bars fade after 300 ms without readings.
  - A visualizer-only redraw is used.
  - No polling while hidden, paused, stopped or covered.
  - An unavailable filter is reported once, and playback continues.
- **Privacy:** logs no longer contain track titles, artists, albums or file paths (`loadfile` is marked sensitive, and scan errors log only the kind).
- **Tests:**
  - macOS: fmt, clippy (both) and `cargo test --all-targets` (613 lib plus integration) pass, and the ignored mpv tests pass 25/25. The real-mpv tests cover tones peaking in the right band, bit-identity, an unavailable graph, and a seek mpv cannot perform.
  - Linux container (mpv 0.35.1): the same, 25/25.
  - PTY smoke: 46/46.
- **Review:** one round plus follow-ups. Fixed:
  - a freeze after a seek mpv acknowledges but does not perform
  - stale readings after rapid seeks
  - superseded-load attach races
  - over-eager "unavailable"
  - polling while not drawn
  - a lenient session field
- **Limitations:**
  - Surround output is gone (always stereo).
  - Bands show about 50–130 ms after the sound.
  - The Linux PulseAudio/PipeWire lead has not been measured acoustically.
  - The graph stays attached while hidden until the next track (about 3.5–6 % of a core).
