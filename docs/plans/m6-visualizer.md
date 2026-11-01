# M6 — Audio visualizer: brief

- **Status:** approved by owner 2026-11-01 ("could we do the audio visualizer milestone first?").
- **Goal:** a small, calm visualizer for the playing track, driven by the actual audio. It must stay within the mpv-over-IPC architecture: no second decoder for playback, and no system audio capture.
- **Delivery:** slice 1 is a timeboxed feasibility spike with written findings. Slice 2 is the feature, briefed in detail from those findings.

## Slice 1 — feasibility spike

### Questions
1. **Levels.** Can mpv report per-channel RMS and peak levels while playing?
   - Candidate: `af=@viz:lavfi=[astats=metadata=1:reset=…]`, then read or observe `af-metadata/viz` over IPC.
   - Measure: the update rate, IPC traffic, and the CPU cost in mpv and in Coda.
2. **Frequency bands.** Can mpv report the levels of about 8–16 frequency bands, with the played audio left bit-identical?
   - Candidate graph: `asplit` → one `bandpass` (or `lowpass`/`highpass`) branch per band → `amerge` the bands as extra channels next to the original → one `astats` with per-channel metadata → `pan` back to the original channel layout.
   - Verify:
     - that the frame metadata survives `pan`;
     - that the output is the untouched original audio;
     - that the latency is not increased;
     - how this works for mono, stereo and 5.1 input.
   - Try other ideas if this fails: `aspectralstats`, `ebur128`, `lavfi-complex`.
3. **Timing.** How far ahead of the speaker is the analysed audio (the audio-output buffer, cf. LESSONS "mpv's `end-file eof` is not the audio finished")? Can Coda compensate, e.g. by delaying values by `audio-out-delay`/`ao-delay`-like properties or by the measured AO buffer? Is the lag visible without compensation?
4. **Lifecycle.**
   - Is the filter added once at start and kept across `loadfile replace`?
   - Can it be toggled at runtime (`af add`/`af remove`) without a click or gap?
   - What happens with streams, with seeks, with pauses, and with odd sample rates, channel counts and codecs?
   - What happens when the ffmpeg build lacks a filter? It must degrade to "no visualizer", never break playback.
5. **Cost.** CPU usage of mpv with and without the filter on this Mac (and in the Linux container if possible), and the Coda-side cost at 30 updates per second.

### Deliverable
- **Findings section.** A "Findings" section appended to this brief, giving for each question the answer, the evidence (commands, numbers, mpv version) and the open risks. End it with a recommendation for slice 2: what the visualizer can show (meter, bands, or both), at what rate, and how it attaches to the backend.
- **Throwaway code.** Prototype code may live in the scratchpad, or behind an `#[ignore]` real-mpv test. Do not change the shipping playback path in the spike, apart from test-only hooks if needed.
- **Tests.** Real-mpv probes use generated test signals (sine sweeps or tones at known frequencies), so the band levels can be checked against expectations.

## Slice 2 — visualizer (briefed after slice 1)
These constraints already hold:
- **Where it shows:** in the now-playing area, toggled with `v` (provisional, settled in M7). Off by default until the owner decides otherwise; the toggle is saved in the session.
- **No work when nothing is shown.** No updates and no redraws while it is hidden, paused or stopped, or while no track is playing. While visible, cap updates at about 30 per second and draw only the visualizer area.
- **Untrusted input.** Metadata values are parsed defensively: bounded numbers, NaN and infinity rejected, missing keys tolerated.
- **Behind the backend boundary.** All of it stays behind the playback boundary (`PlaybackEvent`/`PlaybackCommand`), with no mpv details in the UI.
- **Rendering.** Unicode block characters, with sensible behaviour at small widths and in `NO_COLOR`/16-colour terminals.
