# Genres: brief

- **Status:** approved by the owner 2026-11-15 ("keep the comma split, GUI first, M11, go ahead"). Branch `genres`.
- **Owner's words:**
  > I noticed a genre tag is included. Could it be possible to add a Genre tab to Coda? And then maybe shuffle on genre.

  and, on how the library is tagged:
  > Every album should have: A broad genre (maybe two, but I'd say prefer one); one or two 'subgenres,' ideally.
- **Why now:** genres are the first item under "Direction" in `docs/VISION.md`, and Mixes (the next item) are built on them.
- **VISION principle 3 applies:** Coda reads genres as tagged. It never writes, fetches or corrects them, and it knows no hierarchy: "Rock" and "Progressive Rock" are two genres.

## Decisions (approved)
1. **A track has a list of genres.** `genres: Vec<String>` on `TrackMetadata` and `Track`, in tag order, without duplicates (compared case-insensitively).
2. **Where the list comes from:**
   - **Local:** every `ItemKey::Genre` item of the tag lofty reads (`Tag::get_strings`), so repeated Vorbis `GENRE` fields, ID3 `TCON` lists and numeric ID3 genres all count. Each value is then split on separators, so a single `Rock;Progressive Rock` (as in the owner's M4A album) gives two genres.
   - **Navidrome:** OpenSubsonic `genres: [{name}]` when present, otherwise the classic `genre` string, split the same way.
   - **Separators:** the same ones Navidrome splits on by default, so a song has the same genres from both sources. Believed to be `;`, `/` and `,`; slice 1 checks Navidrome's source first and records the commit, as the Navidrome notes in ARCHITECTURE.md do.
   - **Caps:** each genre goes through `clean_text` (trim, 512 chars); at most 16 genres per track, the rest dropped.
3. **Grouping:** genres group case-insensitively and show their most common spelling, exactly like artists and albums (`TagIndex`). There is no "No genre" entry: finding untagged music is library maintenance.
4. **What the Genres view shows:**
   - A list of genres A–Z, with album and track counts.
   - Opening a genre shows its **albums** (an album is in a genre when any of its tracks is). Local: sorted by artist, then album; Play and Shuffle play the whole genre. Navidrome (accepted deviation, slice 2): the albums come in the server's `byGenre` order, and the page has **Shuffle only** (random songs in the genre, below); playing a whole remote genre in order would need `getSongsByGenre`, which is not in this milestone.
   - **Shuffle a genre:** local shuffles every track in the genre (capped at the queue maximum, like Shuffle all). Navidrome uses `getRandomSongs` with `genre=`, 500 songs, like Shuffle all.
5. **Sources stay separate,** like the rest of browsing: Local genres and Navidrome genres. Combining sources is the Mixes milestone's job.
6. **Order:** core, then the GUI, then the TUI, one slice each, with both frontends building after every slice.

## What exists (verified 2026-11-15)
- **The owner's library** (`/storage/music`, 1,332 tracks): every track has one broad genre and 0–2 subgenres. FLAC files use one `GENRE` field per genre; the one M4A album has `Rock;Progressive Rock` in a single field. 19 distinct genres.
- **No genre code anywhere.**
  - `model.rs`: `TrackMetadata` and `Track` have no genre. Tracks are only built through `Track::local` and `Track::navidrome`, which clean every string.
  - `library/scan.rs` `read_file` reads the tag with lofty 0.25 and uses the first-value `Accessor` methods.
  - `navidrome/api.rs` `RawSong` skips `genre` and `genres` unparsed; only the 10,000-entry playlist test's fixture contains them.
  - `AlbumSort` is a closed set without `byGenre`, and `Client::random_songs` sends no `genre`.
- **Exhaustive `TrackMetadata` literals** that break when a field is added: `session.rs:351` and `:632`, `scan.rs:496`, `api.rs:590`, `lyrics/cache.rs:267` and `:595`, `lyrics/cache/tests.rs:10`, GUI `engine.rs:477`.
- **Session:** saved queue entries go through `EntryFile`, which has no `deny_unknown_fields`; `album_artist` and `cover_art` were added later with `#[serde(default, skip_serializing_if = …)]`.

## Slice 1: core (`coda-core`)
- **Model:** `genres: Vec<String>` on `TrackMetadata` and `Track`. `Track::local`/`Track::navidrome` clean, dedupe and cap them. Update the exhaustive literals listed above.
- **Splitting:** one function, `model::split_genres(values) -> Vec<String>`, used by both sources.
- **Local tags:** `read_file` collects `tag.get_strings(&ItemKey::Genre)`.
- **Session:** `EntryFile.genres`, `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. A file without it still loads; a saved queue keeps its genres.
- **Lyrics cache:** not extended. Tracks rebuilt from it (lyrics search results) have no genres, which nothing shows yet. Noted in ARCHITECTURE.md.
- **Local index:** a genre index in `TagIndex`, maintained in the same `insert` call as artists and albums:
  - `GenreId(pub u32)`, dense, in insertion order (the GUI's local ids rely on that);
  - per genre: spellings, sorted track ids, and its albums;
  - a list sorted A–Z;
  - accessors for the sorted list, a genre's albums and a genre's tracks.
- **`LocalBrowser`:** `HomeEntry::Genres`, `View::Genres` and `View::Genre(GenreId)`, with arms in every exhaustive match (`open_selected`, `play_context`, `selected_tracks`, `selected_label`, `view_title`, `opener_key`, `row_key`, `view_len`, `view_rows`, `position`). A genre's rows are its albums; playing the genre plays all its tracks.
- **Navidrome client:**
  - `RawSong` reads `genre` and `genres`. `genres` items go through a new capped list (`MAX_GENRES_PER_SONG`), counting towards the response budget.
  - `Client::genres()`: `getGenres`, returning name, song count and album count (the name is the item's `value`). Capped list, like the others.
  - `Client::albums_by_genre(genre, offset, size)`: `getAlbumList2` with `type=byGenre&genre=`, size clamped to 1..=500 like `album_list`. A separate method, so `AlbumSort` stays a closed set of plain sorts.
  - `Client::random_songs_in_genre(genre)`: `getRandomSongs` with `genre=` and size 500.
  - The genre name is user data in a query parameter: encoded by the URL builder, never logged.
- **`navidrome::Browser`:** `HomeEntry::Genres`, `Resource::Genres`, `Resource::GenreAlbums { genre, offset }` and a `RandomSongs` variant for a genre, following the existing cache, paging and job rules.
- **Tests:**
  - splitting: separators, duplicates in other cases, blanks, the cap, hostile input;
  - local scan: repeated genre items, an ID3 `TCON` list, a single separated value, no genre (WAV fixtures tagged with lofty, as `library_scan.rs` does);
  - `TagIndex`: grouping and spelling, album membership, ordering, incremental batches;
  - session: round trip with genres, and an old file without them;
  - Navidrome (wiremock): `genres` preferred over `genre`, `genre` alone, the new endpoints and their parameters, empty, failed and oversized replies; the 10,000-entry playlist test still parses within its cap with genres now read.
- **Acceptance:** all AGENTS.md checks pass; `cargo tree -p coda-core` has no UI crates; both frontends build and behave as before.

## Slice 2: GUI
- **Backend:** commands `local_genres`, `local_genre{id}`, `navidrome_genres`, `navidrome_genre_albums{genre, offset}`, `navidrome_genre_random_songs{genre}`. `GenreDto { id, name, albumCount, trackCount }`. Local genre ids use the `"<generation>:<n>"` form. `LocalContextDto::Genre{id}` so the engine resolves and shuffles a local genre itself, as it does for artists and folders.
- **Frontend:**
  - a "Genres" sidebar entry for both sources, after Albums;
  - a genre list view, then a genre page with its albums and Play/Shuffle buttons; `s` shuffles the genre, as on other pages. Accepted deviation: a Navidrome genre page has Shuffle only (random songs in the genre) and lists its albums in the server's order (decision 4);
  - the album page shows the album's genres as small links to their genre pages. On Navidrome a link is resolved against the server's genre listing (case-insensitive cleaned name → that genre's exact value), so it opens what the Genres list opens.
- **Tests:** backend command and engine tests (local genre play and shuffle); `pnpm build` and `pnpm check:colors`. Runtime checks are handed to the owner in `scripts/smoke.md`.

## Slice 3: TUI
- A "Genres" row on both Home menus, after Albums: the genre list, then a genre's albums. Enter plays and `s` shuffles, as everywhere else. No new keys.
- Empty-view messages, the help overlay, `docs/USAGE.md` and README key sections updated.
- **Tests:** `LocalBrowser` and `Browser` unit tests for the new views; a pty smoke run with a temporary library.

## Docs
- `docs/ARCHITECTURE.md` (model, scan, tags, browse, navidrome, session) and `apps/coda-gui/docs/ARCHITECTURE.md` and `DESIGN.md` (sidebar).
- `docs/ROADMAP.md`: a new "M11 — Genres" entry. `apps/coda-gui/docs/PLAN.md`: a dated "Genres" section, like Lyrics.

## Not in this milestone
- Mixes, combining sources and endless play (the next milestones).
- Genres in search, in play history ("Jump back in") and on the now-playing screen.
- Sorting genres by size, genre artwork, and any genre hierarchy (VISION principle 3).
- Anything that edits or fetches genres.
