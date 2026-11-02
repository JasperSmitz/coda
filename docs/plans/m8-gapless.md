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
Briefed after the spike from its recommendation; see "Recommendation for slice 2" below.
