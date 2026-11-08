# Brief: M1 foundation — engine actor, IPC, cover protocol, app shell

Read first: `AGENTS.md`, `docs/PLAN.md`, `docs/DESIGN.md`, and the engine map `/tmp/coda-core-map.md` (written for the original `coda_player` crate; in this repo the crate is `crates/coda-core`, lib `coda_core`, without `app`/`setup`/`ui`; `logging`, `setup_actions`, `Player::seek_absolute` and `Player::set_volume` exist). The TUI's orchestration in `../coda/src/app.rs` and `../coda/src/main.rs` is the reference for how to drive the engine: read it, never modify anything in `../coda`.

This slice sets the patterns every later slice follows, so favour clear structure over breadth.

## Backend (`src-tauri/src/`)
1. **Bootstrap** (`bootstrap.rs`): `Config::load(Cli::default(), &Env::from_process())`, logging via `coda_core::logging` to the GUI's own log file, password resolve + `navidrome::Client`, `playback::spawn_backend()`. The GUI shares Coda's `config.toml` (music folder, Navidrome account) but uses **its own session file** (`coda-app/session.json` under the same state dir Coda uses) so the TUI and GUI never overwrite each other. A missing config (`ConfigError::NoSource`) or missing mpv must not crash: start in a degraded state and report it in the snapshot (`problems` list) so the UI can show a banner (onboarding comes in a later slice).
2. **Engine actor** (`engine.rs`): one tokio task owning `Player`, `PlaybackHandle`, `Option<Client>` (+ generation), `Library` + scan state (start `scan::spawn_scan` when a music dir is configured), scrobbling (port `listen_started/listen_progressed` + `Scrobbler` from the TUI app), visualizer gating (`SetVisualizer(true)` only while a view says it's visible), session restore (never auto-play) and debounced save (revision based, 2 s, one write at a time, final bounded save at exit). `select!` over: intents (`mpsc`), playback events, scan events, timers. All player calls go through `Player` + `resolve::dispatch`; drain `take_messages()` into notices.
3. **Intents** (`Intent` enum) for this slice: TogglePause, Next, Previous, SeekTo(secs), SeekBy(secs), SetVolume(u8), CycleRepeat, PlayEntry(entry index or stable id), RemoveEntry, ClearQueue, SetVisualizerVisible(bool), PlayTracks{tracks: Vec<TrackRef>, start, shuffle}, Enqueue{tracks, next: bool}. `TrackRef` identifies a track the UI got from a browse call (local track id/path or Navidrome song id + metadata) — design it so later browse slices can return lists of them.
4. **Events to the UI** (Tauri `emit`), DTOs in `dto.rs` (serde, camelCase):
   - `snapshot`: status, current/playing entry, queue (items with stable keys), volume, repeat, scan state + counts, navidrome connection state, problems, notices (id + text). Coalesce: at most every ~50 ms, only when something changed.
   - `progress`: position/duration (when they change; ≤ 10/s).
   - `visualizer`: levels + bands for the audible load (only while enabled).
5. **Commands** (`commands.rs`): `intent(intent)` forwarding to the actor, `snapshot()` to fetch the current one at startup. Keep commands thin.
6. **Cover art protocol**: register a custom URI scheme (`coda://cover/...` via `register_asynchronous_uri_scheme_protocol`) serving image bytes for local tracks (embedded tag / folder image — add a bytes-returning loader to `coda-core::cover::local` if needed, or encode `CoverImage` to JPEG/PNG) and Navidrome cover ids (via `Client::cover_art` + Coda's disk cache). Sizes `small` (256) and `large` (1024). Include the cover URL in track DTOs. Set a strict CSP in `tauri.conf.json` allowing `coda:` (and the platform-specific `http://coda.localhost` form on Windows/Linux as Tauri requires) for images.
7. **Lifecycle**: on window close / app exit, run the final session save then `PlaybackHandle::shutdown` (bounded), and only then exit. Verify no mpv process remains.
8. **Tests**: actor logic tested without a window (e.g. drive the engine core with fake events, or a `--ao=null` mpv test marked `#[ignore]`). DTO serialization snapshot tests.
9. Remove the scaffold's `greet` command and `tauri-plugin-opener` if unused; minimal capabilities.

## Frontend (`src/`)
1. Replace the scaffold. Structure: `src/lib/types.ts` (mirror of DTOs), `src/lib/ipc.ts` (typed `intent()` + event subscriptions), `src/state/` (a small store fed by `snapshot`/`progress` events; `useSyncExternalStore` or zustand — your choice, keep it tiny), `src/components/`, `src/views/`, `src/styles/tokens.css` + base styles.
2. The **app shell from `docs/DESIGN.md`**: sidebar (entries present but browse views may be placeholders saying "coming soon" for now), main area, collapsible queue panel, now-playing bar with cover, title/artist, prev/play-pause/next, seek bar (drag to seek), elapsed/total, repeat toggle, volume slider. Queue panel: current track highlighted, double-click plays an entry, remove on hover/context. Problem banner + notice toasts.
3. Global keyboard shortcuts from DESIGN.md that apply to this slice.
4. Native feel per DESIGN.md (no text selection on chrome, no page scroll, default cursors). Dark + light tokens.

## Acceptance
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm build` all pass.
- `pnpm tauri build --debug --no-bundle` (or `cargo build -p coda-app`) builds.
- Do NOT open the app window on the owner's desktop. Instead write `scripts/smoke.md` with the manual checks the owner should do.
- Update `docs/ARCHITECTURE.md` (new) to describe what exists.
- Don't commit. Report: files, design decisions (especially `TrackRef`, queue keys, event shapes), checks run and results, open questions.
