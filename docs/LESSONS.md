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
