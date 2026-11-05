# Smoke checks (manual): M1 foundation, M4 experience

Run from the repo root: `pnpm tauri dev` (or `pnpm build && cargo run -p coda-app`). Log: `~/.local/state/coda/coda-app/coda-app.log` (or `$XDG_STATE_HOME/coda/coda-app/`). Before and after, `pgrep -l mpv` to count mpv processes (a running Coda TUI has its own).

## Startup
0. While "Starting…" (e.g. during a keychain prompt) the transport, repeat, Clear and remove controls are disabled.
1. With your normal Coda config: the window opens at once ("Starting…" only if the password/keychain is slow), sidebar shows "Scanning… N" then "N tracks"; Navidrome dot turns green; a toast "Library scan complete: …" appears and fades after ~5 s.
2. First run: with no GUI session yet (`~/.local/state/coda/coda-app/session.json` absent), the TUI's queue (`~/.local/state/coda/state.json`) is imported once, paused. Check the TUI's file is unchanged (`shasum` before/after). After step "Queue" below, quit and relaunch: the GUI's own queue comes back, **paused/stopped, not playing**; the TUI's queue is not imported again.
3. Without config: see "Onboarding" below.
4. Without mpv: `PATH=/usr/bin:/bin target/debug/coda-app` with mpv only in `/opt/homebrew/bin` or `/usr/local/bin` → it is found there and plays (mpv discovery). With mpv not installed anywhere (or renamed) → red banner "mpv was not found…"; controls are disabled.
4a. `CODA_MUSIC_DIR=~/SomeOtherMusic target/debug/coda-app` → that folder is scanned instead of the config's; Settings says CODA_MUSIC_DIR is used and the folder button is disabled.

## Playback (needs a queue; browse arrives in M2)
For now a queue comes from a restored GUI session. To seed one, copy the TUI's session once: `cp ~/.local/state/coda/state.json ~/.local/state/coda/coda-app/session.json` (while the GUI is not running), then launch.
5. Space plays/pauses; the play button and the queue's equaliser glyph follow. `n`/`p` and Ctrl+→/← skip; `p` after 3 s restarts the track.
6. Seek bar: drag the thumb → elapsed time follows the drag; release → playback jumps there. ←/→ seek ∓5 s. Click on the bar seeks.
7. Volume slider drag and `+`/`-` change the volume, also while stopped: the slider stays where it was dropped (no snapping back), the next track plays at that volume, and it is restored after a relaunch.
8. `r` cycles Repeat off → all → one (toast + icon; repeat-one shows a "1").
9. Local and Navidrome tracks in one queue both play; the cover shows in the bar (placeholder initials when there is none). Gapless: two consecutive tracks of a gapless album play without a gap.

## Queue
10. `q` (and the bar's queue button) toggles the panel; the choice survives a relaunch.
11. Double-click an entry plays it; click selects; ↑/↓ move, Enter plays, Delete or ⌫ (Backspace) removes; hover shows ×; right-click → Play / Remove. Removing the playing entry moves on to the next.
12. Clear empties the queue and stops playback.

## Quit
13. Close the window (and separately, Cmd+Q on macOS / the WM close on Linux): the app exits within ~2 s; `pgrep -l mpv` shows no new mpv; relaunch restores the queue and position-free current track.
14. Change something (e.g. `r`) and quit within 2 s: the change is still restored (final save).
14a. Signals: while playing, `kill -TERM <coda-app pid>` (also try `-HUP`, and Ctrl-C in the `pnpm tauri dev` terminal): music stops, the app exits within ~2 s, `pgrep -l mpv` shows no new mpv, and the queue is restored on relaunch. A second Ctrl-C while quitting exits at once (mpv still terminated).
14b. Linux only: `kill -KILL <coda-app pid>` while playing: mpv stops too (parent-death signal).
14c. Quit while startup is slow (e.g. a `password_command` that waits on pinentry, or an unanswered macOS keychain prompt): close the window during the prompt; within ~3 s the app exits and no mpv is left.

## Look
15. Dark and light (switch the OS appearance): readable, accent amber, no text selectable outside inputs, no page scroll/bounce, focus rings visible when tabbing.
16. Resize to the minimum 900×600: nothing overlaps; long titles ellipsize.

## Now Playing (`f`)
17. `f` (or a click on the bar's cover) opens the Now Playing view: large cover over a blurred, darkened copy, title, artist — album, timeline (seekable), prev/play/next, visualizer bars and "Up next: …" (or "End of queue"). `f` or Esc (or Close) returns to the previous view; picking a sidebar entry closes it too.
18. Visualizer: bars move with the music, fall smoothly to silence within ~1 s after pause. In the log, `SetVisualizer(true)` is only sent while the view is open; minimise the window (or switch to another macOS Space) → readings stop, restore → they resume. With "Reduce motion" on (macOS Accessibility / GNOME), the bars update a few times a second instead of animating.
19. Nothing playing: "Nothing playing", no visualizer.

## Media controls
20. macOS: Control Center / the menu-bar Now Playing widget shows title, artist, album, cover and a moving position; its play/pause, next/previous and the scrubber work; the keyboard's media keys (F7–F9) work while another app is focused.
21. Linux (Arch, GNOME/KDE): `playerctl -l` lists `coda_app`; `playerctl -p coda_app metadata` shows title/artist/album/`mpris:artUrl` (a `file://…/coda-app/media/cover-N.jpg`); `playerctl -p coda_app play-pause|next|previous|position 30` work; the desktop's media widget and media keys work.
22. Seek in the app → the widget's position jumps too. Quit → the widget entry disappears.

## Settings
23. Sidebar → Settings shows the music folder as written (e.g. `~/Music`), the scan status, the server `user @ url` with its connection state, the two switches and the config file path.
24. Music folder → Change… opens the native folder picker; choosing one saves `music_dir` (check `config.toml`), toasts "Scanning the new music folder" and rescans (sidebar count). Cancel changes nothing.
25. Navidrome → Edit…: wrong password + Test connection → inline error; right one → "Connected". Save → toast "Navidrome saved; connecting", dot goes amber then green; the keychain entry is updated (macOS: Keychain Access; Linux: Secret Service, e.g. Seahorse); playback of the current track is not interrupted; the next remote track plays via the new account.
26. Disconnect → "Click again to disconnect" → click again within 3 s → `[navidrome]` is gone from `config.toml`, the keychain entry too; Navidrome dot grey. With no music folder in the file, Disconnect is refused with a message.
27. Cover art off → covers turn into placeholders everywhere (bar, queue, Now Playing); on → back. Scrobbling off → a Navidrome track played to the end does not appear in Navidrome's "recently played"; on → the next one does. `config.toml` keeps every other setting (and a backup `config.toml.bak.<secs>` appears if the file had hand-written settings).
28. A server set via `CODA_NAVIDROME_URL` or a hand-written `password_command` is shown as set outside Coda, with no Edit/Disconnect.
29. Run the TUI afterwards: it starts with the same folder/server (shared config).

## Onboarding
30. `CODA_CONFIG=/tmp/coda-empty/config.toml target/debug/coda-app` (folder absent) → the "Welcome to Coda" screen instead of the shell. Choose… a folder → Continue → the shell appears and the folder scans; `/tmp/coda-empty/config.toml` holds `music_dir`.
31. Same with only "Connect a Navidrome server": Test connection, Continue → the shell appears, Navidrome connects; a wrong password shows the error and stays on the screen.

## Window
32. Resize and move the window, quit, relaunch: same size and position (also after a maximise).
