# M3 — Quality and convenience: implementation brief

- **Status:** approved by owner 2026-10-29 (autonomous session authorizing M1–M3). Scope chosen by the architect from the roadmap's candidate list.
- **Chosen:** queue controls, repeat, shuffle, session persistence, local search, Navidrome scrobbling, playback robustness.
- **Not chosen (still future):** artwork (vision excludes it initially), favourites/starring, gapless lookahead (touches the event-ordering core; revisit separately), resume-at-position, playlists, MPRIS.
- **Delivery:** three slices, each reviewed and committed.

## Slice 1 — queue controls, repeat, shuffle, persistence

### Queue controls (Queue panel)
| Key | Action |
|---|---|
| `J` / `K` | move selected entry down / up (selection follows it) |
| `C` | clear the queue (stops playback; clears `current_load` before `Stop`, like a deliberate stop) |
| `g` | select the current entry |
| `s` | shuffle the entries after the current one (all entries if none is current); current entry untouched |
| `R` (any panel) | cycle repeat: off → all → one → off |

- Queue ops stay pure in `queue.rs` (`move_up/move_down(id)`, `clear`, `shuffle_after_current(&mut impl FnMut(usize) -> usize)` or similar — randomness is injected, tests use a deterministic source; the app uses `getrandom`-seeded state; no new heavy dependency).
- Moving or shuffling never changes `current` or the loaded track.

### Repeat (player)
- `RepeatMode { Off, All, One }` in `Player`; shown in the now-playing bar.
- One: EOF of the current load reloads the same entry (fresh `LoadId`). Errors still advance (never loop a failing track). `n`/`p` behave as usual.
- All: at the end of the queue, EOF/error wraps to the first entry; `n` at the end wraps too.
- Termination: with All, consecutive errors are counted; once the count reaches the queue length, stop with "No playable tracks in queue". Any `Started` resets the count. Test with an all-missing queue under All and One.

### Persistence
- File `$XDG_STATE_HOME/coda/state.json` (same directory resolution as the log). Versioned JSON: `{ "version": 1, "volume", "repeat", "current": index|null, "queue": [ { "source": {"local": path} | {"navidrome": song_id}, title, artist, album, track_no, disc_no, duration_ms } ] }`.
- Never contains credentials, URLs or server address.
- Saved on every exit path that runs shutdown, and debounced (≈2 s after the last queue/volume/repeat change) during the session, written off the UI loop (`spawn_blocking`). Atomic: write a temp file in the same directory with mode 0600, `fsync`, rename.
- Loaded at startup before the TUI; missing → empty; unreadable/invalid/unknown version → start empty, log a warning, keep the bad file as `state.json.bad` (do not overwrite user data silently). Restored text goes through the same capping as tags. Cap restored queue length (e.g. 10,000 entries).
- Restore does not auto-play. The current entry is restored (Space plays it). Volume is applied with `SetVolume` once the backend is up.
- Local entries whose files vanished stay and fail like any missing file. Remote entries without Navidrome configured fail with "Navidrome is not configured" (existing resolution path).
- No `--no-restore` flag in this slice; clearing the queue with `C` covers the need.

### Tests
Queue move/clear/shuffle (deterministic), repeat transitions incl. termination, persistence round-trip, corrupt/unknown-version file handling, atomic write leaves no temp file, mode 0600, debounce emits saves as data (App stays pure: it emits a "save requested" flag/command and the loop performs I/O), keymap and render (repeat indicator).

## Slice 2 — local search and Navidrome scrobbling

### Local search
- `/` with the Local source opens the same single-line input as Navidrome search. `Enter` runs a case-insensitive substring match over title, artist, album and the path relative to the music root, across all scanned tracks; results are a flat track list (cap 1,000 shown, with the total). `Enter` plays now, `a` enqueues, `h`/`Esc` returns. Empty query cancels.
- Runs synchronously on submit (linear scan; measure with 50k synthetic tracks in a test — must stay well under a frame, otherwise move to `spawn_blocking`).

### Scrobbling (Navidrome tracks only)
- On `Started` of a remote track: `scrobble?id=…&submission=false` (now playing).
- Submission (`submission=true`) once per load when played time reaches 50 % of the duration or 4 minutes, whichever comes first (tracks shorter than 30 s are not submitted). Played time is accumulated from position progress while not paused; seeks do not count as listening.
- Fire-and-forget background requests on the existing bounded request path; failures are logged by kind only and never shown repeatedly.
- Config `[navidrome] scrobble = true` (default true), env override not needed.
- Logic for "should submit now" is pure and unit-tested; wiremock test verifies parameters.

## Slice 3 — playback robustness

- **Hung-mpv watchdog:** if a request gets no reply within 10 s, the backend reports `BackendExited("mpv stopped responding")` and kills mpv (existing behaviour for a crashed backend applies). Unit test on the pure state with a fake clock or via injected timeouts.
- **Timing-robust real-mpv tests:** `rapid_loads_end_on_the_last_requested_track` and `pause_volume_and_seek_are_reflected_in_events` fail on audio devices that play WAVs faster than real time (observed: Bluetooth CoreAudio device). Make them assert ordering and outcomes rather than wall-clock speed (e.g. longer generated files, or `--ao=null` in tests if the backend exposes test-only options), without weakening what they prove.
- **Structured volume rejection:** replace the generic error for a rejected `set_property volume` with a structured event so the player restores the displayed volume (M1 known limitation).

## Acceptance (all slices)
- Standard checks pass; ignored mpv tests pass on this machine after slice 3.
- `docs/ARCHITECTURE.md`, `ROADMAP.md`, `DEVELOPMENT_LOG.md` updated per slice.
