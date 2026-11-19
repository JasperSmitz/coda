# Brief N5 — Library, pages and Search (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Library", "Search", "Colour" (page glow, per-tile accents). Builds on N1–N4. Two parts, each reviewed and committed on its own: **N5a** (page structure, headers, Search) then **N5b** (Library sections, Covers/List, sorts, most played).

## N5a — Pages scroll under the top bar; headers; Search

### 1. One scroll container per page
- Today every view has a fixed `.view-header` and a scrolling `.view-body` below the top bar (`.main` padded by `--topbar-h`). Make the page one scroll container whose content starts with its header: the header scrolls away, the top bar floats over the content (the N1 fade of the base under the bar stays), the player floats over the bottom. Top padding keeps the header clear of the top bar at scroll top; bottom padding keeps the last row clear of the player.
- Keep what the fixed headers gave: tools that must stay reachable on long lists (sorts, Covers/List, filters) go into a compact **sticky tool row** that sticks just under the top bar once the big header has scrolled away (glass, the same tools). Virtualized lists (`ArtistsView`, long track tables, paged Navidrome albums) must keep virtualizing against the new scroll container (they measure their scroller today; pass it the page's). Scroll restore per entry (nav `scrollTop`) and keyboard scrolling keep working.
- The N3 page glow now scrolls with the page (it sits at the top of the scroll content, behind the header).

### 2. Page headers (album, artist, genre, playlist, mix, folder)
- The pattern of DESIGN.md "Library": a large cover (~270px; round for artists; a 2×2 mosaic for playlists without their own cover), the eyebrow ("Album · 1977", "Artist", "Genre · 46 albums", "Playlist", "Mix"), the title in large display type (`clamp(36px, 5.4vw, 72px)`, balanced, wraps up to 3 lines), the byline (artist link; for albums genres as links in the meta line), the meta line ("9 songs · 48 min"), then the actions: **Play** (accent pill), **Shuffle** (glass pill), "…" (the existing menu: Add to playlist…, Add to mix…, …). Narrow windows stack the cover above the text.
- Existing header actions and menus are kept, only placed in this pattern. The cover morph target (N7) gets a stable class/id on the header cover.
- Section roots (Albums, Artists, …) keep the N1 Library header ("Library" + section control) in the new style: the title in display type, the section control and tools beside it.

### 3. Search
- A large rounded glass field at the top of the page (`/` and `Ctrl+F` focus it), the placeholder per DESIGN.md. Empty: the source's genres as chips (from the existing genre list command) that open genre pages.
- Results: **Top result** (a card: the best album or artist match, large cover, name in display type, "Album · artist") beside **Songs** (up to 5 rows with cover, title, artist · album, time, the track menu); then Albums (cover grid), Artists (round images), then **Lyrics** with every 1.x rule (DESIGN.md "Search"). "Search … instead" stays. Keyboard navigation of results as today.

### Acceptance (N5a)
- No view keeps a fixed header; long lists stay virtualized and smooth; scroll restore on Back works; sticky tools reachable; nothing hidden under the bars at either end; the glow scrolls.
- Search keeps its lyrics behaviour and the source link; results are keyboard reachable.

## N5b — Library sections, Covers / List, sorts, most played

### 1. Covers / List
- Albums, Artists and a genre's albums get a **Covers / List** toggle (icon segmented control in the tool row), remembered per section and source; the default comes from a new Preferences "Library" card ("Show albums and artists as Covers / List"; `settings.toml` `libraryView`).
- **Covers**: tiles ≥178px, cover with soft shadow, title and "artist · year"; hover lifts the cover and shows a round Play button; Artists as round images. **Per-tile accents**: the visible tiles' palettes are fetched with `cover_palettes` (N3; batched per frame, only for tiles in view) and set each tile's `--accent`/`--accent-text` (house accent until known; no transition on first set).
- **List**: dense virtualized rows (≈52px: small cover, title, artist, genre, year or plays, length), sortable by the current sort; Enter/double-click opens, the menu works, Shift+Enter plays (as cards do).

### 2. Sorts
- Albums: local **A–Z**, **Artist**, **Year**, **Most played**; Navidrome **A–Z**, **Artist**, **Recently added** (`newest`), **Most played** (`frequent`), **Recently played** (`recent`), **Random**. A compact select or segmented control in the tool row; remembered per source. In List, Most played shows a plays column (local: our counts; Navidrome: the server's album `playCount` if the DTO has it, else hide the column).

### 3. Play counts per song (history)
- The history counts listens per **context** today. Add per-**track** listen counts (stable keys: local by the track's stable identity as history already does for contexts — path or tag key; Navidrome by song id + server), bounded (the least recently heard dropped first, ~20,000), recorded where listens are recorded now, saved in `history.json` (bump the file version with a migration that keeps old data). Untrusted-input and privacy rules as today (no URLs, no tokens).
- Local **Most played** album sort = the sum of the album's tracks' listens.
- **Your most played** on the artist page (both sources): up to six of the artist's songs by our per-track counts, with the count ("12 plays"), hidden without plays; double-click/Enter plays from that song in the album's context.

### Acceptance (N5b)
- Covers/List per section and its Preferences default persist; List stays fast with 10,000+ albums; per-tile accents never block scrolling and cap requests to visible tiles.
- Sorts match each source; counts are tested (record, bound, migrate, sum per album, top songs per artist); the history file round-trips.

## Both parts
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core` (workspace-wide if core changes).
- Docs: `docs/ARCHITECTURE.md`, `docs/DESIGN.md` (*(shipped)* markers), `README.md` (Library and Search), `scripts/smoke.md` ("2.0 N5a/N5b" sections).
- Do not commit; report changed files, decisions, checks, open items.
