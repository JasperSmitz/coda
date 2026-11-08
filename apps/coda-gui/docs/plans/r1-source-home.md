# Brief (owner feedback round 1, slice B): one source at a time, and a Home screen

Read first: `AGENTS.md`, `docs/DESIGN.md` (incl. theming: every colour is a token, `pnpm build` enforces it), `docs/ARCHITECTURE.md`. Another agent is building slice A (repeat/queue header icons, immersive Now Playing) in parallel on another branch; it touches `NowPlayingBar.tsx`, `QueuePanel.tsx`, `NowPlayingView.tsx`, `App.tsx` layout. Keep your changes contained; don't reorganise shared files.

## Owner's words
> I'd like to separate navidrome and local mode in some way. Having the sidebar contain albums, artists, and playlists twice isn't very user-friendly, and I think ideally you are either playing everything locally or everything from a server anyway. A mix is a rare breed, and if so you can still switch context.
> I'd also like for there to be a home screen. It should have: recently played (artists, albums and playlists you most recently played the context of); maybe most listened to albums/playlists if possible; perhaps a random selection marked as "Daily Picks"?

## 1. Active source
- A **source switch** at the top of the sidebar (segmented control "Local | Navidrome", with icons) shown only when both sources are configured; with one source, no switch. Below it ONE set of entries for the active source: **Home**, Albums, Artists, Playlists, and Folders (local only). Settings stays at the bottom. Search (`/`) searches the active source only (with a small hint/link "Search Navidrome instead").
- Keyboard: `Ctrl/Cmd+1` Local, `Ctrl/Cmd+2` Navidrome (only if not already bound; check `useShortcuts`).
- The active source is a GUI preference persisted in `~/.config/coda-app/settings.toml` (the themes slice's settings file; extend it, keep its atomic write and FIFO-safe read). Default: Navidrome if configured, else Local. When a source gets disconnected/unconfigured in Settings, fall back to the other.
- Switching keeps a separate navigation back stack per source (switching back restores where you were).
- The queue stays mixed exactly as now (playback of both sources is unchanged); only browsing is per source. Context menus "Go to album/artist" on a track from the other source switch source first.

## 2. Play history (backend)
- Record **context plays**: whenever the user plays an album, artist, playlist or folder as a whole/from a track (PlayTracks/PlayLocal/playFrom/shuffle context…), record `{source, kind, id, name, subtitle (artist), coverUrl source/cover id, at}`. The browse play intents must carry a context descriptor for this (add an optional `context` field; the frontend fills it from the view it's playing).
- Record **listens** per context: when a track has been heard enough (reuse coda-core's scrobble `Listen` rule — half the track or 4 minutes), increment counts for the track's album (by album id/name+artist for local) and for the context it was played from.
- Store in `<state>/coda/coda-app/history.json` (same folder as the GUI session), atomic writes, debounced like the session save and included in the final save at quit; bounded (e.g. last 500 context plays, counts for at most 5,000 contexts with LRU trimming), FIFO-safe read, a corrupt file is quarantined and replaced. Never stores stream URLs or credentials.
- Tests for the recording rules, bounds and persistence.

## 3. Home view (per source)
Rows of cards (album-grid style tiles, horizontally scrollable with arrow buttons, keyboard navigable):
- **Recently played**: the latest distinct contexts (albums, artists, playlists, folders) for this source, newest first (~20). Clicking opens it; a hover play button plays it.
- **Most played**: Navidrome → the server's `AlbumSort::Frequent` list (built from scrobbles, so it includes TUI listening) — and for playlists, our own listen counts; Local → our listen counts (albums and playlists). Hide the row when empty.
- **Recently added** (Navidrome `Newest`; local: newest files by mtime if cheap, else skip) — optional, only if trivial.
- **Daily Picks**: ~12 albums chosen at random, stable for the calendar day (local timezone) and changing at midnight. Local: seeded RNG over the library's albums with the date as seed. Navidrome: fetch `AlbumSort::Random` once per day and cache the ids in `history.json` with the date; refresh at the first Home visit of a new day. A "Shuffle Daily Picks" button plays all picks' tracks shuffled.
- A friendly empty state for a new user (no history yet): show Daily Picks and Recently added and a one-line hint.
- Greeting header is optional; keep it calm (DESIGN.md).
- Home is the default landing view for each source.

## Acceptance
All AGENTS.md checks pass. No window opened; update `scripts/smoke.md`, `docs/DESIGN.md` (sidebar layout) and `docs/ARCHITECTURE.md`. Don't commit. Report to `/tmp/coda-app-r1b-report.md`.
