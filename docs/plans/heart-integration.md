# HEART integration: architecture recommendation

- **Status:** postponed indefinitely (owner, 2026-10-31); kept for reference. Not approved for implementation. Written without inspecting or modifying the HEART repository; HEART's actual needs must be confirmed before a slice is briefed.
- **Goal (roadmap):** a small command/state API HEART can call while Coda remains independently operable, with no HEART dependency in Coda.

## 1. What Coda already offers

The M1–M3 design gives a natural seam:

- `Player` is pure: user intents in (`play_entry`, `toggle_pause`, `play_next`, `seek_relative`, `volume_up`, `cycle_repeat`, `enqueue`, …), `PlayerCommand`s out, plus read-only state (`status`, `position`, `duration`, `volume`, `queue`).
- `resolve::dispatch` turns those into backend commands, so anything that drives `Player` gets local and Navidrome playback for free.
- `PlaybackHandle` (mpv) is already behind a command/event channel boundary.
- `navidrome::Client` and `library::scan` are usable without the TUI.

What does *not* exist yet: a headless runner. The select loop in `app.rs` mixes the `App` (TUI state) with coordination (`Player`, playback events, fetches, saver, scrobbler).

## 2. Options

| Option | Shape | Pros | Cons |
|---|---|---|---|
| A. Library embedding | HEART depends on `coda-player` as a crate and drives a `Session` object | Lowest latency, typed API, no protocol | Couples release cycles and the Tokio runtime; HEART inherits mpv process management; any Coda panic is HEART's panic |
| B. Process protocol | Coda runs headless (`coda --serve`), HEART talks to it over a Unix socket with newline-delimited JSON commands and state events | Process isolation, independent releases, language-agnostic, mirrors how Coda itself talks to mpv | A protocol to version; one more process to supervise |
| C. Both, staged | Extract a headless core first (needed by both), expose it as a library, add the socket protocol on top | Each step is useful alone | Two surfaces to maintain if both ship |

## 3. Recommendation

**C, staged, stopping after step 2 unless HEART needs the library form:**

1. **Extract a headless core** (`core` or `session` runner): own `Player`, `PlaybackHandle`, `Navidrome` client, scrobbler and saver; accept a `Command` enum (play/pause/next/previous/seek/volume/enqueue by source/clear/repeat/shuffle) and publish a `State` snapshot + change events over Tokio channels. The TUI becomes one client of it, which also simplifies `app.rs`. No behaviour change; covered by existing tests.
2. **`coda --serve <socket>`**: a Unix socket in a private `0700` directory (same approach as the mpv socket), NDJSON, `{"v":1,...}` versioned, commands and state events mirroring step 1 types. No network listener, no auth beyond filesystem permissions. Never expose credentials, URLs or file paths beyond what the caller enqueued.
3. Only if HEART needs in-process control: publish the step-1 types as the library API.

Rationale: Coda already treats mpv as an external process behind a protocol for exactly the isolation and replaceability reasons that apply here. A process boundary keeps HEART free of Coda's runtime, mpv lifecycle and credentials, and keeps Coda free of HEART.

## 4. Open questions for the owner / HEART
- Does HEART need to browse (library, Navidrome) or only control playback and show now-playing?
- Should a HEART-controlled Coda also show its TUI, or run headless only? (Determines whether TUI and socket must coexist in one process.)
- Which side owns the Navidrome configuration?
- Is single-client enough, or must several HEART components subscribe to state?

## 5. Non-goals
MPRIS, network-exposed control, multi-user access, and any change to the HEART repository from this project.
