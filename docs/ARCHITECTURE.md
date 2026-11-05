# Coda App — Architecture

What exists in the code (M1 foundation, 2026-11-05). Proposals are marked as such.

```text
React UI ──invoke("intent", {intent})──▶ commands.rs ──mpsc(256)──▶ engine.rs (one Tokio task)
   ▲  invoke("snapshot")                                             │ Engine: Player + resolve::dispatch,
   │                                                                 │   Library + scan state, scrobble listen,
   └── events: snapshot / progress / visualizer ◀── Sink (lib.rs) ◀──┤   notices, save revision, emission
                                                                     │ Io: PlaybackHandle (mpv), scan rx, ping,
webview <img src="coda://localhost/cover/<token>/small">             │   Scrobbler, Saver (blocking writes), 50 ms tick
   └──▶ cover.rs (URI scheme) ──▶ coda_core::cover::load + disk cache
```

## Crates
- `crates/coda-core` (lib `coda_core`): Coda's engine, vendored. Additions for the GUI in M1: `cover::load` (async, one cover at a key and size; `spawn_load` now uses it), `cover::encode_jpeg`, `Library::track_id_by_path`, `logging::init_logging_at(path)`, `Drop for PlaybackHandle` (SIGTERM to a still-running mpv), and on Linux `PR_SET_PDEATHSIG` in `mpv_command`.
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
  - Shutdown (`EngineHandle::shutdown`): `phase = shuttingDown` snapshot, wait ≤3 s for a still-running startup (else abort and await it, so a `PlaybackHandle` it holds is dropped and mpv terminated), final save, `PlaybackHandle::shutdown(2 s)`, reply.
- `dto.rs`: serde DTOs (camelCase), mirrored by hand in `src/lib/types.ts`.
- `commands.rs`: `intent(intent)` and `snapshot()`; nothing else.
- `cover.rs`: `CoverService` (registry, current client, small/large disk caches, 4 concurrent loads) and the protocol handler.

## IPC contract
- Commands: `intent({ intent })`, `snapshot()` → full `Snapshot` (queue included).
- Events:
  - `snapshot` (coalesced on the 50 ms tick, only when the snapshot differs from the last one sent). `queue` is `null` when the entry list (by key) did not change, so a 10,000-entry queue is not resent on every status change; the UI keeps its last queue. A `snapshot()` call resets this, so the next event carries the queue again (the UI applies the command's result only if no event arrived first).
  - `progress`: `{ positionMs, durationMs }` of the heard track, ≤10/s.
  - `visualizer`: `{ levels: [l, r], bands: [7 × number|null] }`, ≤30/s, only while enabled.
- `Intent` (tagged by `type`): `togglePause`, `next`, `previous`, `seekTo{seconds}`, `seekBy{seconds}`, `setVolume{volume}`, `cycleRepeat`, `playEntry{key}`, `removeEntry{key}`, `clearQueue`, `setVisualizerVisible{visible}`, `playTracks{tracks, start?, shuffle?}`, `enqueue{tracks, next?}`, `dismissNotice{id}`.
- **Queue keys**: `QueueEntryId`'s `Display` (`q12`): stable for the entry's life in this run (React key, intent target), not across restarts.
- **`TrackRef`**: the shape of a `TrackDto` minus `coverUrl` (`{ source, title?, artist?, … durationMs? }`, extra fields ignored), so the UI hands back the tracks a browse call gave it. `source` is `{kind:"local", path}` or `{kind:"navidrome", songId, coverArt}`. Local refs are looked up in the scanned `Library` by path (metadata comes from the scan; unknown paths are skipped with a notice). Navidrome refs are rebuilt with `Track::navidrome` from the given metadata (cleaned and capped like server data), since the backend keeps no browse cache.
- Notices: `{id, text}`; the backend drops them after 5 s (max 4), `dismissNotice` drops one early.

## Covers
- DTOs carry `coverUrl` = `coda://localhost/cover/<16 hex>` (`http://coda.localhost/...` on Windows/Android); the UI appends `/small` (256 px) or `/large` (1024 px).
- The token is a per-run keyed hash of the `CoverKey`; the registry (≤25,000 tokens, oldest dropped) is filled when DTOs are built, so the webview can only fetch covers of tracks it was shown and never names a path. Unknown tokens → 404.
- Local: embedded art, else a folder image. Navidrome: `getCoverArt` via the current client, cached in Coda's disk cache (shared with the TUI). Served as JPEG with `Cache-Control: private, max-age=86400`. `[ui] cover_art = false` turns URLs off.

## Lifecycle
- Window close / Cmd+Q / last window closed → `RunEvent::ExitRequested`: prevent the exit, `begin_quit` (run `EngineHandle::shutdown`, then `app.exit(0)`).
- SIGTERM, SIGINT and SIGHUP (`watch_signals` in `lib.rs`) take the same `begin_quit` path; a second signal while quitting terminates mpv and exits at once.
- `RunEvent::Exit` without a finished shutdown (a quit path that skipped `ExitRequested`): blocking, bounded (8 s) shutdown on the event-loop thread.
- A startup still running at exit gets 3 s, then is aborted *and awaited*, which drops a finished-but-unclaimed `PlaybackHandle` before the reply.
- `coda-core`: dropping a `PlaybackHandle` without `shutdown` while its backend runs sends mpv SIGTERM synchronously; on Linux mpv is spawned with `PR_SET_PDEATHSIG = SIGTERM` (covers SIGKILL and aborts); mpv is also spawned with `kill_on_drop`.
- Panics: the app records mpv's pid (`bootstrap::MPV_PID`); a panic hook terminates it when the panic ends the process (always under the release profile's `panic = "abort"`, or on the main thread).
- Session saves that fail are retried with back-off (2 s doubling to 60 s), not only at exit.

## Security
- Capabilities (`capabilities/default.json`): exactly `allow-intent`, `allow-snapshot` (our commands, declared via `AppManifest::commands` in `build.rs`, so none is allowed implicitly), `core:event:allow-listen`, `core:event:allow-unlisten`. No emit (the webview cannot forge engine events), menu, tray, path, window or webview APIs. The opener plugin was removed.
- CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' coda: http://coda.localhost data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` (`devCsp` adds the Vite HMR socket). `freezePrototype` on.
- DTOs carry no passwords, tokens or stream URLs (a test asserts the snapshot JSON of a Navidrome track has none). Server/tag text is rendered as React text only.

## Frontend (`src/`)
- `lib/types.ts` (DTO mirror), `lib/ipc.ts` (`intent()`, `fetchSnapshot()`, `onSnapshot/onProgress/onVisualizer`, `coverSrc`), `lib/format.ts`.
- `state/store.ts`: two `useSyncExternalStore` stores (app snapshot + queue; progress separately so the position does not re-render the queue). `connect()` subscribes first, then fetches the snapshot.
- `components/`: `Sidebar`, `NowPlayingBar` (cover, title/artist, prev/play/next, seek bar, repeat, volume, queue toggle), `Slider` (pointer drag + own keys), `QueuePanel` (virtualized 36 px rows; double-click/Enter plays, Delete/hover button/context menu removes), `ContextMenu`, `Banner` (problems), `Toasts` (notices), `Cover` (+ generated placeholder), `icons.tsx`.
- `views/`: `PlaceholderView` for every browse entry (M2).
- `hooks/useShortcuts.ts`: Space, n/p, Ctrl+→/←, ←/→ (∓5 s), +/=/-, r, q; ignored while typing in a text field. Controls and shortcuts that send intents are disabled until the engine is `ready` (and transport ones while playback is unavailable).
- `styles/`: `tokens.css` (dark first, light via `prefers-color-scheme`, reduced motion), `base.css` (native feel), `app.css` (shell).

## Tests
- `src-tauri`: DTO serialization (`dto.rs`), cover registry/protocol (`cover.rs`), paths (`bootstrap.rs`), engine core with fake playback events (`engine/tests.rs`: restore never plays, intents via the player, keys, `TrackRef` resolution, shuffle start, enqueue placement, Navidrome refs + scrobbling, mpv missing, backend exit, visualizer gating, save debounce, snapshot coalescing, progress throttle, notices, scan failure), the loop end-to-end without mpv, and an `#[ignore]`d real-mpv shutdown test (no process left, session saved).
