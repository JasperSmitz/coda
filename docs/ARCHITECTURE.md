# Coda — Architecture

> **Status:** M1 slice 1 (core, mpv backend, library scan) is implemented; the TUI (slice 2) is not. Update this file to describe reality as work ships. Do not present proposed modules as existing code.

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

- `config` — music directory from the CLI argument or `CODA_MUSIC_DIR` (empty counts as unset), validated and canonicalized.
- `model` — `Track`, `TrackSource::Local`, metadata with filename fallback.
- `queue` — pure ordered queue addressed by `QueueEntryId`.
- `player` — pure coordinator: user intents and `PlaybackEvent`s in, `PlaybackCommand`s out. Auto-advances only on EOF/error of the current `LoadId`.
- `playback` — boundary types and `PlaybackHandle` (unbounded commands, bounded events).
  - `mpv::protocol` — JSON IPC encode/decode.
  - `mpv::state` — pure entry-id ↔ `LoadId` mapping, load serialization and collapse, event buffering, redirect and unrequested-entry handling.
  - `mpv::process` — argv, private socket directory, spawn, connect retry.
  - `mpv` (driver) — async task: reader loop plus a separate writer task, file pre-check, shutdown with `ShutdownOutcome`.
- `library::scan` / `library::tree` — blocking-thread scan with incremental batches; directory browse tree.
- `main` — validates config and mpv availability, then exits; the TUI arrives in slice 2.

Not yet implemented: `app`, `ui`, signal handling, terminal lifecycle.

## Verification
- Standard: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets`, `cargo build`.
- With mpv installed: `cargo test --all-targets -- --ignored` (real-mpv lifecycle, playback and hostile-input tests).
- Event-ordering safety lives in `tests/event_ordering.rs`, which drives the real `MpvState` and `Player` through exhaustive message interleavings.
- Verified on macOS with mpv 0.41. Linux and older mpv versions are untested.
