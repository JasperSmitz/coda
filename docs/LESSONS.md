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
- **Where verified:** M3 slice 1 review measurement; `queue_is_capped_and_current_beyond_the_cap_is_dropped` in `src/session.rs`.

### "Repeat" needs proof of progress, not just a clean end
- **Context / symptom:** under repeat, a track that ended immediately (8-sample WAV, a stream that closes at once) reloaded as fast as mpv could open it; for Navidrome that is a request storm.
- **Root cause:** EOF was trusted as "played"; and resetting the failure counter on `Started` let instant-EOF files reset it forever.
- **Rule:** count an end as successful only after observed progress (position ≥ 1 s after `Started`); otherwise treat it as a failure for termination and advance. Prove termination with tests for all-failing and all-instant queues under every repeat mode.
- **Where verified:** M3 slice 1 review; `files_that_end_at_once_never_loop_under_repeat`.
