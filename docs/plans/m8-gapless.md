# M8 — Gapless playback: brief

- **Status:** approved by owner 2026-11-02 ("go ahead, do the spike, and if finished go ahead with development unless there are urgent points to discuss").
- **Goal:** consecutive queue entries play without an audible pause, for local files and Navidrome streams. Everything that works today must keep working: skip, seek, failures, repeat, queue edits, scrobbling, the visualizer, and session restore.
- **Approach ("one ahead"):** this is the one-track lookahead that ADR 0002 already names as its escape hatch.
  - Coda's queue stays authoritative.
  - The mpv backend holds at most two playlist entries: the current track and a copy of the next one.
  - mpv's gapless audio and prefetching handle the change; Coda follows it from mpv's events.
- **Delivery:**
  - **Slice 1** is a timeboxed feasibility spike with written findings.
  - **Slice 2** is the feature. If the spike finds nothing that needs the owner, slice 2 starts right away, briefed from the findings.
- **Out of scope:** crossfade, ReplayGain, and endless play. Endless play only benefits from this work.

## Slice 1 — feasibility spike

### Questions
1. **Today's gap.** How long is the silence between two tracks with `loadfile replace` on EOF?
   - Measure for local FLAC/WAV and MP3, and for a stream from a local HTTP server that imitates Navidrome. A fake Subsonic `stream` endpoint is fine, ideally with some added latency.
   - Method: render with `--ao=pcm` where the timing allows it, or measure the silence in a capture. Use a tone that runs to the very end of track A and starts at the very first sample of track B.
   - Compare against mpv playing both files in its own playlist with `--gapless-audio=yes` (and `weak`, the default) and `--prefetch-playlist=yes`.
2. **Gapless conditions.**
   - What does mpv need for a seamless change: the same sample rate and format, `gapless-audio=yes` vs `weak`, and does `--audio-channels=stereo` (which Coda always sets) help?
   - Does the visualizer `af` graph survive a gapless change without resetting the audio output or adding a gap?
   - Encoder padding: is MP3 (LAME header) and AAC/M4A trimmed so the join is sample-exact? A rough check is enough.
3. **IPC protocol for the lookahead.**
   - `loadfile <url> append` (or `append-play`, or `insert-next` on newer mpv): which reply carries the new `playlist_entry_id` on the minimum supported mpv (0.33) and on 0.41? If 0.33 lacks it, how else can the id be learned reliably (`playlist/N/id`), or must the minimum version go up?
   - The event sequence at a gapless change: `end-file` (reason `eof`) for A, then `start-file` and `file-loaded` for B. When is each emitted relative to when B is *audible*? Recall LESSONS: "mpv's `end-file eof` is not the audio finished".
   - Which property or event marks the audible switch: `playlist-pos`, `playback-time` restarting, `audio-pts`? Coda needs it for the now-playing bar, for scrobbling and for the visualizer time-code.
4. **Changing the next entry.**
   - Replacing or removing the appended entry (`playlist-remove`, `playlist-clear`, `playlist-move`) at different moments: long before the end, during prefetch, and within the last second.
   - Can a race produce a change to the stale next entry, a double play, or a skip? What is the safe protocol? Candidates: "remove then append", or accepting that a change in the last N ms takes effect one track later and correcting it.
   - What happens when the next entry fails to open (a 404 stream, a missing file) during a gapless change? Which events arrive, and does mpv stop or continue?
5. **Coda-side impact.** Reading the code is enough here. List what must change in:
   - `playback::mpv::state` (load ids ↔ entry ids, stale-event filtering);
   - `Player` (who decides "next": an advance without a `Load` command from Coda);
   - `tests/event_ordering.rs` (new interleavings);
   - the visualizer (time-code alignment across a change);
   - scrobbling (`unscrobbled_load`);
   - session restore;
   - repeat-one, which could use mpv's `loop-file`, or keep the current track as its own next entry.

### Deliverable
- **Findings section.** A "Findings" section appended to this brief. For each question give the answer, the evidence (commands, numbers, mpv versions) and the open risks.
- **Recommendation.** End with a recommendation for slice 2:
  - the IPC protocol;
  - the state-machine changes;
  - the minimum mpv version;
  - the test plan;
  - anything that needs an owner decision. Flag it clearly as **Owner decision needed**.
- **Throwaway code.** Prototype code lives in the scratchpad or behind `#[ignore]` real-mpv tests. The shipping playback path does not change in the spike.
- **Versions.** Test on mpv 0.41 (macOS, installed). Test on an older mpv (Debian's 0.35 in a container) if practical; 0.33 itself may not be available.

## Slice 2 — gapless playback
**Architect decisions (2026-11-02), made under the owner's "continue unless urgent"; the owner can still revert them:**
- **A queue change in the switch window: option (a), correct it.** The queue stays authoritative: a track the user removed must never play. The cost (up to about 0.25 s of the ending track cut and about 70 ms of silence on CoreAudio) is paid only when an edit lands in that window.
- **Visualizer: the two-aligner handover**, not the fallback. The bars must not drop at every gapless change, which is exactly when the owner listens to a segued album.
- **Sample rate:** keep `--gapless-audio=weak`, accepting about 70 ms at a sample-rate change.

Implement "Recommendation for slice 2" below as written, plus the decisions above.

**ADR:** add `docs/decisions/0005-one-ahead-lookahead.md`, amending ADR 0002. Coda's queue stays authoritative, and mpv holds the current track and a copy of the next one. Mark ADR 0002 "amended by 0005".

**Acceptance:**
- **Seamless joins:** consecutive local tracks render bit-identically across the join (`--ao=pcm`), and a stream from a fake server joins without silence with prefetch.
- **Everything else still works:**
  - skip, previous, seek and pause;
  - Enter or `s` replacing the queue;
  - every queue edit and repeat mode;
  - Stop and quit;
  - a failing or missing next track (existing advance-on-error and failure counting);
  - instant-EOF loops still terminate;
  - an account swap;
  - session restore.
- **Audible switch:** the now-playing bar, scrobbling and the visualizer switch when the next track is heard, not when mpv starts reading it.
- **Logs:** log privacy is unchanged. `loadfile … append` is sensitive like `replace`.
- **Fallback:** a missing entry id in a reply turns the lookahead off for the session, which is logged once.
- **Tests:** as in the recommendation's test plan, including the new `tests/event_ordering.rs` interleavings.

## Findings (slice 1 spike, 2026-11-02)

### Setup
- **mpv versions:**
  - **macOS:** mpv 0.41.0 with FFmpeg 9.0.2 (Homebrew) on an Apple M1 Pro. It was run with `--ao=coreaudio --volume=0` (silent), `--ao=null` and `--ao=pcm`.
  - **mpv 0.35.1:** `debian:bookworm-slim` (arm64). Runs used `--ao=null` and `--ao=pcm`. A PulseAudio null sink was tried, but the daemon exited whenever its client disconnected, so this version has no captures.
  - **mpv 0.33.1:** `ubuntu:21.10` (arm64) from `old-releases.ubuntu.com`. Runs used `--ao=null` and PulseAudio with a null sink, captured from the sink's monitor with `parec`.
    - Its `--ao=pcm` is unusable: blocks of silence replace audio even when playing a single file. So sample exactness is not checked on 0.33.
    - `ubuntu:21.04` has 0.32.0, which is below the minimum and was not used. mpv 0.33.0 itself was not obtainable.
- **mpv arguments:** every probe spawns mpv with Coda's base arguments, including `--audio-channels=stereo`, and drives it over JSON IPC from Python.
- **Probe files:** throwaway probes are in `scratchpad/m8/probe/` (`mpvprobe.py`, `q1_gap.py`, `q2_viz.py`, `q3_ids.py`, `q4_change.py`, `q4_order.py`, `fake_stream.py`, `analyze.py`, `align.py`, `render.sh`). No Coda source or test files were changed.
- **Signals (ffmpeg only):**
  - A 6 s, 1 kHz stereo sine and 6 s of seeded white noise (low-passed at 6 kHz), each cut at sample 132300 into `a`/`b` (3 s each). Played back to back, the halves rebuild the uncut reference exactly.
  - The halves are encoded as WAV `s16`, FLAC, MP3 (LAME, 192k), AAC in `.m4a` (ffmpeg's encoder) and Opus.
  - Variants of `b`: 48 kHz, 24-bit FLAC (`s32`) and mono.
- **The stream server:** `fake_stream.py`, a fake Subsonic `GET /rest/stream?id=…` on 127.0.0.1 with Range support, a configurable first-byte delay (0.3 s and 1.0 s) and 404 for unknown ids.
- **Gap measurement:**
  - **Sample exactness:** `--ao=pcm` renders of both files in mpv's own playlist, compared with the uncut reference. Lossless files are compared byte for byte. Lossy files are compared by the lag of the noise against the reference before and after the join (correlation over 512 samples, ±2200).
  - **Time gaps:** `--ao=pcm` cannot show them, because it renders as fast as it can (LESSONS). They are estimated from mpv's own audio clock. For each track, the offset between wall time and `audio-pts` is the median over 5 property changes. The gap is then B's offset − (A's offset + 3.0 s).
    - This is what mpv believes reaches the output, including the latency the output reports. It is not an acoustic measurement.
    - On 0.33.1 it agrees with a real capture: the PulseAudio capture shows 1920 ms of silence, and mpv's clock says 1930 ms.

### Q1: Today's gap
- **Answer:** today's `loadfile replace` on `end-file eof` leaves an audible gap. Within mpv's own playlist, the change is seamless.
  - **Local files:** about 40–70 ms on mpv's clock.
  - **Streams:** about 0.4 s with 0.3 s first-byte latency.
  - **PulseAudio null-sink capture:** 1.9 s.
  - **In mpv's playlist:** 0 ms for local files. Streams are seamless only with `--prefetch-playlist=yes`.
- **Gap by case** (ms, mpv's clock unless marked):

  | Case | 0.41 CoreAudio | 0.41 `null` | 0.35 `null` | 0.33 `null` | 0.33 PulseAudio capture |
  |---|---|---|---|---|---|
  | `replace` on EOF, WAV | 71 | 62 | 70 | 39 | 1920 (capture), 1930 (clock) |
  | `replace` on EOF, MP3 | | | 19 | 40 | 1914 (capture) |
  | `replace` on EOF, stream with 0.3 s latency (MP3/FLAC) | 389/390 | 384/359 | | | |
  | `append`, local WAV/MP3 | 0 | 0 | 0 | 0 | no silent run at the join |
  | `append`, stream with 0.3 s latency, no prefetch | 115/103 | 0 | | | |
  | `append`, stream with 1.0 s latency, no prefetch | 808/840 | | | | |
  | `append`, stream with 1.0 s latency, `--prefetch-playlist=yes` | 0/1 | | | | |

- **Why `replace` is slow:**
  - After the last entry's `end-file`, mpv drains the output before handling the next command. The `loadfile` reply took 228–433 ms.
  - `end-file` A to `playback-restart` B took 274–460 ms for local files and about 600–760 ms for the 0.3 s stream.
- **The PulseAudio figure** is mostly the null sink starting a new stream. As a baseline, `paplay a.wav; paplay b.wav` left 1307 ms of silence. Real PulseAudio/PipeWire hardware sinks were not measured.
- **Prefetch:**
  - Without prefetch, mpv opens the next stream only at A's decode end, so the output buffer (about 0.2 s on CoreAudio) has to hide the first-byte latency.
  - With `--prefetch-playlist=yes`, B was requested about 1 s after A, as soon as A was fully cached. A large track is also cached quickly, because the demuxer cache holds up to 150 MiB by default.

### Q2: Gapless conditions
- **`--gapless-audio`:** `weak` (the default) is right. These figures are mpv's clock on 0.41 CoreAudio.

  | B's change from A | `weak` | `yes` |
  |---|---|---|
  | None (same format) | 0 | 0 |
  | `s16` → `s32` (24-bit FLAC) | 0 | 0 |
  | Stereo → mono | 0 | 0 |
  | 44.1 → 48 kHz | 69 ms (the output reopens) | 0 |

  - **Mono:** the stereo → mono change is seamless because of `--audio-channels=stereo`, which Coda always sets.
  - **Why not `yes`:** it keeps the output at the first file's rate. B played at 44100, and so did a later `loadfile replace` of a 48 kHz file, so the rest of the session is resampled to the first track's rate.
- **Visualizer:**
  - A graph added with `af add` during A is still in `af` after the change, with no added gap (0 ms, and 66 ms for the rate change).
  - Rendered with `--ao=pcm`, WAV A+B through the graph is bit-identical to the reference.
  - The time code restarts at 0 for B, because the graph follows B's timestamps. This was seen on 0.41 and 0.35.
  - B's readings (time code 0.09…) appear about 0.25–0.3 s before B is audible, while `audio-pts` is still between −0.32 and −0.19.
  - A's readings stop at its decode end, with the time code at 3.0.
- **Encoder padding** (`--ao=pcm`, mpv's playlist, the same for `yes`/`weak`/`no`):
  - **WAV and FLAC:** bit-identical to the uncut reference on 0.41 and 0.35.
  - **MP3 (LAME header):** sample-exact on 0.41 and 0.35. The length is exactly 264600 frames, with lag 0 on both sides of the join (correlation 1.000).
  - **Opus:** the length is exact (288000 frames at 48 kHz).
  - **AAC `.m4a` (ffmpeg encoder):** not sample-exact, but no silence at the join.
    - On 0.41 with FFmpeg 9, each file loses its first 1024 samples (23 ms), so 23 ms of B is skipped at the join. The ffmpeg CLI decodes exactly 132300 samples.
    - On 0.35 with FFmpeg 5.1, B is late by 820 samples (19 ms): padding that is not trimmed. The total length is 266240 frames.
    - This is how mpv and FFmpeg handle a single file; Coda cannot fix it. iTunes-encoded files (`iTunSMPB`) were not tested.

### Q3: IPC protocol for the lookahead
- **Entry ids:** the `loadfile` reply carries the new entry id, `{"data":{"playlist_entry_id":N}}`. This holds for `replace` and `append` on 0.33.1, 0.35.1 and 0.41.0.
  - `get_property playlist/1/id` also works.
  - `insert-next` exists only on 0.41: 0.33 and 0.35 answer `invalid parameter`. It is not needed.
  - The minimum version does not need to rise. mpv 0.33.0 could not be tested.
- **Event sequence at a gapless change** (the same on all three versions):
  1. `end-file` A (`eof`), then `start-file` B in the same millisecond, then the `playlist-pos` change.
  2. `file-loaded` and `playback-restart` B, 12–50 ms later for local files. For a stream without prefetch, the delay is its open time (about 315 ms).
- **Audibility:**
  - All of these events come before B is audible: about 0.25 s earlier on CoreAudio and about 0.34 s with `ao=null`.
  - Following LESSONS, `avfoundation` would make this seconds.
  - None of these events marks the audible switch, and neither does `playlist-pos`.
- **The audible marker:** the first `time-pos` > 0 after B's `file-loaded`.
  - **On 0.41:** `time-pos` is reported as `0.0` at B's `file-loaded`, and then not again until it is positive. The first positive value, 0.112, arrived at 3.150 for an audible start at about 3.04.
  - **On 0.33 and 0.35:** `time-pos` (and `audio-pts`) are negative during the overlap, from −0.33 up to 0.
  - **On all three:** `audio-pts` is negative during the overlap.
  - **Using it:** Coda already observes `time-pos`. The audible switch is at the arrival time minus the value.

### Q4: Changing the next entry
- **The safe protocol:**
  - The safe sequence is `playlist-clear` (it keeps only the playing entry), then `loadfile <C> append`. It was tested on 0.33.1, 0.35.1 and 0.41.0.
  - **Before A's decode end** (a change 1.0, 0.5 or 0.35 s before the audible end on CoreAudio):
    - C plays next, gaplessly;
    - B never gets an event.
  - **After A's decode end** (in the last ≈0.25 s on CoreAudio, or right after `end-file` A):
    - mpv has already moved to B;
    - the clear keeps B as the playing entry;
    - B plays in full, then C.
- **The outcome can be read off the socket order:**
  - C was used exactly when the reply to `playlist-clear` arrived before `end-file` A.
  - This held in 50 randomised trials (`q4_order.py`; 30 on 0.41 and 20 on 0.33): 16 times C, 34 times B, no exception.
  - So the backend knows which entry mpv moved to from its *acknowledged* playlist state at `end-file`, and the `start-file` id confirms it. There is no guessing.
- **`playlist-remove 1` is unsafe:**
  - After the advance, index 1 is the playing B.
  - B ends with `stop` and mpv goes idle; C never plays.
  - Seen on 0.33, 0.35 and 0.41. Never address the next entry by index.
- **Playlist size:** after a change, `playlist-clear` also removes the old entry, so mpv's playlist stays at no more than 3 entries.
- **Correcting a stale successor:** a `loadfile C replace` sent on B's stale `start-file`:
  - cuts A's buffered tail by 242 ms (CoreAudio) or 382 ms (`null`);
  - then leaves 69 ms or 47 ms of silence before C.
- **When the next entry fails to open** (a 404 stream or a missing local file, with or without prefetch):
  - The failure is reported only at the change: `end-file` A `eof`, `start-file` B, `end-file` B `error` (`file_error: "loading failed"`), then `idle`.
  - A 404 behind 0.3 s of latency reported its error about 310 ms after `start-file` B. A missing file reported it at once.
  - mpv stops; it does not skip ahead.
  - With prefetch, the 404 URL was requested twice: once for the prefetch and once at the change.
- **Prefetch and the server:**
  - A prefetched stream that is then cleared costs one wasted request: `id=b` was fetched, and then `c` was played.
  - With prefetch, Navidrome sees the next track's stream request early, as soon as the current track is cached. Scrobbling is unaffected, because it uses its own API calls.

### Q5: Coda-side impact (from reading the code)
- **`playback::mpv::state`:**
  - Today a load waits for a separate `get_property playlist/0/id`. That id is wrong for an appended entry; take the id from the `loadfile` reply instead.
  - Keep an acknowledged view of mpv's playlist: the current entry and the appended next entry, each with its `LoadId`, updated on replies.
  - At `end-file eof` of the current entry, the successor is the acknowledged next entry.
  - Buffering of unmapped events must treat an in-flight append as "a mapping is pending". The "an unrequested `start-file` is stopped" rule still holds, because an appended entry is requested.
  - The position coalescer and `opening` already reset at the file events. The new parts are the audible marker and not treating `end-file eof` with a successor as the end of playback.
- **`Player`:**
  - Today only `TrackEnded{Eof|Error}` of the current load advances, and the advance emits `Load`.
  - With the lookahead, mpv advances by itself. The Player must:
    - compute and announce the next entry (queue order, repeat mode, edits, shuffle, clear);
    - accept an advance that it did not command;
    - re-announce the next entry after it.
  - The failure and suspect-end counting (`proven`, `failures_in_a_row`) must also run on gapless advances, otherwise a queue of instant-EOF files would play through without counting.
- **`tests/event_ordering.rs`:** the `Sim` must model mpv's two-entry playlist. The new interleavings are listed in the test plan below.
- **The visualizer:**
  - `track_change` runs only on `Load` and `Stop`, which is fine, but `started`/`ended` would switch to B at B's `file-loaded`. That is about 0.25 s before B is heard (seconds on `avfoundation`).
  - B's readings (time code from 0) would wait for the clock, and A's last readings would be dropped, so the bars fall during A's tail.
  - The graph must never be removed at a gapless change: `af remove` drops 23–55 ms of audio.
- **Scrobbling:**
  - `listen_started` keys on `Started{load} == current_load`, and `unscrobbled_load` on load ids. Both work unchanged if `Started` for the successor is emitted at the audible switch and the Player has already made it current.
  - A successor whose stream URL was signed by the old client must be re-resolved, or cleared, at a client swap.
- **Session restore:** unaffected. It saves the queue and the current entry and does not play. The current entry moves at the advance, which can be up to the output buffer early.
- **Repeat-one:** both `loop-file=inf` and appending the same file are gapless (0 ms on mpv's clock with CoreAudio and `null`). But `loop-file` sends only `seek` and `playback-restart`, with no file events, which breaks:
  - per-play `LoadId`s;
  - scrobbling each repeat;
  - suspect-end counting;
  - the visualizer, whose `seek` handling forgets its readings.

### Open risks
- **Not measured acoustically:** the time gaps come from mpv's clock. The one real capture, on a PulseAudio null sink, agrees with it.
- **Not measured on real devices:**
  - Linux hardware sinks and PipeWire;
  - Bluetooth;
  - `avfoundation`.
  - On slow outputs the switch window, the visualizer overlap and "`Started` early" all grow to seconds.
- **AAC/M4A joins** are off by about 20 ms. This comes from mpv and FFmpeg, not from Coda.
- **Seeks in the switch window:** a seek sent after mpv has moved to B (in A's last ≈0.25 s) applies to B and flushes A's tail. This is rare, and it can be handled by treating seeks between the advance and B's audible start as no-ops.

## Recommendation for slice 2
- **IPC protocol:**
  - Add `--prefetch-playlist=yes` at spawn. Keep `--gapless-audio` at its default, `weak`.
  - **Current track:** `loadfile <t> replace`, as today. Take the entry id from the reply's `playlist_entry_id` instead of `get_property playlist/0/id`.
  - **Setting the next entry:** send `playlist-clear`, then `loadfile <next> append`, back to back. Map the reply's entry id to the next entry's `LoadId`.
  - **Clearing the next entry:** `playlist-clear` alone.
  - **Never** use `playlist-remove <index>`, `playlist-move` or `insert-next`.
  - **Collapsing:** keep at most one set-next in flight; later ones collapse to the latest, like queued loads. Send it only once the current entry is mapped.
  - **Local files:** prechecked before `append`, like loads. A rejected next entry is simply not appended, and the old replace path reports it when the current track ends.
  - **Fallback:** if a reply has no entry id (for example an untested 0.33.0), disable the lookahead for the session and keep today's behaviour.
- **State machine (`state.rs`):**
  - Track the acknowledged mpv playlist: the current entry and the next entry, each with its load id.
  - **At `end-file eof` of the current entry:**
    - With an acknowledged next entry, emit a new `PlaybackEvent::Advanced { from, to }` instead of `TrackEnded`. mpv's `start-file` for `to` follows; if another id arrives instead, treat it as an unrequested entry.
    - With no next entry, emit `TrackEnded { Eof }` as today.
  - **`Started { to }`:** emit it at the audible marker: the first `time-pos` > 0 after its `file-loaded`.
    - Otherwise emit it just before its `end-file`, or before the next `start-file`, so that a very short track gives `Started` then `TrackEnded`.
    - Loads made with `replace` keep `Started` at `file-loaded`.
  - **Failures:** a successor's `end-file error` is a `TrackEnded { to, Error }` after `Advanced`.
- **`Player`:**
  - **`PlayerCommand::SetNext(Option<(LoadId, TrackSource)>)`:**
    - It is emitted whenever the next entry changes; the app resolves it like `Load`.
    - The next entry is the queue's next, or the first entry under repeat All at the end.
    - Under repeat One it is the current entry, and only once the current load is proven (≥ 1 s), so instant-EOF files still cannot loop fast.
  - **On `Advanced { from, to }`** where `from` is current and `to` is the announced next:
    - make `to`'s entry current, with `started = false`;
    - count a failure if `from` was not proven;
    - stop if the failure rule says so;
    - announce the next entry after it.
  - **The now-playing bar** switches at `Started { to }`.
  - **A stale `to`** (the queue changed and mpv already moved on): see the owner decision below.
- **Visualizer:**
  - Never remove the graph at a gapless change.
  - At `Advanced`, keep showing A's queued readings against A's clock, extrapolated from A's last `time-pos`.
  - Tag polls after `start-file` B as B's, in a second aligner, and swap at `Started { to }`.
  - A simpler fallback is to forget at `Advanced`. The bars then fall for about 0.25 s, or seconds on `avfoundation`.
- **Scrobbling:** no logic change if `Started` arrives at the audible switch and the Player made `to` current at `Advanced`. Clear or re-resolve `SetNext` at a client swap.
- **Repeat-one:** append the same source as the next entry, with a fresh `LoadId` for each play. Do not use `loop-file`.
- **Minimum mpv:** stays at 0.33 (verified on 0.33.1, 0.35.1 and 0.41.0), with the fallback above. No owner decision is needed.
- **Test plan:**
  - **Unit tests in `state.rs`:**
    - the set-next request order;
    - the id taken from the reply;
    - the successor decided by whether the replies arrive before `end-file`;
    - the audible marker with negative `time-pos` (0.33/0.35) and with no update at 0 (0.41);
    - a very short successor;
    - a successor's error after `Advanced`;
    - set-next collapsing;
    - `Stop` clearing the next entry;
    - no `af remove` at a gapless change.
  - **Unit tests in `Player`:**
    - `SetNext` for every repeat mode and queue edit;
    - `Advanced` for a non-current load is ignored;
    - all-failing and all-instant queues still terminate under every repeat mode when advances are gapless.
  - **New interleavings in `tests/event_ordering.rs`:**
    - set-next replies before or after `end-file` A, against `start-file`, `file-loaded` and `time-pos` of B;
    - a queue edit (removing the next entry, or play-next) at every point around the change;
    - a user skip (`Load` with `replace`) racing a gapless advance, which must end on the requested track while B's events are stale;
    - `Stop` racing an advance;
    - a 404 or missing successor at every point;
    - repeat-one with instant files;
    - several set-next commands collapsing.
  - **Real-mpv `#[ignore]` tests:**
    - split WAV halves through the backend render bit-identically with `--ao=pcm`;
    - the event sequence and audible marker with `--ao=null`;
    - a set-next race with `--ao-null-buffer`;
    - a stream from a local fake server with prefetch;
    - the visualizer stays attached across a change.
  - **Manual:** listen to an album with segued tracks on CoreAudio.

### Owner decision needed
**A queue change in the switch window.** In the switch window, mpv has already started the old next track:
- the last ≈0.25 s of a track on CoreAudio;
- ≈0.35 s with `ao=null`;
- seconds on slow outputs such as `avfoundation` or Bluetooth.

The changes concerned are removing, moving or replacing the next entry, play-next, shuffle and a repeat change. The options:
- **(a) Correct it (recommended).** Coda sees the stale `start-file` and loads the right track with `replace`. This cuts up to that window of the ending track's tail (measured 242 ms on CoreAudio) and adds about 70 ms of silence. This happens only when an edit lands in the window.
- **(b) Accept it.** The stale track plays in full, and the change takes effect one track later. This is wrong when the user has just removed that track, which then plays even though it is no longer in the queue.

### Trade-off recorded (no decision needed unless the owner objects)
**Sample-rate changes keep a gap of about 70 ms.** Coda stays on `--gapless-audio=weak`:
- A change of sample rate between tracks (for example 44.1 to 48 kHz) keeps this short gap while the output reopens.
- `yes` would remove it, but it resamples every later track to the first track's rate for the whole session.
- Albums normally share one rate, so the transitions where gapless matters stay seamless.
