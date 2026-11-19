# Brief N6 — Home (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Home", "Colour" (the page glow follows the carousel focus). Builds on N1–N5 (page structure, glows, palettes, per-track counts from N5b, the mix engine from ADR 0008).

## Goal
Home becomes two things with room around them: **Where you left off** (a carousel of recent listening, with Resume) and **Moods for today** (three daily quick mixes of three albums each). The 1.x rows (Recently played, Most played, Recently added, Daily Picks) go.

## Scope

### 1. Where you left off
- Data: the history's recent contexts for the source (as `home_history` gives today), albums, playlists and mixes (artists and folders only if they have a cover; else skip them), newest first, up to 8. Each item: kind, id, name, subtitle (artist), cover URL, `playedAt`, and **where it stopped**.
- **Where it stopped**: record per context the last entry played in it (track identity + index within the context's track list) and the position, updated on track changes and on pause/quit while that context is the queue's context (the engine knows the context that filled the queue — `playTracks` with `context`, and mixes; design the smallest hook in `engine/history.rs`); bounded with the plays; saved in `history.json` (N5b made it version 2 with per-song `tracks`; bump to 3 with a migration that keeps v1 and v2 files, and leave newer versions untouched, as N5b does).
- The carousel (`views/HomeView.tsx` rebuilt, components as needed): the focused cover large (~30% of the page width, 180–330px), centred; neighbours ×0.72 then ×0.6, dimmed, fading out by the fourth; positions are transforms (no layout animation), ~0.9s `--ease`; the focused cover casts a soft glow in its accent. ←/→ move the focus while the carousel (or the page with nothing else focused) has focus — they must not seek then; the ‹ › buttons; a horizontal trackpad swipe (wheel `deltaX`, throttled); a click on a neighbour focuses it, on the focused cover opens it. Roving tab stop: one Tab stop for the carousel.
- Under it: title (display type), artist, a quiet line ("Playing now"; "Played 2 hours ago · stopped in track 4 of 9"; relative times in plain words), the dots, and **Resume** (plays the context from the stopped entry and seeks to the position; for the playing context: **Now Playing**, which opens it) and **Open album** / Open playlist / Open mix.
- The page glow (N3's `PageGlow`) and the page's `--accent` follow the focused item's cover, crossfading; the glow fades out before the Moods section (Moods rest on the base).
- The heading block: the eyebrow "Thursday evening" (weekday + morning/afternoon/evening, local time) and "Where you left off".

### 2. Moods for today
- Backend (pure picking logic in the GUI's history module, tested; I/O around it on blocking threads): for the source, rank genres by listens in recent weeks (from per-track counts and context counts, mapping albums to genres: local from the tag index; Navidrome from the album's genres, fetched or cached as needed), take the top three distinct genres (fewer history → fill with the source's largest genres); per genre choose three albums of that genre the listener has not played for longest (never-played first is fine; never one played in the last 7 days); deterministic for (source, server, date) and cached per source and day in `history.json` like the Daily Picks it replaces (remove Daily Picks and their command). **Other moods**: the next three genres for the same day (cached too).
- Each mood: genre, the three albums (id, name, artist, cover), a fact line built from data we have ("2 h 14 min · not played since August"; "one you've never played"; total length when known), and a count for the eyebrow ("46 albums in your library").
- **Play** starts a **quick mix** of the three albums (the existing `playQuickMix` path; the mix line says "Quick mix: <Genre> mood" or similar — choose a plain label); a click on a fanned cover opens that album.
- UI: three cards in a responsive grid (one column when narrow), tinted with their albums' colours (N3 palettes), the three covers fanned (they spread a little on hover; transforms only), the eyebrow, the genre in large display type, the album titles, the fact line and a round accent Play button in the card's first album's accent.

### 3. Empty and loading states
- Home renders its layout at once; the carousel and cards fill in when data arrives without moving the layout (reserve their space). A new listener (no history): the carousel area shows a short welcome line with **Shuffle your library** / **Browse albums**; Moods use the largest genres. Navidrome unreachable: the cards area says so in one quiet line.

## Out of scope
Page-to-page cover morphs (N7). The record flourish (refinement, owner).

## Acceptance
- Picks and fallback rules are deterministic per day and tested (thin history, genres with fewer than three albums, never-played albums, the 7-day rule, cache hit, other-moods); the history migration keeps old files; Resume continues the right entry and position (tested at the engine/history level).
- The carousel is keyboard and pointer usable, never seeks while it has the arrows, honours reduced motion; nothing blocks the engine; Home renders before its data.
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core` (workspace-wide if core changes).
- Docs: `docs/ARCHITECTURE.md`, `docs/DESIGN.md` (Home *(shipped)*), `README.md` (Home), `scripts/smoke.md` ("2.0 N6 — Home"; retire the 1.x Home items).
- Do not commit; report changed files, decisions, checks, open items.
