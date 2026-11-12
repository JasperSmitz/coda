# Smoke checks (manual, owner): Coda App at runtime

Agents cannot open windows or play audio on the owner's desktop, so these checks are the owner's. Start with the hand-off list below; the numbered sections after it are the detailed checks per feature (M1 foundation, M2 browse, M3 queue and playlists, M4 experience, feedback round 1).

## Setup (from the monorepo)

```sh
cd apps/coda-gui                     # in the Coda repository
pnpm install --frozen-lockfile
pgrep -x mpv                         # note any mpv already running (e.g. a terminal `coda`)
pnpm tauri dev                       # debug build; or a release-like binary:
# pnpm tauri build --no-bundle -- --profile release-gui && ../../target/release-gui/coda-app
```

The Cargo target dir is at the repository root (`../../target`); a debug binary with the frontend embedded is `../../target/debug/coda-app` after `pnpm tauri build --debug --no-bundle` (items below that start it directly with environment variables use it). Log: `~/.local/state/coda/coda-app/coda-app.log` (or `$XDG_STATE_HOME/coda/coda-app/`). The config (`~/.config/coda/config.toml`), the keychain entry and the cover cache are shared with the terminal player; the GUI's session and history are its own (`…/coda/coda-app/`).

## Hand-off checklist
- [ ] **Launch:** the window opens; the sidebar shows the scan count, and the Navidrome dot turns green when a server is configured. The log has no errors.
- [ ] **Navigation:** Local → Artists → an artist → an album; Back returns; Albums grid, Folders and Playlists open; `/` searches (items 33–45).
- [ ] **Local playback:** double-click or Enter on a local track plays it; the bar shows the title, the cover and a moving timeline; gapless between tracks of one album (items 5, 9).
- [ ] **Navidrome:** Settings → Navidrome → Edit…/Connect: a wrong password shows an error, the right one "Connected"; a Navidrome album plays (stream) and appears in the queue next to local tracks (items 9, 25, 37).
- [ ] **Queue:** add, play next, reorder by drag, remove, clear (asks first), shuffle upcoming (items 10–12, Q1–Q5).
- [ ] **Controls and seek:** Space, `n`/`p`, ←/→ (∓5 s), drag and click on the seek bar, volume slider and `+`/`-` (also while stopped), repeat `r` (items 5–8).
- [ ] **Visualizer:** `f` opens Now Playing; bars move with the music and fall to rest on pause (item 18).
- [ ] **Lyrics mode:** in Now Playing, `l` moves the player left and shows the lyrics; the highlight follows the song (items L1–L8).
- [ ] **Artwork:** covers in the bar, queue, album grid and Now Playing; Settings → Cover art off shows placeholders, on brings them back (items 9, 27).
- [ ] **Settings persistence:** change the music folder, cover art or scrobbling, quit, relaunch: the settings, the queue, the volume and the window size are restored (items 7, 13, 23–27, 32).
- [ ] **Quit, no mpv left:** close the window while playing: the app exits within ~2 s and `pgrep -x mpv` shows nothing new (also after `kill -TERM`, and on Linux `kill -KILL`, items 13–14b).

## Startup
0. While "Starting…" (e.g. during a keychain prompt) the transport, repeat, Clear (broom) and remove controls are disabled.
1. With your normal Coda config: the window opens at once ("Starting…" only if the password/keychain is slow), sidebar shows "Scanning… N" then "N tracks"; Navidrome dot turns green; a toast "Library scan complete: …" appears and fades after ~5 s.
2. First run: with no GUI session yet (`~/.local/state/coda/coda-app/session.json` absent), the TUI's queue (`~/.local/state/coda/state.json`) is imported once, paused. Check the TUI's file is unchanged (`shasum` before/after). After step "Queue" below, quit and relaunch: the GUI's own queue comes back, **paused/stopped, not playing**; the TUI's queue is not imported again.
3. Without config: see "Onboarding" below.
4. Without mpv: `PATH=/usr/bin:/bin ../../target/debug/coda-app` with mpv only in `/opt/homebrew/bin` or `/usr/local/bin` → it is found there and plays (mpv discovery). With mpv not installed anywhere (or renamed) → red banner "mpv was not found…"; controls are disabled.
4a. `CODA_MUSIC_DIR=~/SomeOtherMusic ../../target/debug/coda-app` → that folder is scanned instead of the config's; Settings says CODA_MUSIC_DIR is used and the folder button is disabled.

## Playback (needs a queue; browse arrives in M2)
For now a queue comes from a restored GUI session. To seed one, copy the TUI's session once: `cp ~/.local/state/coda/state.json ~/.local/state/coda/coda-app/session.json` (while the GUI is not running), then launch.
5. Space plays/pauses; the play button and the queue's equaliser glyph follow. `n`/`p` and Ctrl+→/← skip; `p` after 3 s restarts the track.
6. Seek bar: drag the thumb → elapsed time follows the drag; release → playback jumps there. ←/→ seek ∓5 s. Click on the bar seeks.
7. Volume slider drag and `+`/`-` change the volume, also while stopped: the slider stays where it was dropped (no snapping back), the next track plays at that volume, and it is restored after a relaunch.
8. The repeat button right of the timeline's total time (and `r`) cycles Repeat off → all → one: muted icon → accent icon → accent icon with a "1"; the tooltip names the mode and the next one; **no toast**. The mode is restored after a relaunch.
9. Local and Navidrome tracks in one queue both play; the cover shows in the bar (placeholder initials when there is none). Gapless: two consecutive tracks of a gapless album play without a gap.

## Queue
10. `q` (and the bar's queue button) toggles the panel; the choice survives a relaunch.
11. Double-click an entry plays it; click selects; ↑/↓ move, Enter plays, Delete (fn+⌫) or Cmd+⌫ removes; plain ⌫ (Backspace) is Back and removes nothing; hover shows ×; right-click → Play / Remove. Removing the playing entry moves on to the next.
12. The queue header shows three icon buttons with tooltips: Shuffle upcoming, Save queue as playlist (playlist + plus; only with Navidrome set up), Clear queue (broom). Clear asks first (Cancel/Esc keeps everything); Clear in the dialog empties the queue and stops playback.

## Queue editing (M3)
Q1. Multi-select: click, Shift-click (range), Cmd-click (macOS) / Ctrl-click (Linux) toggles; Shift+↑/↓ extends; Cmd/Ctrl+A selects all. The header says "N of M tracks selected". Delete or Cmd+⌫ removes the whole selection (plain ⌫ goes Back, the queue is untouched); if it held the playing track, the next *remaining* track plays (no blip of a removed one).
Q2. Drag one entry (or a selection) up and down: an amber line shows the drop gap, dragged rows dim; near the top/bottom edge the list autoscrolls (try a queue of a few hundred tracks); Esc during the drag cancels; release drops there. The playing entry keeps playing; drag the track after the playing one elsewhere and let the current one end: the new next one follows gaplessly (check the log for `SetNext`).
Q3. Alt+↑/↓ moves the selection one place (a scattered selection becomes one block).
Q4. Right-click → "Play N tracks next" puts them right after the current entry; "Remove N tracks"; "Add to playlist…" (Navidrome tracks only).
Q5. Shuffle upcoming (the shuffle icon in the queue header): the entries after the current one are shuffled (toast "Shuffled N upcoming entries"); the current and earlier ones stay.
Q6. Save queue as playlist (the playlist-plus icon; Navidrome configured, queue with Navidrome tracks): the dialog suggests `Queue <today>` and says how many local tracks are left out; Enter saves → toast "Created … with N songs (…)"; the playlist appears in Navidrome → Playlists (also in Navidrome's web UI). With only local tracks the button is disabled.

## Playlists (M3, Navidrome)
P1. Playlists → "New playlist…" → name → Create: toast, the list reloads with it. A blank name cannot be submitted; Esc cancels.
P2. On one of your playlists, "…"/right-click → Rename… (or F2): new name shows in the list (and in Navidrome's web UI). Delete… (or Delete/Cmd+⌫): a red confirmation with Cancel focused; clicking Delete removes it; from the playlist page it goes back to the list.
P3. A playlist owned by another user (or a smart playlist): a Read-only tag (hover: the reason), no Rename/Delete/drag; the page shows the reason.
P4. Track context menu anywhere (album, search, queue) → "Add to playlist…": a picker of your editable playlists with a filter field (type, ↑/↓, Enter) and "New playlist…" first; adding toasts "Added N songs to …" and the playlist page shows them (reload is automatic). Local tracks: the item is disabled.
P5. On your playlist's page: drag tracks to reorder (saved at once, "Saving…" while it runs, toast "Saved the new order of …"), Alt+↑/↓ too; select several and press Delete (or Cmd+⌫) → a confirmation (Cancel focused; Enter cancels) → Remove → toast "Removed N songs from …". Plain ⌫ with a track selected goes Back and removes nothing. Alt+↓ three times in a row moves the selected track three places (the selection stays after each save). With the network off, a drag shows the failure toast and the old order comes back. Removing every song is refused ("delete the playlist instead").
P6. Stale: open a playlist, change it in Navidrome's web UI, then reorder or remove here → toast "The playlist changed on the server; reloaded" and the page shows the server's list; nothing was overwritten.
P7. Hidden entries: a playlist whose file for one entry was deleted (Navidrome shows "hides 1 entry"): a reorder or removal first asks in a dialog that names what may be lost; Cancel writes nothing; confirming saves.
P8. Quit while a large add runs: the app waits (≤3 s) for it, then exits; no mpv left; an edit queued behind it does not start.
P10. Start an add, and while it runs disconnect the server in Settings and queue another edit: the running one ends with "(made with the previous server connection)", the queued one says no server is configured and writes nothing.
P9. Keyboard only: every dialog takes Enter/Esc, Tab stays inside it, focus returns to the list afterwards; Space/n/p do nothing while a dialog is open.

## Quit
13. Close the window (and separately, Cmd+Q on macOS / the WM close on Linux): the app exits within ~2 s; `pgrep -x mpv` shows no new mpv; relaunch restores the queue and position-free current track.
14. Change something (e.g. `r`) and quit within 2 s: the change is still restored (final save).
14a. Signals: while playing, `kill -TERM <coda-app pid>` (also try `-HUP`, and Ctrl-C in the `pnpm tauri dev` terminal): music stops, the app exits within ~2 s, `pgrep -x mpv` shows no new mpv, and the queue is restored on relaunch. A second Ctrl-C while quitting exits at once (mpv still terminated).
14b. Linux only: `kill -KILL <coda-app pid>` while playing: mpv stops too (parent-death signal).
14c. Quit while startup is slow (e.g. a `password_command` that waits on pinentry, or an unanswered macOS keychain prompt): close the window during the prompt; within ~3 s the app exits and no mpv is left.

## Look
15. Dark and light (switch the OS appearance): readable, accent amber, no text selectable outside inputs, no page scroll/bounce, focus rings visible when tabbing.
16. Resize to the minimum 900×600: nothing overlaps; long titles ellipsize.

## Now Playing (`f`)
17. `f` (or a click on the bar's cover) opens the Now Playing view and **only** it: no sidebar, browse area, queue column or bottom bar. Large cover over a blurred, darkened copy, title, artist — album, ONE timeline (seekable) with the repeat button beside it, ONE row prev/play/next with volume on the right, visualizer bars and an "Up next" card (cover, title, artist) or "End of queue". `f`, Esc or the × button (top right) returns to the previous view with the normal layout (queue column as before). The window can still be dragged by its title bar and the macOS traffic lights work.
17a. In Now Playing, `q` or the queue button (top right) slides the queue in from the right as a translucent card; clicking the Up next card opens it too (the card hides while it is open). In the panel: click/Shift/Cmd-select, drag to reorder, double-click plays, Delete removes, right-click menu (dark, readable in a light theme too), the header's Shuffle / Save / Clear buttons (Clear's dialog appears over the view). Close the view with the panel open, open it again: the panel is still open; the main layout's queue column kept its own state (toggle each, relaunch: both remembered).
17b. Resize: at the minimum 900×600 the cover shrinks and the content stacks; the panel overlays the content. On a wide window (≥1180px) the content moves left to make room. With Reduce motion on, the panel appears without sliding.
17c. Shortcuts in Now Playing: Space, n/p, ←/→, +/-, `r` (repeat button follows, no toast), `q`, Esc, `f`. Toasts appear bottom-right (no bar below them).
18. Visualizer: bars move with the music, fall smoothly to silence within ~1 s after pause. In the log, `SetVisualizer(true)` is only sent while the view is open; minimise the window (or switch to another macOS Space) → readings stop, restore → they resume. With "Reduce motion" on (macOS Accessibility / GNOME), the bars update a few times a second instead of animating.
19. Nothing playing: "Nothing playing", no visualizer.

## Lyrics mode (`l` in Now Playing)
L1. A local FLAC with timed lyrics: press `f`, then `l` (also once with the focus on the play button, via Tab: after `l` the focus ring is still on it): the player moves into a column on the left (cover, title, artist — album, timeline, transport with volume, visualizer, Up next) and the lyrics appear in the middle. The lyrics button (top right, left of the queue button) shows its on state; its tooltip is "Lyrics (l)". The highlight follows the song; seeking on the timeline (and ←/→) jumps the highlight; clicking a line seeks there. Pause: the highlight stays. The visualizer still reads well at the column's width.
L2. Scroll the lyrics by hand (wheel, dragging the scrollbar, or click the lyrics and use ↑/↓/Page keys): auto-follow pauses, then after ~4 s it eases back to the current line. Drag the scrollbar slowly for more than 4 s, and also hold it still for more than 4 s: it is never pulled back while you hold it; it eases back ~4 s after you let go.
L3. An instrumental track (no `LYRICS` tag): "No lyrics for this track", and the layout stays put. Skip between tracks with and without lyrics: nothing jumps (also skipping from late in one timed track to another timed track played earlier, so cached: the new lyrics start at the top, not scrolled to the old position; likewise a Navidrome track while it buffers), and lyrics of the previous track never show for the next one. A track with plain (untimed) lyrics shows them as text to scroll, none highlighted.
L4. A Navidrome track with lyrics, and one without ("No lyrics for this track"). With the server unreachable: "Couldn't load lyrics" (not "No lyrics").
L5. Open the queue (`q`) in lyrics mode: on a wide window (≥1180px) three columns (player, lyrics, queue), the current line still about a third of the way down; on a narrower one the panel overlays the lyrics.
L6. At the minimum window size (900×600) the column layout still fits: cover, title, timeline, transport and the lyrics all usable (the left column scrolls if the window is short). There is no stacked layout.
L7. Turn on reduced motion (macOS Accessibility / GNOME "Reduce animation"): toggling `l` does not animate, the lyrics jump to the current line instead of scrolling smoothly.
L8. Leave Now Playing (`f`/Esc) and come back: lyrics mode is still on. `l` outside Now Playing does nothing. Restart the app: it is off.

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
30. `CODA_CONFIG=/tmp/coda-empty/config.toml ../../target/debug/coda-app` (folder absent) → the "Welcome to Coda" screen instead of the shell. Choose… a folder → Continue → the shell appears and the folder scans; `/tmp/coda-empty/config.toml` holds `music_dir`.
31. Same with only "Connect a Navidrome server": Test connection, Continue → the shell appears, Navidrome connects; a wrong password shows the error and stays on the screen.

## Window
32. Resize and move the window, quit, relaunch: same size and position (also after a maximise).

## Browse (M2)

Same setup as above. A configured music folder and Navidrome server make every section visible; with only one, the other section is hidden from the sidebar.

### Navigation
33. Local → Artists: an alphabetical list with a letter index on the right; clicking a letter jumps there. While the scan runs, a "Scanning the library… N tracks so far" line shows and the list grows (refreshes about every 2 s) without losing the scroll position.
34. Click an artist → artist page (albums grid, Play / Shuffle). Click an album → album page (large cover, artist link, year for Navidrome albums opened from a grid, track count, length; tracks grouped by disc when there are several). Back with Backspace, Alt+←, Escape, the header's back arrow, and the mouse back button: each returns to the previous page at the same scroll position.
35. A sidebar entry always starts a fresh stack (Back does nothing at a sidebar root).

### Lists and grids
36. Local → Albums: grid with covers (placeholder initials without art), Sort A–Z / By artist. Arrow keys move between tiles, Enter opens, right-click or the "…" on hover opens the album menu (Play, Shuffle, Play next, Add to queue, Open album, Go to artist).
37. Navidrome → Albums: all six sorts; scrolling down loads more ("Loading more…") until the end; Random gives a new order each time it is chosen.
38. Folders: sub-folders as tiles, then the folder's own tracks; breadcrumbs go back up. Shuffle (or `s`) plays everything below the folder.
39. Playlists (local `.m3u` and Navidrome): open one; duplicates are kept; a local playlist with missing entries shows an amber note ("N entries not found in the library"); a Navidrome playlist whose server count differs shows the hidden-entries note.

### Tracks and playing
40. In a track table: ↑/↓ select, Enter or double-click plays the list from that track (the queue becomes that album/playlist/folder list); the playing row shows the amber equaliser in every view.
41. Right-click a track or its "…" button: Play, Play next, Add to queue, Go to album, Go to artist (disabled when unknown). Play next / Add to queue show a toast.
42. `s` shuffles the shown context: an album/playlist's tracks, an artist's tracks (all albums), a folder's tracks below it, or "Shuffle all" on the Artists/Albums lists (local: whole library, toast "Shuffling all (N tracks)"; Navidrome: a fresh set of random songs).
43. Go to album / Go to artist from a queue-built context (e.g. after Play next) lands on the right page for both sources.

### Search
44. `/` or Ctrl+F opens Search with the cursor in the field (from anywhere, also again while on Search). Typing searches after a short pause; results are grouped per source into Artists, Albums and Tracks. Enter on a track plays that source's matching tracks from it; Escape leaves the field, a second Escape goes back.
45. A query with no matches shows "No matches"; Navidrome down (stop the server) shows an error with Retry, while local results still show.

### Errors and states
46. Stop the Navidrome server and open Navidrome → Artists: loading skeleton, then "Could not load" with Retry; start the server and Retry loads.
47. Light and dark themes: skeletons, tiles, rows, menus and notes are readable in both; focus rings visible when tabbing through the album grid and lists.
48. Settings → change the music folder, or save/disconnect the server, while a browse view of that source is open: that source's views return to its Home and reload (no stale ids); a cached Navidrome list is not shown again after a server change. With only one source configured, the sidebar shows only that one, and the other source (if it was shown) falls back to this one (H3).
49. Now Playing open: Backspace/Escape/Alt+← close it (the browse stack stays); `/` closes it and opens Search; Go to album/artist or a sidebar entry also closes it.

## Review fixes
50. Onboarding race: with the onboarding screen showing, run `coda --setup` in a terminal and connect a server; then in the GUI choose only a folder and Continue → "The settings were changed meanwhile…"; `config.toml` and the keychain entry are unchanged.
51. Save a server with an unreachable URL and quit during the ping: the app quits at once (the ping is not under the write lock), and nothing was written.
52. Symlink `config.toml` to a dotfiles copy → Settings shows the symbolic-link warning.

## Browse review fixes
53. Large library, while the scan runs: open Local → Folders and press `s` at the root, open Search and type, scroll Albums — playback never stutters or skips a gapless transition (the engine never waits for browse reads; `RUST_LOG=trace` shows "changes deferred" now and then).
54. Folders root → Shuffle on a library larger than 10,000 tracks: plays at once, toast "Shuffling 10,000 of N tracks"; the bar's cover, the queue covers and the album grid you came from still show their covers afterwards.
55. Navidrome: open a compilation (or an album whose first track has a guest artist) from the grid; the header shows the album artist and year, and clicking the artist opens the album artist, not the first track's.
56. Stop the server, then use an album's menu → Play (or Navidrome Albums → Shuffle): a toast "Could not play: …" appears. Change the music folder in Settings while an artist page is open and press Play at once: no silent failure (the view reloads with a toast).
57. Navidrome Albums sorted "Random": keep scrolling; more pages keep loading even when a page repeats albums, until the end.
58. During a scan, open the context menu on a track of an album being scanned and wait: when the list refreshes, the menu still acts on the same track (or closes if it disappeared); the selection stays on its track.

## One source at a time, Home (feedback round 1, slice B)
Config: `~/.config/coda-app/settings.toml` (`source = …`); history: `~/.local/state/coda/coda-app/history.json`.
H1. Both sources configured: the sidebar starts with a "Local | Navidrome" switch (icons), then Home, Search, Albums, Artists, Playlists (and Folders only on Local), Settings at the bottom; no source appears twice. First run (no `source` in settings.toml): Navidrome is shown.
H2. Click Local (or Ctrl/Cmd+1), open an artist → album; switch to Navidrome (Ctrl/Cmd+2), open something else; switch back: the local album is shown again, and Back walks the local stack. Quit and relaunch: the last chosen source is shown (settings.toml has `source = "local"`/`"navidrome"`; theme settings unchanged).
H3. Only one source configured: no switch, just its name with the scan line or connection dot; Ctrl/Cmd+1/2 for the missing source does nothing. Disconnect the server in Settings while Navidrome is shown: Local is shown and Settings stays open; reconnect: Navidrome comes back (it was the saved choice).
H4. `/` on Local searches the local library only; "Search Navidrome instead" switches and searches Navidrome for the same text (and back).
H5. The queue still mixes both: enqueue a local album and a Navidrome album, both play. On a local track in the queue, while Navidrome is shown, right-click → Go to album: the sidebar switches to Local and opens the album.
H6. Home is the landing view of each source. New user (move history.json away): a one-line hint, Recently added (Navidrome) and Daily Picks; no empty rows.
H7. Play an album, a playlist, an artist (Play) and, on Local, a folder (Play, or a track in it): each appears at the front of Home → Recently played (newest first, no duplicates). A search result track or Shuffle all adds nothing.
H8. Listen to a track for more than half its length (or 4 minutes): after returning to Home, its album (Local: and the playlist it was played from) shows in Most played with the count. Navidrome's Most played starts with the server's most played albums (TUI listening counts too) followed by playlists.
H9. Daily Picks: 12 albums; reopen Home and relaunch the app: the same picks all day; after local midnight (or with the clock moved a day on) they change at the next Home visit. "Shuffle Daily Picks" (or `s` on Home) queues all picks' tracks shuffled.
H10. Rows: ‹ › scroll a row; Tab moves between rows, ←/→ between cards (no seeking), Home/End to the ends, Enter opens, Shift+Enter or the hover ▶ plays. Artist cards are round. Light and dark themes look right.
H11. Put garbage in history.json (or `mkfifo` it while the app is closed) and launch: the app starts at once with an empty history, the old file is now `history.json.bad`; play something and quit: a new history.json is written. history.json contains no `http`, `coda://` or password.
H12. Per server: note Home on Navidrome, then in Settings connect another server (or the same server as another user): its Home shows none of the first server's Recently/Most played or Daily Picks. Connect the first one again the same day: its history and the same Daily Picks come back. history.json holds no server URL or user name (only 16-hex `server` ids).
