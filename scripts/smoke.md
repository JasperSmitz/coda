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

# M2 browse smoke checks (manual)

Same setup as above. A configured music folder and Navidrome server make every section visible; with only one, the other section is hidden from the sidebar.

## Navigation
17. Local → Artists: an alphabetical list with a letter index on the right; clicking a letter jumps there. While the scan runs, a "Scanning the library… N tracks so far" line shows and the list grows (refreshes about every 2 s) without losing the scroll position.
18. Click an artist → artist page (albums grid, Play / Shuffle). Click an album → album page (large cover, artist link, year for Navidrome albums opened from a grid, track count, length; tracks grouped by disc when there are several). Back with Backspace, Alt+←, Escape, the header's back arrow, and the mouse back button: each returns to the previous page at the same scroll position.
19. A sidebar entry always starts a fresh stack (Back does nothing at a sidebar root).

## Lists and grids
20. Local → Albums: grid with covers (placeholder initials without art), Sort A–Z / By artist. Arrow keys move between tiles, Enter opens, right-click or the "…" on hover opens the album menu (Play, Shuffle, Play next, Add to queue, Open album, Go to artist).
21. Navidrome → Albums: all six sorts; scrolling down loads more ("Loading more…") until the end; Random gives a new order each time it is chosen.
22. Folders: sub-folders as tiles, then the folder's own tracks; breadcrumbs go back up. Shuffle (or `s`) plays everything below the folder.
23. Playlists (local `.m3u` and Navidrome): open one; duplicates are kept; a local playlist with missing entries shows an amber note ("N entries not found in the library"); a Navidrome playlist whose server count differs shows the hidden-entries note.

## Tracks and playing
24. In a track table: ↑/↓ select, Enter or double-click plays the list from that track (the queue becomes that album/playlist/folder list); the playing row shows the amber equaliser in every view.
25. Right-click a track or its "…" button: Play, Play next, Add to queue, Go to album, Go to artist (disabled when unknown). Play next / Add to queue show a toast.
26. `s` shuffles the shown context: an album/playlist's tracks, an artist's tracks (all albums), a folder's tracks below it, or "Shuffle all" on the Artists/Albums lists (local: whole library, toast "Shuffling all (N tracks)"; Navidrome: a fresh set of random songs).
27. Go to album / Go to artist from a queue-built context (e.g. after Play next) lands on the right page for both sources.

## Search
28. `/` or Ctrl+F opens Search with the cursor in the field (from anywhere, also again while on Search). Typing searches after a short pause; results are grouped per source into Artists, Albums and Tracks. Enter on a track plays that source's matching tracks from it; Escape leaves the field, a second Escape goes back.
29. A query with no matches shows "No matches"; Navidrome down (stop the server) shows an error with Retry, while local results still show.

## Errors and states
30. Stop the Navidrome server and open Navidrome → Artists: loading skeleton, then "Could not load" with Retry; start the server and Retry loads.
31. Light and dark themes: skeletons, tiles, rows, menus and notes are readable in both; focus rings visible when tabbing through the album grid and lists.
