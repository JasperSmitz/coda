# Lyrics mode in the GUI's Now Playing: brief

- **Status:** approved by the owner 2026-11-12 ("everything you left out is good, I don't want any of that in anyway. You can go ahead and build it."). Branch `gui-lyrics` (based on `gui-artist-album-links`, PR #12, because both touch Now Playing).
- **Owner's words:**
  > Is it possible to have a lyrics mode for now playing? Maybe the current now playing section moves in a vertical positioning (Cover, title, artist/album, timeline) off to the left and that leaves a middle land (keep the right side open for potential queue) for the lyrics.
- **Decisions (approved):**
  1. **Layout: a left column.** In lyrics mode, the player stacks into a column on the left:
     - cover;
     - title;
     - artist · album;
     - timeline and repeat;
     - transport and volume;
     - visualizer;
     - "Up next".

     The lyrics take the middle. The right side stays free for the queue panel. Stacking the player above or below the lyrics is only the fallback for narrow windows (see Layout).
  2. **Toggle:** `l`, plus a lyrics icon button in the Now Playing toolbar. `l` works only inside Now Playing.
  3. **The mode is sticky:** it stays on across track changes until it is turned off. Like the queue panel's open state, it is remembered for the session but not across launches.
  4. **A track without lyrics keeps the layout.** The middle shows a dim "No lyrics for this track", so the layout doesn't jump on every track change.
  5. **Timed lyrics follow the song:**
     - the current line is highlighted and kept about a third of the way down;
     - clicking a line seeks to it.

     Untimed lyrics are shown as plain text you can scroll.
  6. **Scope:** the GUI only, for now. The parsing and fetching live in `coda-core`, so the TUI can add a lyrics screen later without new engine work.
- **VISION.md:** it lists lyrics as out of scope "until justified". On approval, move lyrics from that list into scope ("lyrics in the GUI's Now Playing, from tags and Navidrome"). Record the change in the ROADMAP or GUI PLAN.

## What exists (verified 2026-11-12)
- **The owner's library** (`/storage/music`, 1,157 files: 1,148 FLAC, 9 M4A):
  - 710 files have a `LYRICS` tag, and all 710 are LRC-timed (`[mm:ss.xx] text`).
  - 447 have none, mostly instrumental or ambient music.
  - There are no `.lrc` sidecar files.
- **`coda-core`:**
  - reads tags with `lofty` 0.25 (`library/scan.rs`, `library/tags.rs`);
  - has no lyrics code;
  - detects OpenSubsonic extensions once per client, but only to decide on `formPost` (`navidrome/client.rs`, `detect_form_post`).
- **GUI Now Playing** (`apps/coda-gui/src/views/NowPlayingView.tsx`, `styles/experience.css`):
  - the cover sits beside an info column, inside a box up to 1100 px wide;
  - the queue is a translucent panel about 360 px wide that overlays from the right;
  - the visualizer and "Up next" sit under the transport.
- **Playback position:** the engine sends `progress` events at most every 100 ms (`engine.rs`, `PROGRESS_INTERVAL`), and the frontend holds them in the `progress` store (`state/store.ts`). That is precise enough to change lines on time.
- **Track identity:** `TrackDto` carries the source and id. A local id resolves to a path through the scanned library, as `TrackRef` already does.

## Slice 1: core (`coda-core`), lyrics data
New module `coda_core::lyrics`:
- **Types:** `Lyrics { lines: Vec<Line>, timed: bool }` and `Line { start: Option<Duration>, text: String }`. When `timed` is true, every line has a start and the lines are sorted by start.
- **LRC parser:** `lyrics::parse(text: &str) -> Lyrics`. It must handle:
  - `[mm:ss]`, `[mm:ss.x]`, `[mm:ss.xx]`, `[mm:ss.xxx]` and `[mm:ss:xx]`;
  - several timestamps on one line (`[00:12.00][01:30.00]chorus`), giving one line per timestamp;
  - `[offset:±ms]`, which is applied to every start; the other ID tags (`[ar:]`, `[ti:]`, `[al:]`, `[by:]`, `[length:]`) are dropped;
  - enhanced LRC word stamps (`<mm:ss.xx>`), which are stripped (no word-level highlighting);
  - timed lines with empty text, which are kept as pauses;
  - CRLF and a leading BOM.

  Text with no valid timestamp lines is treated as untimed: one line per text line, with trailing blank lines trimmed. Garbage never panics.
- **Local files:** `lyrics::read_local(path: &Path) -> Result<Option<Lyrics>, …>` uses lofty's `ItemKey::Lyrics`, which covers the Vorbis `LYRICS` comment, MP4 `©lyr`, ID3 `USLT` and APE. It also reads `UNSYNCEDLYRICS`.
  - **Several values:** prefer the first timed one, otherwise the first non-empty one.
  - **Sidecars:** a sidecar `<stem>.lrc` next to the file is read only when the tags have no lyrics. Your library has no sidecars, but they are common elsewhere.
  - **Caps:** cap the text read (256 KiB) and the line count (5,000), like the existing caps on untrusted input.
  - **Calling it:** the function blocks, so it is called through `spawn_blocking`.
- **Navidrome:** `Client::lyrics(song_id) -> Result<Option<Lyrics>, Error>`.
  - **Extension detection:** generalise the detection so the client remembers the extension *set* (`formPost`, `songLyrics`). Keep `transport()` behaving exactly as now, with its tests unchanged.
  - **With `songLyrics`:** call `getLyricsBySongId?id=…` and map `lyricsList.structuredLyrics[]`. Prefer `synced: true`; `line[].start` is in ms, and `offset` is applied.
  - **Without it:** fall back to the classic `getLyrics?artist=&title=`, which returns untimed plain text, only when the track has an artist.
  - **Response handling:** treat the response as untrusted, with the same caps and the same body limit as the other endpoints.
  - **Logging:** never log the song URL or credentials (the existing redaction applies). Log ids only.
- **Tests (core):**
  - parser unit tests for every case above, including malformed and huge input;
  - `read_local` against FLAC and M4A fixtures written with lofty in a tempdir (as `cover/tests.rs` does), plus a sidecar case;
  - Navidrome against `wiremock` fixtures for: songLyrics timed, songLyrics untimed, no extension (classic fallback), empty `lyricsList`, a Subsonic error, and an oversized body.

  No real server is used.
- **Acceptance:**
  - all AGENTS.md checks pass;
  - `cargo tree -p coda-core` still has no UI crates;
  - the TUI is unchanged.

## Slice 2: GUI, lyrics mode
**Backend (`src-tauri`):**
- New command `now_playing_lyrics(track: TrackRef) -> Option<LyricsDto>`.
  - **Behaviour:** it resolves a local id through the library (a stale generation means `None`) or calls the Navidrome client. It runs off the engine actor (the actor's loop never waits on file or HTTP I/O).
  - **Errors:** an error is logged at debug and returned as an error string. The view shows it as "Couldn't load lyrics", not as "No lyrics".
- **DTO:** `LyricsDto { timed: bool, lines: [{ startMs: number | null, text: string }] }`.

**Frontend:**
- **Fetching:** a `useLyrics(track)` hook fetches when lyrics mode is on and the track changes. It keeps the last ~20 results in memory, keyed by source and id. There is no disk cache. Results for a track that is no longer playing are dropped, so the screen never shows lyrics for another track (the M10 rule).
- **Current line:** found by binary search on `positionMs` from the `progress` store. Only the lyrics component re-renders on progress, not the whole view.
- **Scrolling:**
  - smooth scroll keeps the current line about a third of the way down;
  - a manual wheel, drag or keyboard scroll pauses auto-follow for about 4 s, then it eases back;
  - with `prefers-reduced-motion`, it jumps instead of animating.
- **Lines:**
  - the current line is in the text colour; the others are dimmed (theme tokens only, `pnpm build` checks colours);
  - lines are not resized on change, so no layout shift;
  - clicking a timed line seeks there (the existing seek intent);
  - lines are not focusable (keyboard users seek with the timeline).
- **States:**
  - Loading: empty, no spinner flash under about 300 ms.
  - "No lyrics for this track".
  - "Couldn't load lyrics".
  - Nothing playing: unchanged.
- **Paused:** the highlight stays where it is.

**Layout** (`experience.css`):
- **Lyrics off:** the current layout, unchanged.
- **Lyrics on:**
  - **The left column:** the player column is about 300–340 px wide. The cover is at most the column width.
  - **The other two areas:** the lyrics fill the middle (text max ~40 ch, centred in its area). With the queue open, the queue takes its own column on the right instead of overlaying, when the window is wide enough (≥ ~1200 px). Below that it overlays as now.
  - **Narrow windows:** below ~900 px, the layout stacks. A compact player row (small cover, title, timeline, transport) goes on top and the lyrics fill the rest.
- **Visualizer and "Up next":** both stay in the left column. Check that the visualizer still reads well at the column's width. It is drawn from the same measured bands, and only the drawn bar count may change. Don't change the band measurement.
- **The toggle:** the toolbar's lyrics button shows its on state. Its tooltip is "Lyrics (l)". The aria-label names the state.
- **Moving between layouts:** a short transition (reduced motion: none).

**Docs:**
- `apps/coda-gui/docs/DESIGN.md`: the lyrics-mode layout rules.
- `apps/coda-gui/README.md`: the `l` key and where lyrics come from.
- `apps/coda-gui/scripts/smoke.md`: new owner checks (below).
- `apps/coda-gui/docs/ARCHITECTURE.md`: the command and the hook.
- `docs/ARCHITECTURE.md`: the core module.
- `VISION.md`: the scope change.

**Acceptance:**
- all AGENTS.md checks pass, including `pnpm build` (tsc and the colour check);
- frontend logic is unit-tested where the GUI has tests. At least the line lookup and auto-follow timing are pure functions with tests;
- no window is opened by agents.

## Owner smoke checks (to add to `smoke.md`; agents can't run these)
1. A local FLAC with timed lyrics:
   - press `f`, then `l`: the player moves left and the lyrics appear;
   - the highlight follows the song;
   - seeking on the timeline jumps the highlight;
   - clicking a line seeks.
2. Scroll the lyrics by hand: auto-follow pauses, then resumes.
3. An instrumental track: "No lyrics for this track", and the layout stays put. Skipping between tracks with and without lyrics doesn't jump.
4. A Navidrome track with lyrics, and one without.
5. Open the queue in lyrics mode, wide and narrow: three columns when wide, the overlay when narrow.
6. Shrink the window below ~900 px: the stacked layout.
7. Turn on reduced motion: no animated scrolling or transitions.
8. Leave Now Playing and come back: lyrics mode is still on. Restart the app: it is off.

## Out of scope (owner: not wanted at all, not just deferred)
- word-level (karaoke) highlighting;
- ID3 `SYLT` frames;
- editing or writing lyrics;
- online lyrics providers (LRCLIB and the like: a network service and a privacy decision of its own);
- translations or choosing between several languages (the first suitable value wins);
- a TUI lyrics screen;
- persisting the mode across launches.

## Risks
- **Navidrome lyrics depend on the server version.** `getLyricsBySongId` needs a recent Navidrome. The classic fallback gives only plain text. The server returns whatever lyrics it extracted from the file (embedded or a sidecar).
- **Timestamp quality varies.** Some LRC files are offset. `[offset:]` is honoured. A per-track user offset is out of scope.
- **The left column constrains the visualizer and cover.** If the visualizer looks poor at column width, stop and report rather than change its measurement.
