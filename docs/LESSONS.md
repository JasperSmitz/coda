# Coda — Lessons

Record only verified, recurring lessons that would change how future work is done.

Each entry should state:
- **Context / symptom**
- **Root cause** (grounded in code or test evidence)
- **General rule or safeguard**
- **Where verified** (commit, test, or review)

### mpv detects playlists by content, not extension
- **Context / symptom:** a file named `evil.mp3` containing `#EXTM3U` and two paths made mpv play both, while Coda showed Stopped.
- **Root cause:** mpv expanded the "track" into new playlist entries (`end-file` reason `redirect`) that Coda never mapped. An extension allowlist does not limit what mpv opens.
- **Rule:** launch mpv with `--access-references=no`, treat `redirect` as a playback error, pre-check that targets are regular files, and stop any entry Coda did not request.
- **Where verified:** M1 slice 1 review; `playlist_disguised_as_audio_is_rejected`, `redirect_of_a_mapped_entry_is_an_error`.

### Convert untrusted float seconds with `try_from_secs_f64`
- **Context / symptom:** a `duration` of `1e300` from mpv panicked the playback driver.
- **Root cause:** `Duration::from_secs_f64` panics on overflow, NaN and negative input.
- **Rule:** parse durations from untrusted sources (files, mpv, future server responses) with `Duration::try_from_secs_f64` and treat failure as unknown.
- **Where verified:** M1 slice 1 review; `untrusted_numbers_never_panic`.

### mpv accepts `seek` only after `file-loaded`
- **Context / symptom:** seeks sent just after `start-file` failed with "error running command".
- **Root cause:** the file is not yet open at `start-file`. `file-loaded` carries no entry id, so it must be attributed to the preceding `start-file`.
- **Rule:** treat `file-loaded` as "started" and gate seeks on it.
- **Where verified:** M1 slice 1 review IPC probes on mpv 0.41; `file_loaded_is_attributed_to_the_opening_entry`, `seeks_are_no_ops_until_started`.

### Never await socket writes in the loop that reads replies
- **Context / symptom:** flooding commands without draining events made shutdown time out.
- **Root cause:** mpv stops reading requests while its replies are unread, so a driver blocked in `write_all` never got back to reading.
- **Rule:** give IPC writes their own task (or otherwise never await a write on the read loop), and keep a priority path for shutdown.
- **Where verified:** M1 slice 1 fixes; `shutdown_is_graceful_even_when_nobody_drains_events`.

### Do not let Ratatui's `Terminal` drop on a hung-up tty
- **Context / symptom:** closing the terminal window (SIGHUP) made Coda exit with code 101 (a panic) instead of 0.
- **Root cause:** Ratatui's `Terminal::drop` shows the cursor and `eprintln!`s on failure; on a hung-up tty both writes fail with EIO and `eprintln!` panics.
- **Rule:** release the terminal explicitly with fallible calls (`try_restore`, explicit cursor show), log failures, and never write to stderr with panicking macros on exit paths.
- **Where verified:** M1 slice 2 pty smoke harness (SIGHUP by closing the pty master); `release_terminal` in `src/app.rs`.

### Truncate by the width of the string you actually emit
- **Context / symptom:** a track named with repeated `❤️` (VS16 emoji) crashed the TUI with an arithmetic underflow (debug) or `capacity overflow` (release) when its folder was opened.
- **Root cause:** truncation summed per-`char` display widths, which disagree with whole-string width for emoji sequences; the "fitted" text was wider than its column and the padding subtraction underflowed.
- **Rule:** measure the growing prefix (or grapheme clusters) with string width, use saturating arithmetic in layout code, and test truncation against emoji sequences, combining marks and bidi controls from untrusted metadata.
- **Follow-up rule:** never re-measure a whole growing string per character. That is quadratic, and zero-width characters let a crafted tag make it unbounded (3.9 s per frame from a 100 KB tag). Cap untrusted strings at ingestion, cap again at render time, and find the truncation point by binary search over measured prefixes (3.9 s → 60 µs per call).
- **Where verified:** M1 slice 2 review, pty reproduction; `fit` and `row_line` in `src/ui/render.rs`.

### HTTP redirects leak signed URLs through `Referer`
- **Context / symptom:** Subsonic auth puts a replayable token and salt in the query string of every request.
- **Root cause:** reqwest's default redirect policy copies the previous URL, query included, into the `Referer` header sent to the redirect target. `reqwest::Error`'s `Display` also embeds the full URL.
- **Rule:** for APIs that carry credentials in the query string, disable redirects (and test that a 3xx is not followed), call `.without_url()` on every `reqwest::Error`, and hold signed URLs in a type that redacts in `Debug`/`Display`.
- **Where verified:** M2 slice 1 review; reqwest 0.13.5 `src/redirect.rs` (`make_referer`); `redirects_are_not_followed`.

### Render config errors without the library's full `Display`
- **Context / symptom:** a malformed `password = …` line in `config.toml` would be echoed back in the error.
- **Root cause:** toml's `Display` for parse errors prints the offending source line.
- **Rule:** build user-facing parse errors from the error message plus line/column only, and test with a malformed secret line.
- **Where verified:** M2 slice 1; `unquoted_password_is_not_echoed`.

### Clamp third-party HTTP crates in the log filter
- **Context / symptom:** with `CODA_LOG=debug` the log contained `connecting to <server ip:port>`, despite all of Coda's own messages being redacted.
- **Root cause:** hyper-util emits `tracing` events with peer addresses at debug and hosts at trace. A plain `crate=warn` directive is not enough: EnvFilter prefers more specific user directives (`hyper_util::client::legacy=trace`).
- **Rule:** enforce a separate per-target filter layer (ANDed with the user's filter) that drops sub-warn events from HTTP crates, and test it by driving the real layer with hostile `CODA_LOG` values.
- **Where verified:** M2 slice 2 review; `http_crates_are_clamped_to_warn_whatever_coda_log_says`, pty smoke log.

### Every spawned request must answer exactly once
- **Context / symptom:** a fetch task that panicked or was cancelled left its resource "in flight" forever: the view stayed on Loading and `r` could not recover.
- **Root cause:** the response was only sent on the happy path; a guard created inside the task does not exist if the task is aborted before its first poll.
- **Rule:** create a drop guard that sends an error response *before* spawning, consume it on the normal send, and test cancellation before first poll.
- **Where verified:** M2 slice 2 review; `a_cancelled_fetch_still_answers_with_an_error`.

### Cap untrusted JSON by what it parses into, not by file size
- **Context / symptom:** a 64 MiB state file of small objects took 5.3 GB RSS to load.
- **Root cause:** parsing into `serde_json::Value` first costs ~80× the input for small objects; entries past the item cap were fully built before being dropped.
- **Rule:** deserialize untrusted files/bodies into typed structs, probe the version with a minimal struct, enforce item caps inside a sequence visitor (skip the rest with `IgnoredAny`, never preallocate from `size_hint`), and keep byte caps near realistic sizes.
- **Also per item and per field:** parsing each list item, or each "lenient" field, through a `Value` reintroduces the problem one level down: a single item holding `[{"a":0},…]` (in a known field or under an unknown key) cost 1.57 GB for a 16 MiB `getPlaylist` body. Read items with `next_element_seed` into the typed struct, write lenient fields as visitors that accept only the expected scalar and drain arrays and objects with `IgnoredAny`, and copy strings only up to the length kept. After the fix the same body adds about 0.1 MB over reading it.
- **And per response:** capping each list is not enough when lists nest (`artists.index[].artist[].album[].song[]`): the caps multiply, and an empty `{}` still becomes a struct of about 150 B. 24 MiB of nested `{}` took 1.31 GB (8 MiB: 435 MB). Bound the whole response: parse a per-endpoint reply type that drains every other section, and draw every object built, at every level, from one shared item budget (reset by a guard around each parse). After that, the same bodies add about 19 MB.
- **Where verified:** M3 slice 1 review measurement; `queue_is_capped_and_current_beyond_the_cap_is_dropped` in `src/session.rs`. M5 slice 1 review probe; `hostile_nested_values_are_drained_not_built` in `src/navidrome/api.rs`, `a_single_hostile_entry_at_the_cap_is_drained` and `nested_empty_objects_at_each_cap_parse_in_bounded_time` in `tests/navidrome_client.rs`, `one_budget_bounds_every_nesting_level_and_resets_between_parses` in `src/navidrome/api.rs`.

### "Repeat" needs proof of progress, not just a clean end
- **Context / symptom:** under repeat, a track that ended immediately (8-sample WAV, a stream that closes at once) reloaded as fast as mpv could open it; for Navidrome that is a request storm.
- **Root cause:** EOF was trusted as "played"; and resetting the failure counter on `Started` let instant-EOF files reset it forever.
- **Rule:** count an end as successful only after observed progress (position ≥ 1 s after `Started`); otherwise treat it as a failure for termination and advance. Prove termination with tests for all-failing and all-instant queues under every repeat mode.
- **Where verified:** M3 slice 1 review; `files_that_end_at_once_never_loop_under_repeat`.

### Capture wall-clock timestamps when the event happens
- **Context / symptom:** scrobble submissions carried a play-start time that was too late by the length of any system sleep during the track.
- **Root cause:** the start was stored as an `Instant` and converted to wall-clock time at send time; `Instant` does not advance during system sleep on macOS or Linux.
- **Rule:** timestamps destined for external systems are captured as `SystemTime` at the event and carried as data; use `Instant` only for local durations and timeouts.
- **Where verified:** M3 slice 2 review; `Listen::new(load, song_id, SystemTime)`.

### mpv's `end-file eof` is not "the audio finished"
- **Context / symptom:** two real-mpv tests failed on one Mac only, with "unexpected advance" and missing positions; in M3 the watchdog's assumption that "mpv replies at once" turned out to be false right after EOF.
- **Root cause:** mpv reports `end-file eof` when decoding ends, which can be early by up to the audio output's buffer: about 0.3 s for CD-quality audio on CoreAudio, and about 4 s for 8 kHz mono WAVs that CoreAudio rejects and avfoundation plays instead. On the last playlist entry mpv then blocks its core while that buffer drains, so replies to the next commands are delayed by up to the same amount. The buffered audio still plays out, so tracks are not cut off.
- **Rule:** run timing-sensitive real-mpv tests with `--ao=null`; size reply timeouts to cover an audio-buffer drain; never treat EOF as a wall-clock signal.
- **Where verified:** M3 slice 3 root-cause probes (mpv 0.41, `--msg-level=all=v`); reviewer IPC probes; `pause_volume_and_seek_are_reflected_in_events`, `rapid_loads_end_on_the_last_requested_track`.
- **Note (M6):** the avfoundation case predates `--audio-channels=stereo`. Coda now always outputs stereo, so mono reaches CoreAudio rather than avfoundation (seen via `current-ao`; not checked on Bluetooth). The rule still holds: any output can buffer, and the visualizer measures a 2.5–3 s lead on avfoundation.

### A feature that swaps in a mock security backend is hidden by `--all-features`
- **Context / symptom:** the `mock-secret-store` feature, which exists for PTY smoke runs, replaces the OS keychain with an in-memory store. The standard `clippy --all-features` check then linted only the mock path, and a build with all features said "macOS Keychain" while saving nothing.
- **Rule:** also lint and build without features whenever a feature swaps out a shipping backend, and make a mock build announce itself (store name on screen, a warning in the log).
- **Where verified:** M4 slice 1 review; `src/secret_store.rs`, `docs/ARCHITECTURE.md` verification list.

### Setup and startup must derive a secret's key from the same inputs
- **Context / symptom:** with `CODA_NAVIDROME_USER` set, `coda --setup` pinged and stored the password under the typed account, while startup looked it up under the env-merged account. Startup then failed with "Run `coda --setup`", which repeated the failure.
- **Rule:** when overrides can change the identity a secret is stored under, either have setup use exactly the identity startup will compute, or refuse to save while the override is active. Never verify and store credentials that startup will not read.
- **Where verified:** M4 slice 2 Part A review; `Onboarding` blocked-server state, `keeping_navidrome_under_env_overrides_changes_only_the_folder`.

### Swapping the identity behind async work must also cover events that arrive later
- **Context / symptom:** in the in-app account swap, a track resolved with the old client but not yet started at the swap would have been scrobbled to the new server.
- **Root cause:** the swap cleared the listen that existed at swap time, but mpv's `Started` for that load arrived afterwards and created a new one.
- **Rule:** when swapping the identity behind async work, also invalidate events that arrive after the swap for work issued before it. Do this by recording the last pre-swap id, or by tagging work with a generation (as fetches and pings now are).
- **Where verified:** M4 slice 2 Part B review; `a_load_still_loading_at_the_swap_is_not_scrobbled_to_the_new_server`, `the_old_ping_racing_the_swap_is_ignored`.

### Resolve untrusted paths as lookup keys, not as files
- **Context / symptom:** `.m3u` entries are untrusted text naming files relative to the playlist. Lexical normalisation of `link/../x` disagrees with the file system when `link` is a symlinked folder, and resolving with the file system (`canonicalize`) would follow symlinks out of the music folder and break file symlinks the scanner accepts.
- **Root cause:** `..` is only exact lexically while every folder before it is a real folder; the entry's own names may be symlinks, the walked folder and the canonical root are not.
- **Rule:** resolve without touching the file system. Allow `..` only before the first name of a relative entry, relative to a folder known to contain no symlinks (one the scanner walked below a canonical root); refuse `..` after a name and in absolute entries; map foreign separators (`\`) before applying these rules. Use the result only as a key into the set of paths the scan actually found, never as something to open, so the worst an entry can do is select a scanned track.
- **Where verified:** M5 slice 2 review, including a randomised check of 400k entries; `entries_cannot_climb_out_of_the_root`, `windows_separators_and_drive_letters` in `src/library/m3u.rs`, `a_playlist_entry_through_a_symlinked_folder_is_missing` in `tests/library_scan.rs`.

### A local cancel must not race an in-flight commit of the same state
- **Context / symptom:** pressing Esc while a playlist's new order was being saved said "discarded", but the save still landed on the server; a later `J` started a new draft, which the first save's answer then cleared.
- **Root cause:** the draft (local state) and the save (an async commit of that state) were linked only by the playlist id. Discarding dropped the local copy without cancelling the request, and the answer could not tell its draft from a newer one.
- **Rule:** while a commit of some local state is in flight, refuse a local cancel of that state (say it is being saved), and tie the state to the commit's request id so that only that request's answer may end it.
- **Where verified:** M5 slice 3 review; `esc_during_a_save_is_refused_and_only_that_save_ends_the_draft`, `a_stale_answer_for_another_save_does_not_end_a_new_draft` in `src/app.rs`.

### Check what a list API hides before rewriting the list from it
- **Context / symptom:** removing a song from a Navidrome playlist (a replace of the whole list with the songs shown) would also delete entries the user never saw.
- **Root cause:** Navidrome's `getPlaylist` omits entries whose file is missing and songs in libraries the user cannot access, while the stored playlist keeps them; a replace writes only what was shown. Its `songCount` counts every stored entry.
- **Rule:** before rewriting a server-side list from what the API shows, check for entries the API hides (compare the stored count with the returned entries). If some are hidden, say so and require confirmation, and recheck at write time that no more are hidden than the user confirmed.
- **Where verified:** M5 slice 3 re-review, traced in Navidrome's source (`GetWithTracks`, `refreshCounters`); `removing_from_a_playlist_with_hidden_entries_warns_first`, `saving_an_order_over_hidden_entries_needs_a_second_w` in `src/app.rs`, `more_hidden_entries_than_confirmed_on_the_refetch_is_stale` in `tests/navidrome_edit.rs`.

### mpv's `--ao=pcm` renders as fast as it can
- **Context / symptom:** a bit-identity test meant to compare output with and without the visualizer could not exercise Coda's lazy `af add`: a 60 s file had already played to the end about 10 ms after `playback-restart`.
- **Root cause:** the `pcm` output is not timed by a clock; mpv decodes and writes the whole file as fast as it can, so anything Coda sends "while playing" lands after the end, or while idle.
- **Rule:** use `--ao=pcm` only to compare output bytes, with everything under test in place before the load (here, the graph passed at spawn in a test-only option). Test behaviour that depends on playback time with `--ao=null`, which follows mpv's clock.
- **Where verified:** M6 slice 2 IPC probe on mpv 0.41; `visualizer_graph_leaves_the_audio_bit_identical`, `visualizer_tones_peak_in_their_band_when_they_are_heard`.

### mpv answers `seek` with `success` even when it does not seek
- **Context / symptom:** review of M6 slice 2. The visualizer held its readings from the moment a seek was sent until `playback-restart`. A seek that mpv acknowledges but does not perform (a non-seekable stream, beyond what is cached) brings no `seek` event and no `playback-restart`, so the bars would have frozen for the rest of the track.
- **Root cause:** the reply to `seek` only says the command was accepted. Whether a seek happens is reported by mpv's `seek` event, and its end by `playback-restart`.
- **Rule:** never gate state on an expected `playback-restart` after sending a seek. Arm the gate on mpv's own `seek` event. On send, only invalidate what is already in flight. Give anything that waits for readings a timeout of its own: the bars fall after 300 ms without a reading.
- **Where verified:** M6 slice 2 review follow-up; `only_mpvs_seek_event_holds_readings_back`, `a_seek_mpv_does_not_perform_does_not_freeze_the_readings`, `bars_fall_when_readings_stop`.
