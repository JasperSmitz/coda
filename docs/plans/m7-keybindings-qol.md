# M7 — Keybindings and quality of life: brief

- **Status:** approved by owner 2026-11-02 ("Go."), from the owner's notes in `docs/M7_QOL_Findings.md` and the discussion that followed.
- **Goal:** Coda plays music *in context*. Enter plays what you point at, in its album or playlist, and `s` plays it shuffled. The queue is temporary: you can add to it, but a stored queue is a playlist. Keys follow two rules:
  - **One key, one meaning.** A key does the same kind of thing everywhere, adapted to where it is pressed.
  - **Shift means a stronger version of the same action.**
- **Delivery:** three slices, each reviewed and committed separately.
  1. Keymap and play in context.
  2. Shuffle all.
  3. Wide visualizer.
- **Out of scope:** endless play and context-aware autoplay (a future milestone), configurable keybindings, and an undo for queue changes.

## Final keymap

| Key | Library | Queue | Notes |
|---|---|---|---|
| `j` `k` `↓` `↑` | move | move | unchanged |
| `l` `→` | open the selected entry | — | new. On a track it does nothing. |
| `h` `←` `Backspace` | go up | — | `←` is new. `Esc` still leaves search results first. |
| `Enter` | play in context (see below) | play this entry | changed |
| `s` | play the selection shuffled (see below) | shuffle upcoming | `s` in the library is new |
| `a` / `A` | add / play next | — | unchanged |
| `,` / `.` | seek −5 s / +5 s | same | moved from `←` `→` |
| `<` / `>` | previous / next track | same | moved from `p` / `n` |
| `r` / `R` | repeat (off/all/one) / refresh Navidrome | repeat | swapped. Refresh is library-only, as now. |
| `D` | remove from a playlist; on a playlist, delete it (with a confirmation) | remove the entry | see below |
| `O` | settings | settings | moved from `,` |
| unchanged | `Space` pause, `+` `-` volume, `v` visualizer, `/` search, `Tab` panel, `J` `K` move, `C C` clear, `g` jump to current, `S` save queue, `P` add to a playlist, `N` new playlist, `e` rename, `w` save order, `o` sort, `1` `2` source, `?` help, `q` quit | | |

- **Unbound:** `n`, `p`, and lowercase `d`.
- **Arrow keys:** `←` `→` no longer seek anywhere.
- **Help screen:** the help overlay, the README if it lists keys, and every status hint that names a key ("Press D again…", "w to save", and so on) must match the table. Grep for old keys in user-facing strings.

## Slice 1 — keymap and play in context

### Enter: play in context
Enter **replaces the queue** and starts playing. There is no confirmation, by owner decision.

| Selection | Queue becomes | Starts at |
|---|---|---|
| Album | the album's tracks, in album order | the first track |
| Playlist (local m3u or Navidrome) | the playlist's playable entries, in order | the first entry |
| Track inside an album, playlist, folder, artist or search-results view | the whole list the view shows, in view order | **this track**: earlier tracks sit before it in the queue, so `<` walks back through the context |
| Artist, folder, home menu entry | — (Enter opens it, like `l`) | — |

- **Navidrome lists that aren't cached yet:** they reuse the existing enqueue fetch jobs (`JobTarget`) with a new "replace and play" outcome. It is applied when the fetch completes, and dropped if the client generation changed or a newer play request superseded it. Only the latest play request wins. An `a`/`A` job in flight is unaffected and still appends when done.
- **Queue cap (`MAX_QUEUE_LEN`):** if the context is longer than the cap, keep a window that contains the start track. Drop entries from the front before the back, and report it the way enqueue does.
- **Status line:** "Playing <album/playlist name>", or "Playing <track>" for a single track.
- **Behaviour that carries over:** the existing play-now semantics (LoadId, scrobble bookkeeping, `unscrobbled_load`, and repeat mode staying as set).
- **Queue focus:** Enter in the queue panel is unchanged.

### `s`: play shuffled
- **In the library:** like Enter, `s` replaces the queue and plays, but in shuffled order. It uses the existing `Rng`, so tests can seed it.
  - **On a track:** shuffle its context, with the selected track first.
  - **On an album or playlist:** shuffle its tracks.
  - **On an artist:** shuffle all of that artist's tracks, local or Navidrome. The Navidrome `JobTarget::Artist` fetch already exists.
  - **On a folder:** shuffle every track below it, as `a` collects them.
  - **On a home entry:** nothing, until slice 2.
- **In the queue:** unchanged (shuffle upcoming).

### `D`: remove and delete
- **Remove an entry:** one press of `D` removes the entry under the cursor, both in the queue and in a Navidrome playlist's contents. Repeated presses work. Playlist removals are already serialized by the `EditRunner`, and the hidden-entry warning flow stays.
- **Delete a whole playlist:** `D` on a playlist in the playlist list opens a **y/n confirmation prompt** naming it ("Delete playlist 'Road trip'? y/n"), using the existing prompt mechanism. Only `y` deletes. Every other key, including `D`, cancels. The `EditArm::Delete` / `D D` path goes away.
- **Unchanged:** the `EditArm` double-press and its timeout remain for `C C`, if they are shared, and for anything else that still uses them.

### Tests
- **Keymap tests** cover every key in the table. They include the Shift variants as crossterm reports them: `<` and `>` arrive as `Char('<')`/`Char('>')`, sometimes with `SHIFT` set. They also check that old bindings are gone: `n`, `p` and `d` do nothing, `←`/`→` don't seek, and `,` doesn't open settings.
- **App tests for Enter:**
  - album, playlist, and a track mid-list (the queue shows the full context and the current entry is the selected track);
  - a search-result track;
  - Navidrome uncached: a fetch completes, then the replace happens;
  - a stale generation, and a superseding second Enter;
  - the cap window.
- **App tests for `s`:** the selected track comes first, the multiset is preserved, and a seeded order is deterministic. Cover artist, local and Navidrome (mocked).
- **App tests for `D`:**
  - queue removal;
  - repeated playlist removal;
  - the playlist-delete prompt: `y` deletes, anything else cancels and nothing is sent.
- **Real-mpv and smoke tests:** the PTY smoke test is updated for the new keys.

## Slice 2 — shuffle all
- **Where:** a "Shuffle all" entry at the top of each source's home menu. Enter or `s` on it replaces the queue and plays.
- **Local:** every track in the library index, shuffled (seeded `Rng`), within the queue cap. While a scan is still running, use what has been scanned so far, and say so in the status line, as search does.
- **Navidrome:** one `getRandomSongs` request with `size=500`, the server maximum. Use the per-endpoint reply type and the object budget like the other endpoints, under the same generation and supersede rules as Enter. It plays random picks, not a strict shuffle; that is accepted. There is no refill: endless play is a later milestone.
- **Tests:** a mock response for `getRandomSongs`, malformed and oversized replies, and the local shuffle.

## Slice 3 — wide visualizer
- **Bars:** the bands area spreads across the available width of the visualizer area, with the L/R meter staying on the right. The bar count follows the width, with roughly 2–3 columns per bar plus a gap, up to a sensible cap (about 48 bars).
- **Values:** between the 7 measured bands, values are interpolated **on a log-frequency axis** with a smooth curve (monotone cubic or cosine). It should not overshoot, so there are no bars above both neighbours. It's about the look, not accuracy (owner).
- **Bands Coda doesn't have:** bands above Nyquist (`None`) stay empty, and interpolation does not invent values beyond the last available band.
- **Narrow terminals:** they keep today's layout (7 bars at 1 column, or hidden).
- **Drawing only:** no change to the mpv graph or the polling. The work is in `draw_visualizer`/`Meter` presentation code.
- **Tests:**
  - interpolation hits the 7 real values at their positions, stays within neighbouring bounds, and handles `None` bands;
  - bar layout at several widths;
  - render snapshots at narrow, medium and wide widths.
