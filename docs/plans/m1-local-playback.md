# M1 — Local playback: implementation brief

- **Status:** approved by owner 2026-10-22. Slice 1 completed (08768e4, Linux hardening e6c510d). Slice 2 completed and reviewed.
- **Scope:** local files only. Navidrome is M2; everything under "Out of scope" stays out.
- **Delivery:** two slices, each implemented by the developer, independently reviewed, fixed, re-verified, then committed by the architect. No push.

## 1. Approved decisions

| Area | Decision |
|---|---|
| Queue | Coda exclusively owns queue state. mpv holds at most one playlist entry and plays one track at a time (ADR 0002). |
| Playback boundary | `PlaybackCommand` in, `PlaybackEvent` out, over Tokio channels. One concrete mpv backend; no trait. |
| Event loop | A single `tokio::select!` loop over terminal input, playback events, scan events, signals and a redraw tick. |
| mpv launch | `--no-config --idle=yes --no-video --no-terminal --input-ipc-server=<socket>`, plus `--ytdl=no --access-references=no`. Spawned directly (no shell), stdio null. |
| Platforms | Linux and macOS (Unix sockets). Windows out of scope. |
| Missing mpv | Detected at startup, before the TUI starts; clear message with install hint; non-zero exit. |
| Discovery | `walkdir` (no directory-symlink following) + `lofty`, on a blocking thread, results streamed in batches. |
| Config | One music directory: positional CLI argument, else `CODA_MUSIC_DIR`. No config file. |
| Logging | `tracing` to a file only. Never to stdout/stderr while the TUI is active. |
| Models | Opus for M1 developer and reviewer. |

## 2. Module layout (single package `coda-player`, lib + bin `coda`)

```text
src/
  lib.rs                 module declarations (lets integration tests reach internals)
  main.rs                CLI, tracing init, startup checks, runs app
  config.rs              music-dir resolution and validation
  model.rs               Track, TrackSource, TrackMetadata
  queue.rs               Queue, QueueEntryId, pure transitions
  player.rs              pure coordinator: queue + playback state -> PlaybackCommands
  playback/
    mod.rs               PlaybackCommand, PlaybackEvent, LoadId, EndReason, PlaybackHandle
    mpv/
      protocol.rs        JSON IPC request/reply/event types (serde)
      state.rs           pure: commands + mpv messages -> mpv requests + PlaybackEvents
      process.rs         argv, private socket dir, spawn, connect retry, shutdown
      mod.rs             async driver task wiring the above to the socket
  library/
    scan.rs              walkdir + lofty, ScanEvent batches
    tree.rs              directory tree built from scanned tracks (browse model)
  app.rs                 slice 2: App state, select loop
  ui/                    slice 2: render.rs, keymap.rs, help.rs
```

Dependency direction: `ui -> app -> {player, library, playback}`; `player -> {queue, model, playback types}`. Nothing outside `ui/` and `app.rs` imports Ratatui or Crossterm. Nothing outside `playback/mpv/` knows mpv exists.

Crates (latest stable at implementation time): `tokio`, `serde`, `serde_json`, `lofty`, `walkdir`, `tempfile`, `clap` (derive, env), `tracing`, `tracing-subscriber`, `thiserror` for module errors, `anyhow` at the binary edge only. Slice 2 adds `ratatui`, `crossterm` (`event-stream`), `futures-util`. Verify crate APIs against the downloaded sources under `~/.cargo/registry/src`, not memory.

## 3. Behaviour decisions

### 3.1 Domain
- `Track { source: TrackSource, title, artist, album, track_no, disc_no, duration }`; all metadata optional except `title`, which falls back to the file stem.
- `TrackSource::Local { path: PathBuf }` only. M2 adds a remote variant; do not stub it now.
- Paths are absolute (rooted at the canonicalized music dir). This also stops mpv from interpreting a local name like `foo://bar` as a protocol.

### 3.2 Queue (`queue.rs`, pure)
- Ordered entries, each with a unique monotonic `QueueEntryId`; duplicates of the same track are allowed and distinct.
- `current: Option<QueueEntryId>`. All operations address entries by id, never by index captured earlier.
- Operations: `append`, `insert_after_current` (appends if no current), `set_current`, `next`, `previous`, `remove`.
- Removing a non-current entry never changes `current`. Removing the current entry is handled by the player (3.3).
- **Enter on a library track (exact order):** a new entry for the track is inserted directly after `current` and played immediately; nothing is discarded, and the previously current entry stays in place, so `p` returns to it.
  - `[A, B*, C]` with B current → `[A, B, X*, C]`.
  - Current is the stopped last entry `[A, B*]` → `[A, B, X*]`.
  - No current entry `[A, B]` → `[A, B, X*]` (appended).
  - Empty queue → `[X*]`.
  - X already in the queue → a second, distinct entry is inserted; the existing one is untouched.
- A separate Play Folder / Play Album action is future work, not M1.
- No shuffle, repeat or persistence in M1. Because there is no repeat, error-driven advancing always terminates.

### 3.3 Player coordinator (`player.rs`, pure)
Holds the queue, `current_load: Option<LoadId>`, and the last reported paused/position/duration/volume. Every user intent and every `PlaybackEvent` is a method returning `Vec<PlaybackCommand>`; no I/O.

- Every load gets a fresh monotonic `LoadId`. `current_load` is set when `Load` is emitted.
- **Auto-advance happens only on `TrackEnded { load, reason: Eof | Error }` where `load == current_load`.** Error also records a user-visible message. An Error at the end of the queue emits `Stop`, because a rejected load can leave mpv playing the previous track.
- `TrackEnded` with any other `LoadId` is stale and ignored. This covers: the old track's EOF racing a user's Enter/n/p, and the `stop` end-file mpv emits when a track is replaced.
- `TrackEnded { reason: Stopped }` for the current load (an external stop) sets stopped state without advancing.
- Deliberate stop (quit, removing the current entry at end of queue) clears `current_load` *before* emitting `Stop`, so its end-file is stale by construction.
- End of queue: state becomes stopped; `current` stays on the last entry so Space replays it.
- `n`: play next entry; at end, no-op with message "End of queue"; with no current entry, no-op ("Nothing is playing").
- `p`: if started and position > 3 s, seek to 0; else play previous entry; at the first entry, seek to 0 (no-op while stopped or loading).
- Space: if loaded, toggle pause; if stopped with a current entry, play it; if no current and queue non-empty, play the first entry.
- Seeks act only after `Started` (mpv rejects seeks before `file-loaded`); during loading they are no-ops. Volume works whenever loaded, clamped to 0–100 in steps of 5. Steps are computed from the last requested volume, so rapid presses are not undone by delayed echoes from mpv; an unchanged target sends no command.
- Removing the current entry while loaded: behaves like `n`, then removes it; if there is no next entry, stop.
- Position events between `Load` and `Started` for the current load are ignored so the old track's progress never flashes. `Duration` is accepted from `Load` onward (it can precede `Started`) and cleared when the load ends.
- `BackendExited` (mpv crashed or its socket closed): clear `current_load`, stop playback, mark playback unavailable, and show a persistent error. Playback commands become no-ops with a message. No automatic mpv restart (owner decision).

### 3.4 Playback boundary (`playback/mod.rs`)
- `PlaybackCommand`: `Load { load: LoadId, path }`, `SetPause(bool)`, `Stop`, `SeekRelative(f64)`, `SeekAbsolute(f64)`, `SetVolume(u8)`, `Shutdown`.
- `PlaybackEvent`: `Started { load }`, `TrackEnded { load, reason }`, `Position(Duration)`, `Duration(Option<Duration>)`, `Paused(bool)`, `Volume(u8)`, `Error(String)`, `BackendExited(String)`.
- `Started` corresponds to mpv's `file-loaded` (attributed to the entry of the preceding `start-file`), not `start-file`.
- `EndReason`: `Eof`, `Error(String)`, `Stopped`. mpv's `stop`/`quit`/`unknown` map to `Stopped`. `redirect` maps to `Error`: mpv detects playlists by content, so a "track" can expand into entries Coda never requested.
- Contract: `Load` always starts unpaused (the backend sets `pause=no` first, because mpv's pause survives loading a new file).
- Contract: `Load` for a path that is missing or not a regular file produces `TrackEnded { Error }` without contacting mpv.
- Commands go over an unbounded channel so the UI loop never awaits. Events use a bounded channel; `Position` is coalesced to at most 4/s.

### 3.5 mpv backend (`playback/mpv/`)
- **Minimum mpv 0.33** (playlist entry ids on events). Use only `["loadfile", <path>, "replace"]`; the third/fourth `loadfile` arguments changed meaning in 0.38 and are not used.
- **Socket:** private `0700` directory from `tempfile` under `$XDG_RUNTIME_DIR`, else `$TMPDIR`, else `/tmp`; socket `mpv.sock` inside. A candidate is skipped if the full path is ≥ 100 bytes or the directory cannot be created (for example a stale `XDG_RUNTIME_DIR` after `su`). The directory is removed on shutdown.
- **Connect:** retry every 20 ms for up to 3 s; abort early if the child has exited. Failure is a startup error.
- **Requests:** newline-delimited JSON with integer `request_id`; replies are correlated by id. Unknown events and fields are ignored; malformed lines are logged and skipped, never panic.
- **Observed properties:** `time-pos`, `duration`, `pause`, `volume`.
- **Entry-id mapping (stale-event core):**
  1. Loads are serialized: one in flight; further `Load` commands queued meanwhile collapse to the latest.
  2. Send `set_property pause false`, `loadfile <path> replace`, then `get_property playlist/0/id`. Because Coda is the only playlist writer and `replace` leaves one entry, the reply is the new entry id. Map `entry_id -> LoadId`.
  3. `start-file`/`end-file` carry `playlist_entry_id`. Mapped ids become `Started`/`TrackEnded` with the `LoadId`. Events for unmapped ids above the highest mapped id that arrive while a mapping is pending are buffered (bounded; overflow drops the newest) and replayed once the id is known; otherwise dropped.
  4. A failed `loadfile` reply becomes `TrackEnded { Error }` for that `LoadId`.
  5. An unmapped `start-file` with nothing in flight means mpv is playing something Coda did not request: the backend sends `stop`.
  6. `file-loaded` carries no entry id; it is attributed to the entry of the preceding `start-file` ("opening"), cleared by that entry's `end-file`.
- **Socket writes:** a dedicated writer task owns the write half, so the driver loop never awaits a socket write (mpv stops reading requests while its replies are unread, which deadlocked a single-task design). Normal requests are FIFO; `quit` uses a priority path that overtakes only unwritten lines.
- **Process lifecycle:** `kill_on_drop(true)`. `Shutdown` closes the event receiver, sends priority `quit`, drains mpv output for up to 500 ms, then kills if needed, reaps and removes the socket dir; it reports a `ShutdownOutcome`. Socket EOF or child exit produces `BackendExited`; if the event channel closes without one, `PlaybackHandle::recv` synthesizes it once.
- **Missing binary:** spawn `NotFound`, or an early child exit with status 127, maps to a dedicated error. The program path is an internal parameter (default `mpv`) so tests can use a nonexistent one; it is not user configuration.

### 3.6 Library scan (`library/`)
- Runs in `spawn_blocking`. Sends `ScanEvent::Batch(Vec<Track>)` every 200 tracks or 100 ms, then `ScanEvent::Finished { tracks, skipped, errors }`.
- Stops early when the receiver is dropped (app quitting).
- `follow_links(false)`. File symlinks pointing at regular audio files are included; directory symlinks are not followed.
- Hidden files and directories (leading `.`) are skipped.
- Extension allowlist, case-insensitive: `mp3 flac ogg oga opus m4a aac wav aif aiff wv ape mpc`.
- Unreadable tags or unparseable files are still listed with filename fallback; mpv decides playability.
- Permission errors are counted and logged; the scan continues.
- Non-UTF-8 paths are skipped and counted, because mpv IPC is JSON/UTF-8.
- Browse model: a directory tree relative to the music root, built incrementally. Directories without audio are omitted. Directories sort by name, case-insensitively. Tracks sort by disc, track number, then filename.

### 3.7 Config and startup (`config.rs`, `main.rs`)
- `coda [MUSIC_DIR]`; `CODA_MUSIC_DIR` used when the argument is absent; the argument wins. No tilde expansion beyond what the shell does.
- Validated before the TUI starts: unset gives a usage hint and exit 2. Nonexistent, not a directory, or unreadable gives a clear error and exit 1.
- Existing but empty directory: TUI opens with an empty-library message.
- Startup order: parse config → init logging → spawn mpv and connect → enter TUI → start scan.

### 3.8 Logging
- File: `$XDG_STATE_HOME/coda/coda.log`, defaulting to `~/.local/state/coda/coda.log` on both platforms; truncated at each launch. Level from `CODA_LOG` (EnvFilter syntax), default `info`.
- Log the `LoadId` and display name, never raw `loadfile` arguments or raw IPC lines above `trace`. This keeps M2 stream URLs with tokens out of logs by construction.
- If the log file cannot be opened, continue without logging; do not fall back to stderr.

### 3.9 TUI (slice 2)
- Ratatui `init()`/`restore()` for raw mode and alternate screen. mpv cleanup on panic relies on unwinding dropping the backend (`kill_on_drop`). Do not set `panic = "abort"`.
- Panic hooks run even for panics later caught by `catch_unwind` (the scan's lofty guard) and for panics on Tokio worker threads. After `ratatui::init()`, install one hook that records the UI thread's `ThreadId`: on the UI thread, restore the terminal and chain to the hook that was active before `ratatui::init()` (not Ratatui's own, which would restore twice); on any other thread, only `tracing::error!` the message and location, never touching the terminal or stderr.
- Select loop branches: crossterm `EventStream`, playback events, scan events, `SIGTERM`/`SIGHUP`/`SIGINT` (graceful quit), and a 250 ms tick. A closed playback or scan channel disables its branch; a scan channel closing without `Finished` is shown as a failed scan. Redraw on state change or tick, never awaiting playback or scan work. Ctrl-C arrives as a key in raw mode and quits.
- Layout: Library panel (left), Queue panel (right), now-playing bar (title, artist, state, elapsed/total gauge, volume), and a one-line status area for transient messages (about 5 s). Below 60 columns, show only the focused panel. Show scan progress ("Scanning… 1,234 tracks") until finished.
- Quit always runs `Shutdown` and waits for it (bounded) before restoring the terminal.

### 3.10 Keymap

| Key | Library panel | Queue panel |
|---|---|---|
| `j`/`k`, `↓`/`↑` | move selection | move selection |
| `Enter` | directory: open; track: insert after current and play now | play selected entry |
| `h`, `Backspace` | parent directory | — |
| `a` | enqueue track; on a directory, enqueue all its tracks recursively in browse order | — |
| `d` | — | remove entry |
| `Space` | pause/resume/start | same |
| `n` / `p` | next / previous (3.3) | same |
| `←` / `→` | seek −5 s / +5 s | same |
| `+` (`=`) / `-` | volume ±5 | same |
| `Tab` | switch panel | same |
| `?` | toggle help (`Esc` closes; only `?`, `Esc`, `q` and Ctrl-C act while it is open) | same |
| `q`, `Ctrl-C` | quit | same |

`h`/`Backspace` and `d` extend the owner's keymap; they are needed to browse folders and manage the queue.

## 4. Slice 1 — core and mpv backend

**Delivers:** Cargo package, `model`, `queue`, `player`, `playback` (+ mpv), `library`, `config`. `main.rs` validates config, checks that mpv can be spawned, and exits with a short message that the TUI arrives in slice 2. No Ratatui dependency yet.

Library scanning sits in slice 1 because it is non-UI source logic, which keeps slice 2 to UI and integration.

**Acceptance:**
- All behaviour in 3.1–3.8 implemented and covered by the tests below.
- No module outside `playback/mpv/` references mpv. Domain, queue and player have no Tokio I/O.
- Missing mpv yields the dedicated error and install hint.
- `fmt`, `clippy -D warnings`, `test --all-targets`, `build` pass; ignored mpv tests pass locally with mpv installed.

**Tests:**
- *Queue:* append/insert on empty and non-empty queues; next/previous at bounds; remove before, after and at current; duplicates have distinct ids.
- *Player:* Load gets fresh `LoadId`; EOF and Error on current advance; Stopped does not advance; **stale `TrackEnded` after the user played another track is ignored**; deliberate stop's end-file ignored; end of queue stops; `p` restart threshold; Space from stopped/empty; remove-current; an all-missing queue terminates; `BackendExited` disables playback.
- *Protocol:* exact JSON for commands including paths with quotes, backslashes, newlines, Unicode and leading `-`; parse replies, events, `end-file` with and without `file_error`; unknown events ignored; malformed lines return errors.
- *mpv state:* id correlation; entry-id mapping; end-file before id reply is buffered and replayed; unknown ids dropped; collapsed queued loads; failed loadfile → `TrackEnded { Error }`.
- *Event-ordering safety (owner requirement; permutations also cover collapsed loads, stop in flight, failed loadfile, redirect and `file-loaded`):* prove that entry ids and asynchronous events can never advance the queue incorrectly. Drive the pure mpv state and the player together through scripted message sequences covering every relevant interleaving of `loadfile` reply, `playlist/0/id` reply, `start-file` and `end-file`. Cover at least:
  - **Rapid skipping:** several `n` presses before any reply; only the last load reaches mpv; exactly one track plays; no extra advance.
  - **Replacement during playback:** the old entry's `end-file` arrives with reason `stop` or `eof` (it finished at the same moment), before or after the new id reply; neither advances.
  - **Very short files:** the new entry's `start-file` and `end-file { eof }` both arrive before the `playlist/0/id` reply; the buffered EOF is replayed and advances exactly once.
  - **Failed load:** `end-file { error }` for the new entry before its id reply advances exactly once, with a message.
  - Each scenario asserts the final queue position and the exact number of `Load` commands emitted.
  - Integration counterpart (`#[ignore]`, real mpv): a queue of several generated 50 ms WAVs plays through in order exactly once; issuing rapid loads ends on the last requested track.
- *Process:* argv contents; socket dir mode `0700` and path length; nonexistent program → not-found error.
- *Scan (tempdir):* nesting, extension filter, hidden skip, directory symlink not followed, file symlink included, untagged fallback, tags written via lofty read back, incremental batches, early stop on dropped receiver, non-UTF-8 skip (Linux only).
- *Integration, `#[ignore]`, need mpv:* a generated 1 s silent WAV (written by the test, no binary fixtures) gives `Started` then `TrackEnded { Eof }`; pause/volume/seek reflected in events; a deleted file → `TrackEnded { Error }`; `Shutdown` exits the child and removes the socket dir; dropping the handle kills the child.

## 5. Slice 2 — TUI and lifecycle

**Delivers:** `app.rs`, `ui/`, the select loop, signals, the full keymap and the startup/shutdown path in `main.rs`.

**Acceptance:**
- All M1 roadmap criteria hold end to end.
- Input and redraw stay responsive during a scan of a large library (thousands of files) and while mpv is loading.
- Terminal is restored, and no `mpv` child remains, after: `q`, Ctrl-C, `SIGTERM`, `SIGHUP` (closing the terminal window), a startup error after the TUI started, and a panic.
- Error states display without crashing: empty library, deleted file mid-queue, unsupported file, mpv exiting mid-session.

**Tests:**
- *Keymap:* key → action per focused panel, including the help overlay swallowing keys.
- *App state:* directory navigation and selection clamping as batches arrive; Enter/`a`/`d` map to the correct player intents; status-message expiry.
- *Render:* Ratatui `TestBackend` checks for empty library, scanning, now playing, help and narrow layout. Assert key strings, not full snapshots.
- *Manual smoke (macOS, plus Arch when available):* play, pause, seek, volume, next/previous, auto-advance across an album; delete a queued file; empty and missing directory; mpv absent (`env PATH=/usr/bin:/bin coda ~/Music`); resize; each exit path above, then `pgrep -x mpv` shows no leftover; the log contains no raw IPC lines.

## 6. Out of scope
Navidrome, search, Play Folder / Play Album action, config file, persistence of queue/volume, shuffle/repeat, gapless playback, mpv auto-restart, custom keybindings, mouse, artwork, MPRIS, Windows.

## 7. Known limitations accepted for M1
- **Gaps between tracks.** Feeding mpv one track at a time means a short audible gap, noticeable on live or classical albums. A later fix can keep Coda's queue authoritative while handing mpv a one-track lookahead (ADR 0002).
- **SIGKILL orphans.** If Coda is killed with `SIGKILL`, an idle mpv remains. Every catchable exit path cleans up.
- **No hung-mpv watchdog.** If mpv stays alive but stops responding, requests queue in memory and the UI is not told. A reply timeout producing `BackendExited` is a candidate follow-up.
- **Early seeks are dropped.** A seek while a track is still loading is a no-op.
- **Linux coverage is container-only.** Slice 1 passed on Debian bookworm (mpv 0.35.1, native arm64) and Arch (mpv 0.41, amd64 emulation) in Docker. No physical Linux desktop and no mpv 0.33/0.34 have been run.
- **A rejected volume change can show the wrong volume.** A failed `set_property volume` reaches the player only as a generic error, so the requested value stays displayed until the next press or echo. Fixing it needs a structured backend event.
- **Large queues redraw in full.** Every queue entry is rebuilt per frame: about 8 ms at 50,000 normal entries in release builds, but about 200 ms at 5,000 deliberately hostile (maximum-length, zero-width-heavy) entries. Rendering only the visible window fixes both and is the first candidate follow-up.
- **No working audio device.** Every track ends with "audio output initialization failed" and the queue errors through to the end, one track at a time.
