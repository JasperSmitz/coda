# M1 smoke checks (manual)

Run from the repo root: `pnpm tauri dev` (or `pnpm build && cargo run -p coda-app`). Log: `~/.local/state/coda/coda-app/coda-app.log` (or `$XDG_STATE_HOME/coda/coda-app/`). Before and after, `pgrep -l mpv` to count mpv processes (a running Coda TUI has its own).

## Startup
1. With your normal Coda config: the window opens at once ("Starting…" only if the password/keychain is slow), sidebar shows "Scanning… N" then "N tracks"; Navidrome dot turns green; a toast "Library scan complete: …" appears and fades after ~5 s.
2. First run: the queue is empty (the GUI keeps its own session; the TUI's queue is not imported). After step "Queue" below, quit and relaunch: the same queue and current track come back, **paused/stopped, not playing**.
3. Without config: `CODA_CONFIG=/nonexistent pnpm tauri dev` → amber banner "No music source is set up yet…"; the app stays usable (no crash).
4. Without mpv: `PATH=/usr/bin:/bin pnpm tauri dev` (with pnpm/cargo reachable, or run the built binary `PATH=/usr/bin:/bin target/debug/coda-app`) → red banner "mpv was not found…"; controls are disabled.

## Playback (needs a queue; browse arrives in M2)
For now a queue comes from a restored GUI session. To seed one, copy the TUI's session once: `cp ~/.local/state/coda/state.json ~/.local/state/coda/coda-app/session.json` (while the GUI is not running), then launch.
5. Space plays/pauses; the play button and the queue's equaliser glyph follow. `n`/`p` and Ctrl+→/← skip; `p` after 3 s restarts the track.
6. Seek bar: drag the thumb → elapsed time follows the drag; release → playback jumps there. ←/→ seek ∓5 s. Click on the bar seeks.
7. Volume slider drag and `+`/`-` change the volume (only while a track is loaded; at rest the slider shows the saved volume).
8. `r` cycles Repeat off → all → one (toast + icon; repeat-one shows a "1").
9. Local and Navidrome tracks in one queue both play; the cover shows in the bar (placeholder initials when there is none). Gapless: two consecutive tracks of a gapless album play without a gap.

## Queue
10. `q` (and the bar's queue button) toggles the panel; the choice survives a relaunch.
11. Double-click an entry plays it; click selects; ↑/↓ move, Enter plays, Delete removes; hover shows ×; right-click → Play / Remove. Removing the playing entry moves on to the next.
12. Clear empties the queue and stops playback.

## Quit
13. Close the window (and separately, Cmd+Q on macOS / the WM close on Linux): the app exits within ~2 s; `pgrep -l mpv` shows no new mpv; relaunch restores the queue and position-free current track.
14. Change something (e.g. `r`) and quit within 2 s: the change is still restored (final save).

## Look
15. Dark and light (switch the OS appearance): readable, accent amber, no text selectable outside inputs, no page scroll/bounce, focus rings visible when tabbing.
16. Resize to the minimum 900×600: nothing overlaps; long titles ellipsize.
