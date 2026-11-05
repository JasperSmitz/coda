# Coda App — Architecture

What exists in the code (M1 foundation and M2 browse, 2026-11-05). Proposals are marked as such.

```text
React UI ──invoke("intent", {intent})──▶ commands.rs ──mpsc(256)──▶ engine.rs (one Tokio task)
   ▲  invoke("snapshot")                                             │ Engine: Player + resolve::dispatch,
   │                                                                 │   Library + scan state, scrobble listen,
   └── events: snapshot / progress / visualizer ◀── Sink (lib.rs) ◀──┤   notices, save revision, emission
React UI ──invoke("local_*" / "navidrome_*")──▶ browse/commands.rs ──▶ BrowseHandle (shared, read-only for commands):
                                                  Library (RwLock, engine writes) + current Client, each with a generation
                                                                     │ Io: PlaybackHandle (mpv), scan rx, ping,
webview <img src="coda://localhost/cover/<token>/small">             │   Scrobbler, Saver (blocking writes), 50 ms tick
   └──▶ cover.rs (URI scheme) ──▶ coda_core::cover::load + disk cache
```

## Crates
- `crates/coda-core` (lib `coda_core`): Coda's engine, vendored. Additions for the GUI in M1: `cover::load` (async, one cover at a key and size; `spawn_load` now uses it), `cover::encode_jpeg`, `Library::track_id_by_path`, `logging::init_logging_at(path)`. In M2: `Track::album_id`/`artist_id` (Navidrome ids for navigation, set from `albumId`/`artistId`; not saved in the session), `cover_art` on Navidrome `Artist`/`Album`/`Playlist` and `artist_id` on `Album`, `TagIndex::album_of`/`artist_of` (a local track's album/artist) and `TagIndex::search` (artists/albums by name).
- `src-tauri` (`coda-app`, lib `coda_app_lib`): the backend.

## Backend modules (`src-tauri/src/`)
- `lib.rs`: Tauri builder; registers the `coda` URI scheme, spawns the engine in `setup`, manages `EngineHandle`, and owns the quit sequence.
- `bootstrap.rs`: `Paths` (GUI files under `<Coda state dir>/coda-app/`: `session.json`, `coda-app.log`; Coda's cover cache is shared), `init_logging`, and `start()`: `Config::load(Cli::default(), Env::from_process())`, password + `navidrome::Client` (`config::navidrome_client`), `playback::spawn_backend()` and the session restore, run concurrently. Every failure becomes a `ProblemDto` (`noConfig`, `config`, `mpvMissing`, `playback`, `navidrome`); nothing panics.
- `engine.rs`: the actor.
  - `Engine` is the synchronous core: `start(sources, playback_problem, restored)`, `handle_intent`, `handle_playback_event`, `handle_scan_event`/`scan_closed`, `handle_ping`, `handle_scrobble_failure`, `tick`; outputs `Vec<PlaybackCommand>` plus queued visualizer frames and scrobble requests. Every player call goes through `resolve::dispatch` (which also re-announces the gapless next after queue edits), then `take_messages()` become notices and the save revision is checked.
  - `run` is the loop: `select!` over startup completion, intents, playback events, scan events, the Navidrome ping, the first scrobble failure, and a 50 ms tick (notice expiry, session-save polling). After each branch it flushes: visualizer frames at once, scrobbles to the worker, a `snapshot` event if anything visible changed, `progress` if the position changed and ≥100 ms passed.
  - Startup runs as its own task, so the UI gets a `starting` snapshot immediately (a password command or keychain prompt can take a while). Intents other than visualizer visibility and notice dismissal are ignored until `ready`.
  - Session: revision based (`Player::revision`), saved 2 s after the last change, one blocking write at a time; restore never auto-plays; final save at exit bounded to 2 s. The GUI has its own session file, so it never overwrites the TUI's queue.
  - Scrobbling: `listen_started`/`listen_progressed` ported from the TUI's `App` (no client swap yet, so no `unscrobbled_load`).
  - Visualizer: `SetVisualizer(true)` only while the UI says a view is visible (`setVisualizerVisible`), never after `VisualizerUnavailable`; frames are forwarded raw only for `audible_load` while playing. Smoothing/decay is the UI's job (none yet: no visualizer view in M1).
  - Shutdown (`EngineHandle::shutdown`): `phase = shuttingDown` snapshot, wait ≤3 s for a still-running startup (else abort it; dropping the handle kills mpv), final save, `PlaybackHandle::shutdown(2 s)`, reply.
- `browse.rs` (+ `browse/`): everything the browse views read.
  - `BrowseHandle` (managed Tauri state, cheap `Clone`): the local `Library` behind a `std::sync::RwLock` plus a *library generation*, and the current Navidrome `Client` plus a *client generation*. The engine is the only writer: `replace_library` (start; a future rescan) bumps the generation, scan batches go through `update_library` (one short write per batch), `set_client` publishes the client at start. Commands never touch `Player` or the engine channel.
  - Local commands run their read on a blocking thread (`BrowseHandle::local`), so long reads (search, a 5,000-album list) never run on the engine task; the engine waits at most one such read for its next batch write.
  - Navidrome commands go through `BrowseHandle::fetch`: at most `MAX_CONCURRENT_FETCHES` (4, Coda's) requests at once (a semaphore); a result that arrives after the client generation changed is dropped as `stale`. Failures are logged with `Error::detail()` and returned as their `Display` text (never credentials).
  - `browse::track_dto` builds every `TrackDto` (queue and browse), so local tracks are named the same way everywhere.
  - `browse/dto.rs` (browse DTOs and `BrowseError {kind, message}`), `browse/local.rs` (queries over a `LocalView`), `browse/navidrome.rs` (queries through the client), `browse/commands.rs`, `browse/tests.rs`.
- `dto.rs`: serde DTOs (camelCase), mirrored by hand in `src/lib/types.ts`.
- `commands.rs`: `intent(intent)` and `snapshot()`; the browse commands live in `browse/commands.rs`.
- `cover.rs`: `CoverService` (registry, current client, small/large disk caches, 4 concurrent loads) and the protocol handler.

## IPC contract
- Commands: `intent({ intent })`, `snapshot()` → full `Snapshot` (queue included).
- Events:
  - `snapshot` (coalesced on the 50 ms tick, only when the snapshot differs from the last one sent). `queue` is `null` when the entry list (by key) did not change, so a 10,000-entry queue is not resent on every status change; the UI keeps its last queue. A `snapshot()` call resets this, so the next event carries the queue again (the UI applies the command's result only if no event arrived first).
  - `progress`: `{ positionMs, durationMs }` of the heard track, ≤10/s.
  - `visualizer`: `{ levels: [l, r], bands: [7 × number|null] }`, ≤30/s, only while enabled.
- `Intent` (tagged by `type`): `togglePause`, `next`, `previous`, `seekTo{seconds}`, `seekBy{seconds}`, `setVolume{volume}`, `cycleRepeat`, `playEntry{key}`, `removeEntry{key}`, `clearQueue`, `setVisualizerVisible{visible}`, `playTracks{tracks, start?, shuffle?}`, `enqueue{tracks, next?}`, `dismissNotice{id}`, `shuffleLibrary` (local "Shuffle all": every scanned track shuffled, a random sample at `MAX_QUEUE_LEN`, as in Coda).
- Browse commands (all async, `Result<DTO, BrowseError>`; `BrowseError.kind` is `notConfigured` | `stale` | `notFound` | `failed`):
  - Local: `local_artists`, `local_albums{order: "name"|"artist"}`, `local_artist{id}` → `{artist, albums}`, `local_artist_tracks{id}` (every album A–Z, each in album order: what `s` plays), `local_album{id}` → `{album, tracks}`, `local_folder{path}` → `{path, dirs, tracks}` (one level), `local_folder_tracks{path}` (everything below), `local_playlists`, `local_playlist{id}` → `{playlist, tracks, missing, truncated, hidden, warning}`, `local_search{query}` → `{artists, albums, tracks, totalTracks}` (tracks: `library::search`, at most 1,000; artists/albums: `TagIndex::search`, at most 50 each). Scan state is in the snapshot (`scan`), not a command.
  - Navidrome: `navidrome_artists`, `navidrome_artist{id}` → albums, `navidrome_artist_tracks{id}` (albums fetched in parallel within the fetch limit), `navidrome_album{id}`, `navidrome_albums{sort, offset}` → `{albums, nextOffset}` (pages of 100, the six `AlbumSort`s, stops at `MAX_ALBUMS`), `navidrome_playlists`, `navidrome_playlist{id}` (with `hidden`: Coda's `Hidden` as `{kind: none|some{count}|suspect}`, and `truncated`), `navidrome_random_songs` (Navidrome "Shuffle all": the UI plays them with `playTracks`), `navidrome_search{query}` (`search3`, 20/20/50).
  - Covers for albums, artists and playlists go through the same registry (`CoverService::url_for_key`): local = the first track's art, Navidrome = the item's `coverArt`.
- **Local ids**: `"<generation>:<n>"` for tracks, artists, albums and playlists (indices into the library, valid while it only grows). An id from another generation is `stale` (commands) or skipped with the notice "The library changed; N tracks skipped. Reload the view" (intents); a malformed or out-of-range id is `notFound` / "not in the library". Folders are named by their path of folder names from the root.
- **Queue keys**: `QueueEntryId`'s `Display` (`q12`): stable for the entry's life in this run (React key, intent target), not across restarts.
- **`TrackDto`** also carries `albumId` / `artistId` (local ids, or Navidrome's) for "Go to album/artist"; `null` when unknown. Queue DTOs resolve a local track's id by path when the queue is sent, so a track restored before the scan found it has `id: null` until the queue changes.
- **`TrackRef`**: the shape of a `TrackDto` minus `coverUrl` (`{ source, title?, artist?, … durationMs?, albumId?, artistId? }`, extra fields ignored), so the UI hands back the tracks a browse call gave it. `source` is `{kind:"local", id}` or `{kind:"navidrome", songId, coverArt}`. Local refs are looked up in the scanned `Library` by id and generation, so non-UTF-8 paths play (metadata comes from the scan; unknown or stale ids are skipped with a notice). Navidrome refs are rebuilt with `Track::navidrome` from the given metadata (cleaned and capped like server data; `albumId`/`artistId` kept, bounded), since the backend keeps no browse cache.
- Notices: `{id, text}`; the backend drops them after 5 s (max 4), `dismissNotice` drops one early.

## Covers
- DTOs carry `coverUrl` = `coda://localhost/cover/<16 hex>` (`http://coda.localhost/...` on Windows/Android); the UI appends `/small` (256 px) or `/large` (1024 px).
- The token is a per-run keyed hash of the `CoverKey`; the registry (≤25,000 tokens, oldest dropped) is filled when DTOs are built, so the webview can only fetch covers of tracks it was shown and never names a path. Unknown tokens → 404.
- Local: embedded art, else a folder image. Navidrome: `getCoverArt` via the current client, cached in Coda's disk cache (shared with the TUI). Served as JPEG with `Cache-Control: private, max-age=86400`. `[ui] cover_art = false` turns URLs off.

## Lifecycle
- Window close / Cmd+Q / last window closed → `RunEvent::ExitRequested`: prevent the exit, run `EngineHandle::shutdown`, then `app.exit(0)`.
- `RunEvent::Exit` without a finished shutdown (a quit path that skipped `ExitRequested`): blocking, bounded (8 s) shutdown on the event-loop thread.
- mpv is also spawned with `kill_on_drop`.

## Security
- Capabilities: `core:default` only (events, app/window basics); the opener plugin was removed.
- CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' coda: http://coda.localhost data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` (`devCsp` adds the Vite HMR socket). `freezePrototype` on.
- DTOs carry no passwords, tokens or stream URLs (a test asserts the snapshot JSON of a Navidrome track has none). Server/tag text is rendered as React text only.

## Frontend (`src/`)
- `lib/types.ts` (DTO mirror, browse block at the end), `lib/ipc.ts` (`intent()`, `fetchSnapshot()`, `onSnapshot/onProgress/onVisualizer`, `coverSrc`), `lib/format.ts`, `lib/browse.ts` (typed browse commands; Navidrome answers cached in memory for 5 min so Back is instant, never the random ones; Retry clears it), `lib/play.ts` (`playTracks`/`enqueue`/`shuffleAll`, `sameTrack` for the playing row, Go to album/artist, the `s` handler a view registers with `useShuffleContext`).
- `state/nav.ts`: the navigation back stack (`Route`s; each entry keeps its scroll position). The sidebar sets a new root; views push; Back pops.
- `state/store.ts`: two `useSyncExternalStore` stores (app snapshot + queue; progress separately so the position does not re-render the queue). `connect()` subscribes first, then fetches the snapshot.
- `components/`: `Sidebar`, `NowPlayingBar` (cover, title/artist, prev/play/next, seek bar, repeat, volume, queue toggle), `Slider` (pointer drag + own keys), `QueuePanel` (virtualized 36 px rows; double-click/Enter plays, Delete/hover button/context menu removes), `ContextMenu`, `Banner` (problems), `Toasts` (notices), `Cover` (+ generated placeholder), `icons.tsx`.
- `components/` (M2): `TrackTable` (virtualized 36 px rows inside the view's scroller, disc headers, playing row equaliser, ↑/↓/Enter/double-click, context menu and "…" button), `TrackMenu` (Play, Play next, Add to queue, Go to album, Go to artist; a marked `ADD TO PLAYLIST` spot), `AlbumGrid` (virtualized ~180 px tiles, arrow-key focus, album menu, `onEnd` for infinite scroll), `LoadState` (skeletons, empty, error with Retry / Go back).
- `views/`: `BrowseView` (the stack's top), `ArtistsView` (virtualized list + letter index), `AlbumsView` (local orders; Navidrome sorts with infinite scroll), `ArtistView`, `AlbumView`, `PlaylistsView`/`PlaylistView` (warnings), `FolderView` (breadcrumbs), `SearchView` (debounced 250 ms, grouped per source), `common.tsx` (header with back button, Play/Shuffle, scan line); `PlaceholderView` remains for Settings. Local views reload quietly at most every 2 s while the scan grows the library (`useLibraryVersion`). The sidebar hides a source that is not configured.
- `hooks/useShortcuts.ts`: Space, n/p, Ctrl+→/←, ←/→ (∓5 s), +/=/-, r, q; ignored while typing in a text field. `hooks/useBrowseKeys.ts`: Back (Backspace, Alt+←, Escape, mouse button 3), `/` and Ctrl+F (search), `s` (shuffle the shown context). `hooks/useLoad.ts`, `hooks/useLayout.ts` (element size, scroll restore, library version).
- `styles/`: `tokens.css` (dark first, light via `prefers-color-scheme`, reduced motion), `base.css` (native feel), `app.css` (shell).

## Tests
- `src-tauri`: DTO serialization (`dto.rs`), cover registry/protocol (`cover.rs`), paths (`bootstrap.rs`), engine core with fake playback events (`engine/tests.rs`: restore never plays, intents via the player, keys, `TrackRef` resolution, shuffle start, enqueue placement, Navidrome refs + scrobbling, mpv missing, backend exit, visualizer gating, save debounce, snapshot coalescing, progress throttle, notices, scan failure), the loop end-to-end without mpv, and an `#[ignore]`d real-mpv shutdown test (no process left, session saved). M2 adds: stale-generation refs rejected with a notice, queue DTOs by id without paths, `shuffleLibrary`, Navidrome album ids surviving refs; `browse/tests.rs`: the id/generation rule, every local query over a synthetic library, and Navidrome queries against `wiremock` (artists/albums/tracks, paging, playlists with the hidden warning, search, random songs, failures, stale results dropped after a client change, the 4-request cap).
- `coda-core`: `album_of`/`artist_of`, tag search, and the new Navidrome id/cover fields.
