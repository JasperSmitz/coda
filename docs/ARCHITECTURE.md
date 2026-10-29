# Coda — Architecture

> **Status:** M1 (local playback) is implemented: core, mpv backend, library scan and TUI. M2 (Navidrome) is implemented: config file, Subsonic client, remote tracks in the unified queue, remote browsing and search. M3 slice 1 adds queue controls, repeat modes and session persistence. Update this file to describe reality as work ships. Do not present proposed modules as existing code.

## Intended boundaries

```text
Ratatui frontend / keyboard input
             |
       application commands
             |
  player state + unified queue
       /             \
local source       Navidrome source (M2)
             |
    playback controller
             |
     mpv JSON IPC backend
```

The drawing is conceptual, not a dependency diagram. Source adapters locate/browse tracks and resolve playback requests; the playback controller is responsible for controlling mpv.

## Responsibilities
- **Domain:** track/source identity, queue semantics, player state and commands; no terminal widgets.
- **Sources:** browse/scan metadata and resolve local paths or authorized remote streams. Local in M1; Navidrome via Subsonic/OpenSubsonic in M2.
- **Playback:** mpv process lifecycle, JSON IPC, playback status/progress and error reporting.
- **Application:** coordinate async tasks and propagate player state/events to the UI.
- **TUI:** draw views and translate keys to commands; never block on file/network I/O.

## Decisions / constraints
- Begin with one Rust Cargo package. Split into crates only if there is a real packaging or reuse requirement.
- Use mpv through IPC rather than shell-interpolated commands or reimplementing audio decoding (ADR 0001).
- Coda exclusively owns the queue; mpv plays one track at a time. Stale track-end events are filtered by load id (ADR 0002).
- The playback boundary is a command enum in and an event enum out over Tokio channels, with one concrete mpv backend and no trait.
- The application runs one `tokio::select!` loop over terminal input, playback events, scan events, signals and a redraw tick. Scans run on blocking threads; nothing in the loop awaits player or file I/O.
- Queue transitions and the advance/stale-event logic are pure code, testable without mpv or a terminal.
- Supported platforms: Linux and macOS. Windows is out of scope.
- Logs go to a file only; raw IPC and playback targets are never logged above `trace`.
- Avoid early commitments on daemon mode or HEART interoperability protocol. A reusable library boundary may be enough at first.

Proposed M1 module layout and behaviour details: `docs/plans/m1-local-playback.md`. They become architecture here only once they ship.

## Implemented modules
Single package `coda-player`: library `coda_player`, binary `coda`.

- `config` — optional TOML file (`CODA_CONFIG`, else `$XDG_CONFIG_HOME/coda/config.toml`, else `~/.config/coda/config.toml`) plus env overrides. Music directory: CLI > `CODA_MUSIC_DIR` > file (absolute, validated, canonicalized). Navidrome url/user/password (file, `CODA_NAVIDROME_*`, or `password_command` argv with timeout and output cap). File permission checks for secrets; parse errors never echo source lines (ADR 0003).
- `model` — `Track`, `TrackSource::{Local, Navidrome { song_id }}`, metadata with fallbacks and length caps.
- `queue` — pure ordered queue addressed by `QueueEntryId`, capped at 10,000 entries: append/insert, move up/down, clear, shuffle-after-current with injected randomness (small SplitMix64 `Rng` seeded from `getrandom`), and a revision counter for change detection.
- `player` — pure coordinator: user intents and `PlaybackEvent`s in, `PlayerCommand`s out (loads carry the `TrackSource`). Credential-free. Auto-advances only on EOF/error of the current `LoadId`. `RepeatMode::{Off, All, One}`: a repeat reload or wrap is a fresh load, so stale-event filtering is unchanged. A load counts as *proven* once it reports ≥ 1 s of position after `Started`; an unproven EOF is treated like a failure (it advances instead of repeating), and failures in a row stop playback once they reach the queue length, so repeat can never hot-loop. Consequence: tracks shorter than about 1.25 s are never repeated.
- `resolve` — turns `PlayerCommand`s into backend `PlaybackCommand`s: local path → `PlaybackTarget::File`, Navidrome song → freshly signed `PlaybackTarget::Stream`. An unresolvable load is fed back to the player as `TrackEnded { Error }` so the queue advances; when Navidrome is not configured, runs of remote entries are skipped in one pass to the next local entry. Shared by the app and real-mpv tests.
- `navidrome` — Subsonic client (`reqwest`/rustls): salted-token auth with a fresh salt per request, ping/getArtists/getArtist/getAlbum/search3, stream URL signing. Timeouts, 8 MiB streamed body cap, redirects disabled, URLs stripped from errors, lenient parsing of untrusted responses. `Password`, `StreamUrl` and `Client` redact in `Debug`/`Display`.
- `playback` — boundary types and `PlaybackHandle` (unbounded commands, bounded events).
  - `mpv::protocol` — JSON IPC encode/decode.
  - `mpv::state` — pure entry-id ↔ `LoadId` mapping, load serialization and collapse, event buffering, redirect and unrequested-entry handling.
  - `mpv::process` — argv (incl. `--network-timeout`), private socket directory, spawn with `CODA_NAVIDROME_*` removed from the environment, connect retry.
  - `mpv` (driver) — async task: reader loop plus a separate writer task, file pre-check (files only; streams skip it and are never trace-logged), shutdown with `ShutdownOutcome`.
- `session` — `state.json` in the state dir: versioned JSON of queue (source + metadata, never URLs or credentials), current index, volume and repeat. Loaded before the TUI with a 16 MiB cap, `O_NONBLOCK` open + fstat, typed deserialization with a capped sequence visitor; invalid files are quarantined as `state.json.bad.<secs>` (never overwriting) and the user is told once. Saved atomically (temp file 0600, fsync, rename, dir fsync) by the app's `Saver`: debounced 2 s after a revision change, one write at a time on `spawn_blocking`, and a bounded final save at exit, run alongside backend shutdown. Restore never auto-plays.
- `library::scan` / `library::tree` — blocking-thread scan with incremental batches; directory browse tree.
- `app` — App state and the single `tokio::select!` loop (terminal input, playback events, scan events, SIGTERM/SIGHUP/SIGINT, 250 ms tick); terminal lifecycle and the panic hook. Every exit runs a bounded backend shutdown, then restores the terminal.
- `ui::keymap` / `ui::render` / `ui::help` — key-to-action mapping per focused panel; Library, Queue, now-playing bar and status line; help overlay. Untrusted text is sanitized (control and bidi characters) and truncated by display width with bounded work. Library and Queue panels keep a persistent scroll offset and build only the visible rows, so frame cost does not grow with list length.
- `navidrome::browse` — pure remote browse state (`Browser`): navigation stack of views (Artists → Artist → Album, plus Search), session cache keyed by `Resource`, at most one in-flight request per resource, FIFO enqueue jobs completed strictly in press order, single-line search input. Emits `FetchRequest`s as data; `spawn_fetch` runs them on Tokio tasks under a 4-permit semaphore, with background enqueue jobs limited to 2 in flight so browsing is never starved. A drop guard guarantees every fetch answers exactly once (`Error::Aborted` on panic/cancel).
- `app` has two library sources (`Source::{Local, Navidrome}`, keys `1`/`2`) and a `Connection` state. It drains fetch requests at the top of each loop iteration and applies `FetchResponse`s from a channel branch. Bracketed paste is enabled; pastes go only into the search input, filtered like typed text.
- `app` also starts a background Navidrome `ping` after the TUI is up and reports the result in the status line; without a music directory there is no scan and the library shows "No local music directory configured".
- `main` — config (including `password_command`), file logging, optional Navidrome client, mpv spawn and connect, then `app::run` on an explicitly built runtime that is shut down with a 500 ms bound (also after a panic), so a stuck DNS lookup cannot delay exit. The log filter always clamps HTTP crates (`hyper_util`, `hyper`, `h2`, `reqwest`, `rustls`) to warn, whatever `CODA_LOG` says, because hyper-util logs peer addresses at debug.

Only `app` and `ui` depend on Ratatui/Crossterm.

## Verification
- Standard: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets`, `cargo build`.
- With mpv installed: `cargo test --all-targets -- --ignored` (real-mpv lifecycle, playback, hostile-input and HTTP streaming tests).
- Navidrome client tests run against `wiremock` and a raw TCP server (`tests/navidrome_client.rs`); no test contacts a real server.
- Event-ordering safety lives in `tests/event_ordering.rs`, which drives the real `MpvState` and `Player` through exhaustive message interleavings.
- UI rendering is covered by Ratatui `TestBackend` tests; the panic hook has its own test binary (`tests/panic_hook.rs`).
- Verified on macOS with mpv 0.41, including pty-driven smoke runs of every exit path. Slice 1 also passed in Debian (mpv 0.35) and Arch (mpv 0.41) containers; the TUI has not been run on Linux.
