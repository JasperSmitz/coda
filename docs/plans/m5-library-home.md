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

## Slice 2 — local home, tag-based artists and albums, `.m3u` playlists

### Home
- The local source opens on **Home**: `Playlists`, `Folders`, `Artists`, `Albums`.
  - `Folders` is today's directory tree, unchanged.
  - Keys and titles work as on Navidrome: `Enter`/`l` opens, `h`/`Backspace` goes back up and ends at Home, `a`/`A` enqueue, and `/` searches (local search is unchanged; its results return to where search was opened).
  - Titles show the path, e.g. `Local › Albums (by artist)`.
- During a scan, Home and the lists are usable and fill in as tracks arrive. A small "Scanning…" note stays in the title until the scan finishes.

### Artists and albums from tags
- `TrackMetadata` and `Track` gain `album_artist` (lofty's album-artist item).
  - The saved session must stay compatible: a version 1 `state.json` without the field still loads, and the field is optional when saving.
  - Remote tracks fill it when the API provides it, and otherwise leave it `None`.
- **Grouping:**
  - Artist key: album artist, else artist, else "Unknown artist".
  - An album is (artist key, album tag), so the same album name under two album artists gives two albums.
  - Tracks without an album tag go into "Unknown album" under their artist.
  - Keys compare case-insensitively with surrounding whitespace trimmed; the most common spelling is displayed.
- **Sorting:**
  - Artists A–Z, case-insensitive.
  - An artist's albums A–Z.
  - Album tracks by disc, track, then title, then path (the same rule as the tree).
- **Albums list:** every album, with `o` cycling A–Z by name or by artist. Each row shows the artist.
- **Performance:**
  - The index updates with scan batches or is rebuilt off the input loop. It must never block a frame: measure with 50,000 synthetic tracks, as local search did.
  - Memory: the index refers to tracks (indices or shared handles) rather than cloning them where practical.

### `.m3u` / `.m3u8` playlists
- The scan also collects `*.m3u` and `*.m3u8` files under the music folder, with the same symlink rules as audio files.
- **Parsing** (a pure function that the tests drive directly):
  - Read at most 1 MiB per file and at most 10,000 entries; larger files are cut, with a note.
  - `.m3u8` is UTF-8. A plain `.m3u` is tried as UTF-8 and otherwise decoded lossily.
  - Blank lines and `#` lines are skipped; `#EXTINF` titles are ignored, because tags are the source of truth. A leading BOM is ignored.
  - Relative entries resolve against the playlist's folder. An absolute entry is accepted only if it falls inside the music folder after normalisation.
  - URLs (anything with a scheme) are skipped.
  - An entry must match a scanned track by normalised path. Anything else counts as missing.
- **Display:**
  - The name is the file stem. Playlists sort A–Z.
  - Each row shows "N tracks" and, when some entries are missing, ", M missing".
  - The playlist view lists the matched tracks in file order, keeping duplicates.
- **Resolution:** playlists are resolved against the library after the scan finishes, or on demand when opened; never on the input loop if that could take longer than a frame.
- **Never:** follow a playlist entry outside the music folder, execute anything, or log entry paths.

### Tests
- **Grouping:**
  - album artist vs artist fallback
  - case and whitespace merging
  - same album name under two artists
  - unknown artist and album
  - track order
  - the 50k-track timing
- **Session:** a version 1 file without `album_artist` loads, and the round trip keeps the field.
- **m3u parser:**
  - relative and absolute paths, entries outside the root (`..`, absolute elsewhere, symlinked folders), URLs
  - comments, `#EXTINF` and BOM
  - CRLF line endings
  - lossy `.m3u`
  - caps and duplicates
  - missing entries counted
- **App/browser:**
  - Home navigation
  - `h` back up to Home
  - lists filling in during a scan
  - `a`/`A` on an artist, album or playlist, in order
  - `o` on Albums
- **Render:**
  - Home
  - artists, albums (both sorts), playlist rows with missing counts
  - the scanning note
  - help still fits 80x24
- **PTY smoke:** a temporary music folder with tagged files, including compilations and a missing album tag, plus playlists (relative, absolute inside and outside, missing, URL).

### Decisions made during slices 1–2
- `l` was not bound in either source, contrary to "Enter/`l`" above. This is deferred to M6 together with the owner's keybinding notes.
- "Missing" in a playlist row covers unmatched files, entries outside the music folder, refused `..` paths and URLs.
- Unknown artist and Unknown album sort last.

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
