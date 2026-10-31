# M5 — Library home and playlists: implementation brief

- **Status:** approved by owner 2026-11-01 ("let's go to the next milestone"). The roadmap scope is refined below.
- **Goal:** each library source opens on a small home menu instead of dropping straight into one hierarchy. Navidrome playlists become playable. Albums can be browsed directly.
- **Delivery:** slices, each implemented, independently reviewed, fixed and committed. No push.

## Slice 1 — Navidrome home, playlists, albums, "play next"

### Home menu
- The Navidrome source opens on **Home**: `Playlists`, `Artists`, `Albums`.
  - `Enter` or `l` opens an entry; `h`/`Backspace` from a top-level list returns to Home.
  - Search (`/`) still works from anywhere in the source, and `h` from results returns to where it was opened.
- The panel title shows the path, e.g. `Navidrome › Albums (newest)`.
- Home is a static view; it fetches nothing. The connection state stays visible in the title as today.
- Session persistence of the browse position is **not** in scope; Navidrome opens on Home.

### Playlists (read-only)
- `Playlists` lists `getPlaylists`, sorted as served: name, song count, total duration.
- `Enter` opens a playlist (`getPlaylist`) as a song list in playlist order, and duplicates are kept.
  - On a song, `Enter` plays it now (existing insert-after-current rule).
  - On the playlist row in the Playlists list, `a` appends the whole playlist in order.
- Entries the server returns without a usable id are skipped, as albums do today.
- Untrusted data:
  - Names and comments are capped like other metadata.
  - Song counts and durations saturate.
  - A playlist with thousands of entries stays within the existing body cap. Measure: 10,000 entries must parse and render.
  - Rendering only visible rows already applies.

### Albums
- `Albums` lists `getAlbumList2`. The sort cycles with `o`, shown in the title:
  1. A–Z by name (`alphabeticalByName`, default)
  2. by artist (`alphabeticalByArtist`)
  3. recently added (`newest`)
  4. most played (`frequent`)
  5. recently played (`recent`)
  6. random (`random`)
- **Paging:** `size=500`, starting at `offset=0`. The next page is requested automatically when the selection comes within one screen of the end of what is loaded, until a page returns fewer than `size`. A footer row shows "Loading more…" or the error ("Could not load more — r to retry").
  - Each sort order is cached separately.
  - Random is never cached; reopening or `r` reshuffles.
  - Cap the total at the queue cap order of magnitude (e.g. 50,000 albums) to bound memory.
- `Enter` opens an album (existing album view). `a` appends it, using the existing ordered enqueue jobs.
- The request-id, generation, at-most-one-request-per-resource and exactly-once rules extend to paged resources. A page response is applied only if it is the next expected page of the current sort.

### Play next
- `A` inserts the selection directly after the current entry (as next) instead of appending. This works everywhere `a` works, local and remote: a song, album, artist, playlist, folder or search result.
  - Several `A` presses in a row insert in press order. The second goes after the first, not before it, so a "next up" run keeps its order. Remember the insertion point until playback moves on or the queue changes otherwise.
  - With nothing current, `A` behaves like `a`.
- Async enqueue jobs (an album or playlist not yet fetched) keep their position in press order, as `a` jobs do today.
- The help overlay and README list `A`. The final keymap is still M6's business, and this is the one binding the owner asked for already.

### API (`navidrome::client`)
- New calls:
  - `playlists()` (`getPlaylists`) and `playlist(id)` (`getPlaylist`).
  - `album_list(kind, offset, size)` (`getAlbumList2`; `type` from a closed enum, `size` ≤ 500).
- New domain type `Playlist { id, name, song_count, duration }`. The playlist's songs are `Track`s, as for albums.
- Wiremock tests:
  - auth parameters;
  - parsing with missing fields and hostile values;
  - paging parameters;
  - an empty list (Subsonic omits the array);
  - a failed status.

### Tests
- **Browser state:**
  - Home navigation and `h` back to Home.
  - Playlists: open and enqueue in order, including duplicates.
  - Albums: sort cycling.
  - Paging: the next-page trigger, stale page responses ignored, end of list detected, a failed page with retry, and random not cached.
  - Generation checks for the new resources.
- **Play next:**
  - consecutive `A`s keep order;
  - `A` with nothing current;
  - `A` on an async job keeps its turn;
  - interaction with shuffle and repeat;
  - local folder `A`.
- **Render:** Home, playlist list, playlist songs, album list with the sort in the title, the paging footer, and help still fitting 80x24.
- **PTY smoke** against a fake Subsonic server with playlists and more than 500 albums: page through, change the sort, play a playlist, then `A` a song.

## Slice 2 — local home (proposed, briefed in detail after slice 1)
- The local source gets the same Home: `Playlists`, `Folders` (today's tree), `Artists`, `Albums`.
- Artists and albums are derived from tags. This needs an album-artist tag in `TrackMetadata` (fallback: artist) and grouping by (album artist, album), with track and disc order.
- `.m3u`/`.m3u8` files found during the scan are local playlists. Relative paths resolve against the playlist's folder; entries outside the music folder, URLs and missing files are skipped and counted.

## Slice 3 — Navidrome playlist editing (approved by owner 2026-11-01; briefed in detail after slice 2)
- **Save the queue as a new playlist** (`createPlaylist`, name typed in a single-line input).
- **Add to playlist:** add the selected song, album or playlist to a playlist picked from a short list, with "New playlist…" at the top. Uses `updatePlaylist` `songIdToAdd`, in order.
- **Remove** a song from a playlist, with confirmation (`songIndexToRemove`).
- **Reorder** songs with `J`/`K`, as in the queue. Subsonic has no move call, so this rewrites the playlist; define how to do that safely.
- **Rename and delete** a playlist; delete asks for confirmation.
- After every change, refresh the affected views. Show errors inline. Never let a partly applied change look applied.
- Navidrome playlists only; local `.m3u` editing is not included.

## Later (not in M5 unless approved)
Editing local `.m3u` playlists, starring/favourites, genre and year browsing.

## Risks
- Paging plus stale responses plus generations make the browser state machine bigger. Keep paging as one well-tested piece rather than special cases in views.
- `A`'s insertion-point memory is easy to get subtly wrong with shuffle, removal and moves. Define it as data on the queue, with tests.
