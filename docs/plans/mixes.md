# Mixes: brief

- **Status:** approved by the owner 2026-11-16 ("merge genres into main first, then go ahead"). Branch `mixes` (from `main` after PR #15).
- **Owner's words:**
  > It would be neat if you could make a "mix", a kind of pseudo-playlist. You could select for example "Ah, I feel like listening to indie rock and prog rock today." So you'd be able to somehow select both genres and shuffle from both sets. That could then also be expanded to: "I want to listen to the big three, King Crimson, Yes, and Genesis today."

  and on scope (2026-11-16): "one source per mix, add albums, exclusions later, TUI is fine" (the TUI plays saved mixes and makes quick mixes; creating and editing saved mixes is in the GUI).
- **Why now:** Mixes are the second item under "Direction" in `docs/VISION.md`, built on genres (M11).

## Decisions (approved)
1. **A mix is a saved recipe:** a name, one source (Local, or the Navidrome server it was made on) and a list of ingredients. Each ingredient is a **genre**, an **artist** or an **album**. It is resolved each time it plays, so music added later joins in.
2. **One source per mix.** The owner's local folder and server hold the same music, so mixing both would offer every song twice. Combining sources (with matching of the same song) is later work; `VISION.md` is updated to say so.
3. **Ingredients are stored by name,** never by library ids (local ids change with every scan):
   - Local: genre name; artist name; album as artist + album name. Matched case-insensitively, like `TagIndex` groups them.
   - Navidrome: genre by its exact server value; artist and album by server id **plus** name, falling back to the name (search) when the id no longer exists.
   - An ingredient that matches nothing is skipped; the mix plays the rest and says which ingredient found no songs.
4. **Balance:** ingredients take turns. Each round visits every ingredient once, in a new random order, and each turn picks a random song from that ingredient. "King Crimson, Yes and Genesis" is then a third each, whatever the album counts. A song that fits two ingredients is not favoured.
5. **Variety:**
   - no song repeats until a window of recent picks has passed (about half the mix's songs, at most 200);
   - the next song avoids the previous song's album, and then its artist, when the ingredient has an alternative.
6. **Endless:** starting a mix queues 25 songs and plays the first. Whenever 5 or fewer songs remain after the current one, 20 more are added. When the queue nears its 10,000 cap, the oldest already-played entries are removed first. Repeat modes are left alone (the top-up happens before the end, so Repeat All never wraps).
7. **When a mix stops:** anything that replaces the queue (playing an album, a playlist, Shuffle all, another mix) or clears it ends the mix. Adding songs to the queue, reordering, removing or skipping does not. The current mix is saved with the session, so it keeps topping up after a restart.
8. **Quick mixes:** a mix can be played without saving it. In the GUI the mix editor has **Play** next to **Save**; in the TUI you mark genres, artists or albums and play the marked ones as a mix. A quick mix lives only in the session.
9. **Storage:** saved mixes live in `$XDG_DATA_HOME/coda/mixes.json` (else `~/.local/share/coda/mixes.json`): user data that both apps read. It is the first file both apps write, so every change re-reads the file, applies that one change (add, update or delete by mix id) and writes it atomically. Recorded in ADR 0008. No URLs or credentials are stored; a Navidrome mix records a hash of its server, like the GUI history.
10. **Order:** core, then the GUI (the full editor), then the TUI (play and quick mix), one slice each, with both frontends building after every slice.

## What exists (verified 2026-11-16)
- **Queue and player** (`queue.rs`, `player.rs`): `enqueue_all(tracks, Placement::End)` appends without starting playback, and `resolve::dispatch` re-announces the next track after any queue edit (ADR 0005), so a top-up keeps gapless playback. At the end of the queue the player stops (`advance` → `clear_load`). Only `play_context` and `clear_queue` clear the queue, but nothing tells a caller that the queue was replaced; `revision()` also changes on ordinary edits. `MAX_QUEUE_LEN = 10_000`, enforced by `enqueue_all` (it silently caps).
- **Randomness** (`queue.rs`): `Rng` (SplitMix64, seedable for tests), `sample`, `shuffle_with_first`. No round-robin or weighted picking yet.
- **Local ingredients** (`library/tags.rs`): `ArtistId`, `AlbumId` and `GenreId` are dense scan-order indices that do not survive a rescan; there is no public lookup by name. Accessors exist for an artist's albums, an album's tracks and `genre_tracks(id)`. The GUI history already uses durable keys (`local_artist_key`, `local_album_key`, `history.rs:645-653`).
- **Navidrome ingredients** (`navidrome/client.rs`): `random_songs_in_genre` (500 random songs per call), `artist(id)` (its albums), `album(id)` (its songs), `search`. An artist's songs cost 1 + one request per album. Genres have no id; the exact value is the key.
- **Frontends:** the TUI applies Navidrome results through `FetchRequest`/`JobOutcome` with a generation check (`navidrome/browse.rs`, `app.rs:1993`). The GUI engine actor has no fetch arm today: Navidrome browsing runs in Tauri commands. The engine already tracks which queue entries came from the last play (`played_from`, `engine/history.rs`).
- **Persistence:** four private atomic-write helpers in core (`session.rs:588`, `cover/cache.rs`, `lyrics/cache.rs`, `config_writer.rs`) and one in the GUI (`theme/fsio.rs`). No file is written by both apps yet.
- **Selection:** neither the GUI's genre, artist and album views nor the TUI has multi-select (the GUI queue does, `useSelection.ts`).

## Slice 1: core (`coda-core`)
- **`coda_core::mix` module:**
  - **Model:** `Mix { id, name, source: MixSource, ingredients: Vec<Ingredient> }`; `MixSource::{Local, Navidrome { server: String /* hash */ }}`; `Ingredient::{Genre { name }, Artist { name, id: Option<String> }, Album { artist, name, id: Option<String> }}`. Ids are random, generated by Coda. Caps: name and text lengths as elsewhere; at most 50 ingredients per mix and 500 mixes.
  - **Store:** `load(path)`, and `add`, `update`, `delete` that each re-read, apply and write atomically (one shared `write_atomic` helper, extracted from the existing copies where that is a straight move). A bad file is kept aside like `state.json.bad.<secs>` and an empty list is used. Versioned JSON. Blocking: called through `spawn_blocking`.
  - **Picking** (pure, seeded in tests): `Picker` holds one pool of candidate tracks per ingredient, the round order, the recent-picks window and the previous pick; `next(n, rng) -> Vec<Track>` implements decisions 4 and 5. Empty pools are skipped.
  - **Local resolver:** ingredient → pool from the `Library` (new `TagIndex` lookups by name for artist, album and genre, matching its own grouping keys).
  - **Navidrome resolver:** async functions using `Client`. Genre: `random_songs_in_genre`, refetched when its pool runs out. Artist: `artist(id)` then each album, cached for as long as the mix plays, with the name fallback through `search`. Album: `album(id)`, same fallback. Concurrency within the existing fetch limits.
  - **Active mix:** `ActiveMix { recipe, picker, queue_context }` with `wants_top_up(&Player) -> Option<usize>` (decision 6) and the trim of played entries near the cap.
- **Queue:** a `context` counter bumped by `Queue::clear`, exposed by the player, so an active mix knows when the queue was replaced (decision 7). A `remaining_after_current()` helper.
- **Session:** `EntryFile`-style optional field for the active mix (its id, or the recipe for a quick mix), backward compatible like the genres field.
- **Tests:** model limits and malformed files; the store's concurrent-edit rule (two writers, each change kept); picking: balance across uneven ingredients, overlap not favoured, the recent window, album/artist spacing, empty pools, determinism with a seed; local resolution by name across a rescan; Navidrome resolution with wiremock (genre refetch, artist fan-out, id fallback by name, failures); top-up thresholds, the cap trim, and replace detection.
- **ADR 0008:** the shared mixes file and the queue top-up (ownership, replace detection, the cap).
- **Acceptance:** all AGENTS.md checks; both frontends build and behave as before.

## Slice 2: GUI
- **Engine:** intents to play a mix (saved by id, or a quick recipe); the engine owns the active mix, runs local top-ups inline and Navidrome top-ups on a new fetch arm (a task holding a cloned `Client`, checked by generation on return), so topping up never depends on the webview.
- **Commands:** list, add, update and delete mixes; search the current source's genres, artists and albums for the editor.
- **Frontend:**
  - a **Mixes** sidebar entry (per source, beside Playlists): the saved mixes as cards with their ingredients as chips, Play on each, and **New mix**;
  - the **mix editor**: a name, and pickers for genres, artists and albums (search as you type) shown as removable chips; **Play** (quick mix) and **Save**;
  - **Add to mix…** in the context menus of genres, artists and albums: pick a saved mix or start a new one with it;
  - Now Playing and the queue show "Mix: <name>" while a mix is active.
- **Tests:** engine tests for start, top-up, replace ends the mix, restart restores it; command tests; `pnpm build`, `pnpm check:colors`; owner runtime checks in `scripts/smoke.md`.

## Slice 3: TUI
- **Mixes** on both Home menus: the saved mixes for that source (from the shared file); Enter or `s` plays one.
- **Quick mix:** a key marks the selected genre, artist or album row (marked rows show a marker), and a second key plays the marked rows as a mix; the marks clear afterwards. Keys chosen from free ones in slice 3, shown in the help overlay.
- The status bar shows the active mix's name.
- **Tests:** browser and app tests; a pty smoke run with a temporary library and a mixes file.

## Docs
- `docs/VISION.md`: Mixes play from one source, with genres, artists and albums; combining sources later.
- `docs/ARCHITECTURE.md`, `apps/coda-gui/docs/ARCHITECTURE.md`, `DESIGN.md`; `docs/USAGE.md`, README; ADR 0008.
- `docs/ROADMAP.md`: "M12 — Mixes"; `apps/coda-gui/docs/PLAN.md`: a dated "Mixes" section.

## Not in this milestone
- Combining local and Navidrome in one mix.
- Exclusions ("Prog Rock, but not Yes") and weights ("more Yes").
- Other ingredients (years, playlists, decades).
- Endless play for ordinary albums and playlists, and autoplay.
- Mixes in the GUI's play history.
- Creating or editing saved mixes in the TUI.
