# Coda — Architecture

> **Status: proposed design; not yet implemented.** Update this file to describe reality as work ships. Do not present proposed modules as existing code.

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
_None yet — project scaffold only._

## Verification
_Commands and platform-specific smoke-test notes will be recorded after M1 exists._
