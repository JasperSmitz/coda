# Lyrics search (local library and Navidrome): brief

- **Status:** approved by the owner 2026-11-12 ("go ahead and build it"). Branch `lyrics-search`.
- **Owner's words:**
  > is it possible to make lyrics searchable? Say I enter a phrase in the search bar, it shows artist/album/song matches, but also lyrical matches (maybe a separate category?)

  > option 1, yes to all three … I think a couple dozen MB for a large library is totally fine to enable the functionality.
  > 2. I think just playing the song is fine. No need for needless functionality.
  > 3. Hide them … I wouldn't want them to appear in the same list twice

- **Decisions (owner):**
  1. **Navidrome lyrics are searchable.** Navidrome's `search3` does not search lyrics, so Coda keeps its own index: a lyrics cache on this computer, built in the background from the server's lyrics. Option 2 (changing Navidrome or reading its database) and option 3 (mapping local files to Navidrome songs) are rejected.
  2. **A lyric match plays like any song row.** There is no "play from this line" and no seeking to the matched line.
  3. **A separate "Lyrics" section on the search page, below the existing sections.** A track already listed in that search's songs results is left out of the Lyrics section, so no track appears twice.
  4. **Size:** a cache and memory use of a couple of dozen MB for a large library is acceptable.
  5. **Memory (owner, 2026-11-12, after slice 2 measured 77 MB for 20,000 songs that all have lyrics):** slim it down. Keep one copy of the lyrics text in memory (the original), and search it with a fast fold-on-the-fly matcher that gives exactly the results of fold-then-find. Store song details compactly. Target: about 40 MB worst case, about 50 ms or less per search at 20,000 songs (release).
- **Scope change to record:** AGENTS.md lists "database-backed library indexing" as future work unless approved. This is a cache file, not a database, but it is persistent indexing, so record the approval:
  - update the AGENTS.md line;
  - update VISION.md;
  - add ADR `docs/decisions/0007-lyrics-search-cache.md`, a short one: why a client-side cache, why a file and not a database, and the refresh policy.

## What exists (verified 2026-11-12)
- **Lyrics reading:**
  - `coda_core::lyrics`: `parse`, `read_local` (tags, then a `<stem>.lrc` sidecar) and caps;
  - `navidrome::Client::lyrics(song_id, artist, title)`: `getLyricsBySongId` when `songLyrics` is listed, else the classic `getLyrics`.
- **Local search** (`coda_core::library::search`): a linear, case-insensitive substring match over title, artist, album and relative path, capped at 1,000 results. Measured at about 14 ms for 50,000 tracks with no match.
  - **The scan** (`library/scan.rs`) reads every file's tags with lofty at each launch (cover art off).
  - **Persistence:** the scan keeps no cache between launches.
- **Navidrome search:** `Client::search` (`search3`). Albums can be paged with `album_list` (`getAlbumList2`) and their songs listed with `album` / `album_detail` (`getAlbum`).
- **Cover cache precedent:** the cover cache lives at `$XDG_CACHE_HOME/coda/covers` (`cover/cache.rs`, `cache_dir`).
- **GUI search** (`apps/coda-gui/src/views/SearchView.tsx`):
  - debounced search-as-you-type;
  - one command per source (`local_search`, `navidrome_search` in `src-tauri/src/browse/commands.rs`), rendered as Artists, Albums and songs sections.

## Matching rules (shared, in core)
- **Folding** is applied to the lyrics and the query alike:
  - lowercase;
  - `’ ‘ ʼ` become `'`, `“ ”` become `"`, and `– —` become `-`;
  - all whitespace, line breaks included, collapses to one space.

  So a phrase that wraps across two lyric lines still matches. There is no diacritic stripping (that would need a new dependency).
- **The query** is folded the same way. It must be at least 3 characters after folding, otherwise there are no lyric results and no work is done.
- **A match is a substring match on the folded text.** One result per track, at its first match.
- **Snippet:**
  - the original lyric line (or the two lines a match spans) that contains the match;
  - the matched part is given as char ranges for highlighting;
  - if the folded match can't be mapped back exactly (an unusual case-mapping that changes length), return the snippet with no highlight rather than a wrong one;
  - timestamps and word stamps never show (the text comes from the parsed `Lyrics`).
- **Results:**
  - at most 50 are returned, and the total is counted;
  - order: artist, album, disc, track, as the local results are ordered;
  - **exclusion:** the command takes the ids of the songs the main search returned and skips those tracks (decision 3).

## Slice 1: local lyrics search
**Core:**
- **New `coda_core::lyrics::index`:**
  - `LyricsIndex`: per track, the folded text plus what's needed to give the snippet (the original lines and where each line starts in the folded text);
  - `fn search(&self, query, exclude: &HashSet<…>, limit) -> LyricsResults`.

  Tracks without lyrics take no space. Keep memory at about twice the lyrics text or less, and state the measured figure in the report.
- **The scan fills the index:**
  - the lyrics come from the tag lofty already parses for that file, with no second open;
  - a sidecar `.lrc` is read only when the directory listing the scan already has contains it, and only when the tags have none, with the same rules and FIFO safety as `read_local`. Share code with `lyrics::local` rather than duplicating it;
  - the index is rebuilt with each scan. The local index is not persisted (the scan rereads the tags anyway).
- **Tests:**
  - folding (curly quotes, dashes, case, a phrase across a line break);
  - the minimum length;
  - the cap and the total count;
  - exclusion;
  - snippet and highlight ranges, including non-ASCII text and a match spanning two lines;
  - scan integration with lofty-written fixtures (tag, sidecar, none);
  - an `#[ignore]`d timing test over 50,000 synthetic tracks with about 1.5 KB of lyrics each, like the existing search timing test. Record the number in the module doc.

**GUI:**
- **New command:** `local_lyrics_search(query, exclude: Vec<id>) -> LyricsSearchDto`. It is separate from `local_search`, so the main results never wait for it. It runs on `spawn_blocking`, not the engine actor.
- **The search page:**
  - a **Lyrics** section below the existing sections, requested once the main results are in, so their song ids can be excluded;
  - each row shows the title, artist · album, and the snippet on a second, dimmer line, with the match emphasised (theme tokens only);
  - rows behave like the page's song rows: select, double-click or Enter plays with the same play rule as search results, the same context menu, and clickable artist and album names;
  - "and N more" when the total is over 50;
  - while the section loads it is absent, with no spinner flash.
- **Placeholder:** the search box's placeholder mentions lyrics.

**Acceptance:**
- all AGENTS.md checks pass;
- the TUI still builds and is unchanged (its search does not gain lyrics in this brief);
- smoke checks LS1–LS4 (below) are added to `apps/coda-gui/scripts/smoke.md`.

## Slice 2: Navidrome lyrics cache
**Core, new `coda_core::lyrics::cache`:**
- **File:** `$XDG_CACHE_HOME/coda/lyrics/<server key>.bin` (or JSON; the developer picks, and it must stay compact).
  - **Name:** the server key is a hash of the server URL and username, so the file name holds no URL. The file never contains URLs, credentials or tokens.
  - **Writing:** written atomically (temp file and rename), with the same permissions as the cover cache.
  - **Corrupt files:** a corrupt, unreadable or wrong-version file counts as empty and is rebuilt, with a debug log and no error shown.
  - **Size limit:** a file over a sane cap (for example 512 MB) is treated as corrupt.
- **Contents:**
  - a format version and the time of the last full refresh;
  - per album: id, song count and when it was last listed;
  - per song: id, album id and the metadata a search row and a play need (title, artist, album, album artist, disc, track, duration, cover id), the parsed lyrics, or "none" when it has none, and the time it was checked.
- **Sync,** given a `Client`. It never holds a lock across a request.
  1. Page every album with `getAlbumList2`, alphabetically, in large pages.
  2. Fetch songs (`getAlbum`) only for albums that are new or whose song count changed. Drop albums and their songs that no longer exist.
  3. Fetch lyrics only for songs never checked.
  4. Every 30 days, and on "Rebuild", check every album and song again.
- **Pace:**
  - at most 2–3 requests at a time, from its own limiter, so browsing and playback requests never queue behind it;
  - progress is saved to disk every few hundred songs, so an interrupted build continues where it stopped.
- **Errors:**
  - a transient error (timeout or 5xx) leaves that song unchecked, to retry on the next sync;
  - rejected credentials stop the sync;
  - "not found" means no lyrics.
- **Search:** the cache loads into a `LyricsIndex` (the same type and rules as local), so local and Navidrome results behave identically.
- **Tests:**
  - wiremock builds of the cache: albums over several pages, songs with timed, untimed and no lyrics;
  - the incremental case: one new album fetches only that album and its songs;
  - a song-count change refetches only that album;
  - a removed album is dropped;
  - a transient error is retried next time, and an auth error stops the sync;
  - an interrupted build resumes;
  - the file: round-trip, corrupt, wrong version, oversized;
  - no URL or credential appears in the file (search the bytes);
  - the pace limit (for example, a wiremock delay shows at most N requests in flight).

**GUI:**
- **When the sync runs:**
  - about 30 s after launch when a Navidrome server is configured, and right after a server is added or changed;
  - one sync at a time, off the engine actor, as a background task that ends cleanly on quit;
  - a server change cancels it and starts again for the new server (the cache is per server, so the old one is kept);
  - the GUI does not run a sync on a timer while the app is open; launch and Rebuild are enough.
- **New command** `navidrome_lyrics_search(query, exclude) -> LyricsSearchDto` searches the loaded cache. It is the same Lyrics section as local. Its rows play as Navidrome tracks do (built from the cached metadata, which goes through the same cleaning and caps as server data).
- **While the first build is running,** the Navidrome Lyrics section shows the matches so far, plus a quiet note: "Lyrics are still being indexed (1,200 of 2,400 songs)".
- **Settings › Navidrome** gets a "Lyrics search" block:
  - a status line: "2,341 songs indexed · up to date", "Indexing… 1,200 of 2,400 songs" or "Paused: couldn't reach the server";
  - a **Rebuild index** button;
  - an **Index lyrics for search** switch, on by default. Turning it off stops the sync, deletes this server's cache file and hides the Navidrome Lyrics section.

  The switch is stored in the GUI's preferences, through the existing preferences writer.
- **Disconnecting a server** deletes its lyrics cache file.

**Acceptance:**
- all AGENTS.md checks pass;
- smoke checks LS5–LS9 added;
- the ADR, AGENTS.md, VISION.md, both ARCHITECTURE files, the GUI README and DESIGN.md are updated.

## Owner smoke checks (to add to `smoke.md`)
1. **LS1:** local search for a phrase from a song's lyrics shows that song under **Lyrics**, with the matched line and the phrase emphasised. Double-click plays it.
2. **LS2:** search a phrase that wraps across two lyric lines: it still matches. Curly and straight apostrophes match each other.
3. **LS3:** search a song title that also appears in its chorus: the song shows under the songs results and not again under Lyrics.
4. **LS4:** a 1–2 letter search shows no Lyrics section, and typing stays smooth.
5. **LS5:** with Navidrome configured, Settings shows the index building. Searching Navidrome while it builds shows matches so far and the "still being indexed" note.
6. **LS6:** quit during the build and relaunch: it continues and does not start over.
7. **LS7:** add an album on the server and relaunch: only the new album is fetched, and its lyrics become searchable.
8. **LS8:** **Rebuild index** re-checks everything. Turning the switch off removes the section and the cache file.
9. **LS9:** browsing and playback stay responsive while the index builds.

## Out of scope
- "Play from this line" or any seeking to a match;
- lyric search in the TUI (the core API allows it later);
- fuzzy or typo-tolerant matching, diacritic folding and ranking by relevance;
- a database;
- indexing lyrics from online providers;
- server-side changes to Navidrome;
- a periodic sync while the app is open.

## Risks
- **First-build load on the server:** one lyrics request per song (plus one per album). It is paced to 2–3 requests in flight. For a 20,000-song library that is minutes to tens of minutes, once.
- **Detecting changes:** a song's lyrics changing on the server without its album's song count changing is only seen at the 30-day refresh or with Rebuild. This is accepted.
- **Older servers without `songLyrics`:** the classic `getLyrics` (by artist and title) gives plain text only and can miss or mismatch songs. Search still works on what it returns.
- **Memory:** the Navidrome cache is held in memory while the app runs. Estimate: 20,000 songs × ~1.5 KB of lyrics × about 2 (folded and original) ≈ 60 MB at most. That is the upper end of what the owner accepted, so measure it in slice 2 and report the figure. If it is clearly larger, stop and report before shipping.
